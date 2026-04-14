use std::path::PathBuf;
use std::{fs, path::Path};

#[cfg(feature = "native")]
use super::backend_policy::should_run_tests_native;
use super::{
    flags::DriverFlags,
    frontend::{collect_roots, inject_flow_prelude},
    module_compile::tag_module_diagnostics,
    session::DriverSession,
    shared::{
        DriverDiagnosticConfig, emit_diagnostics_or_exit, sort_stdlib_first, tag_and_attach_file,
    },
    support::shared::{DiagnosticRenderRequest, emit_diagnostics},
};
use crate as flux;
use flux::{
    bytecode::{
        compiler::Compiler,
        vm::VM,
        vm::test_runner::{collect_test_functions, print_test_report, run_tests},
    },
    diagnostics::{Diagnostic, DiagnosticPhase},
    syntax::{
        lexer::Lexer,
        module_graph::{ModuleGraph, ModuleKind},
        parser::Parser,
    },
};

pub(crate) struct TestRunRequest<'a> {
    pub(crate) flags: &'a DriverFlags,
    pub(crate) session: &'a DriverSession,
}

/// Parsed source file plus module graph roots for a test run.
struct ParsedTestFile {
    source: String,
    roots: Vec<PathBuf>,
    parser: Parser,
    program: flux::syntax::program::Program,
}

/// Loads and parses a test file before graph construction.
fn load_test_file(path: &str, request: &TestRunRequest<'_>) -> ParsedTestFile {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            std::process::exit(1);
        }
    };

    let entry_path = Path::new(path);
    let roots = collect_roots(
        entry_path,
        &request.session.roots,
        request.session.roots_only,
    );
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    ParsedTestFile {
        source,
        roots,
        parser,
        program,
    }
}

/// Emits parse diagnostics for the initial test file and exits on parse errors.
fn emit_parse_diagnostics_or_exit(
    path: &str,
    source: &str,
    parser: &mut Parser,
    session: &DriverSession,
    all_diagnostics: &mut Vec<Diagnostic>,
) {
    let mut parse_warnings = parser.take_warnings();
    tag_and_attach_file(&mut parse_warnings, DiagnosticPhase::Parse, path);
    all_diagnostics.append(&mut parse_warnings);

    if !parser.errors.is_empty() {
        tag_and_attach_file(&mut parser.errors, DiagnosticPhase::Parse, path);
        emit_diagnostics(DiagnosticRenderRequest {
            diagnostics: &parser.errors,
            default_file: Some(path),
            default_source: Some(source),
            show_file_headers: false,
            max_errors: session.max_errors,
            format: session.diagnostics_format,
            all_errors: session.all_errors,
            text_to_stderr: true,
        });
        std::process::exit(1);
    }
}

/// Runs the discovered tests on the VM backend.
fn run_tests_vm(file_name: &str, compiler: &Compiler, tests: Vec<(String, usize)>) -> bool {
    let bytecode = compiler.bytecode();
    let mut vm = VM::new(bytecode);
    if let Err(err) = vm.run() {
        eprintln!("Error during test setup: {}", err);
        std::process::exit(1);
    }
    let results = run_tests(&mut vm, tests);
    print_test_report(file_name, &results)
}

/// Prints the empty-test discovery message for the current file.
fn print_no_tests_message(file_name: &str, filter: Option<&str>) {
    println!("Running tests in {}\n", file_name);
    if let Some(filter) = filter {
        println!("No test functions found matching filter `{}`.", filter);
    } else {
        println!("No test functions found (define functions named `test_*`).");
    }
}

/// Returns whether test execution should use the native backend.
fn should_use_native_test_backend(flags: &DriverFlags) -> bool {
    #[cfg(feature = "native")]
    {
        should_run_tests_native(flags)
    }

    #[cfg(not(feature = "native"))]
    {
        let _ = flags;
        false
    }
}

/// Applies the optional test-name filter and returns the remaining tests.
fn filter_tests_by_name(
    mut tests: Vec<(String, usize)>,
    filter: Option<&str>,
) -> Vec<(String, usize)> {
    if let Some(filter) = filter {
        tests.retain(|(name, _)| name.contains(filter));
    }
    tests
}

pub(crate) fn run_test_file(path: &str, request: TestRunRequest<'_>) {
    let ParsedTestFile {
        source,
        roots,
        mut parser,
        mut program,
    } = load_test_file(path, &request);
    let entry_path = Path::new(path);

    let mut all_diagnostics: Vec<Diagnostic> = Vec::new();
    emit_parse_diagnostics_or_exit(
        path,
        &source,
        &mut parser,
        request.session,
        &mut all_diagnostics,
    );

    inject_flow_prelude(&mut program, &mut parser, request.flags.is_native_backend());
    let interner = parser.take_interner();
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, &roots);
    let mut graph_diags = graph_result.diagnostics;
    tag_and_attach_file(&mut graph_diags, DiagnosticPhase::ModuleGraph, path);
    all_diagnostics.extend(graph_diags);

    let failed = graph_result.failed_modules;
    let module_count = graph_result.graph.module_count();
    let is_multimodule = module_count > 1;
    let graph = graph_result.graph;

    let mut compiler = Compiler::new_with_interner(path, graph_result.interner);
    compiler.set_strict_mode(request.session.strict_mode);
    compiler.set_strict_types(request.session.strict_types);
    let entry_canonical = std::fs::canonicalize(entry_path).ok();

    let mut ordered_nodes = graph.topo_order();
    sort_stdlib_first(&mut ordered_nodes, |node| node.kind);

    for node in ordered_nodes {
        if node.imports.iter().any(|e| failed.contains(&e.target_path)) {
            continue;
        }
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        compiler.set_current_module_kind(node.kind);
        let is_entry_module = entry_canonical.as_ref().is_some_and(|p| p == &node.path);
        let is_flow_library = node.kind == ModuleKind::FlowStdlib;
        compiler.set_strict_require_main(is_entry_module);
        if is_flow_library {
            compiler.set_strict_mode(false);
            compiler.set_strict_types(false);
        }
        let compile_result = compiler.compile_with_opts(
            &node.program,
            request.session.enable_optimize,
            request.session.enable_analyze,
        );
        if is_flow_library {
            compiler.set_strict_mode(request.session.strict_mode);
            compiler.set_strict_types(request.session.strict_types);
        }
        let mut compiler_warnings = compiler.take_warnings();
        tag_module_diagnostics(
            &mut compiler_warnings,
            DiagnosticPhase::Validation,
            &node.path,
        );
        all_diagnostics.append(&mut compiler_warnings);

        if let Err(mut diags) = compile_result {
            tag_module_diagnostics(&mut diags, DiagnosticPhase::TypeCheck, &node.path);
            all_diagnostics.append(&mut diags);
        }
    }

    emit_diagnostics_or_exit(
        &all_diagnostics,
        path,
        source.as_str(),
        is_multimodule,
        DriverDiagnosticConfig::from(request.session),
    );

    let tests = filter_tests_by_name(
        collect_test_functions(&compiler.symbol_table, &compiler.interner),
        request.flags.input.test_filter.as_deref(),
    );

    if tests.is_empty() {
        let file_name = entry_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path);
        print_no_tests_message(file_name, request.flags.input.test_filter.as_deref());
        return;
    }

    let file_name = entry_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);

    #[cfg(feature = "native")]
    let all_passed = if should_use_native_test_backend(request.flags) {
        run_tests_native(NativeTestRunConfig {
            file_name,
            source_path: path,
            source: &source,
            roots: &roots,
            tests: &tests,
            enable_optimize: request.session.enable_optimize,
            strict_mode: request.session.strict_mode,
            use_native: should_use_native_test_backend(request.flags),
        })
    } else {
        run_tests_vm(file_name, &compiler, tests)
    };

    #[cfg(not(feature = "native"))]
    let all_passed = run_tests_vm(file_name, &compiler, tests);

    if !all_passed {
        std::process::exit(1);
    }
}

#[cfg(feature = "native")]
struct NativeTestRunConfig<'a> {
    file_name: &'a str,
    source_path: &'a str,
    source: &'a str,
    roots: &'a [PathBuf],
    tests: &'a [(String, usize)],
    enable_optimize: bool,
    strict_mode: bool,
    use_native: bool,
}

#[cfg(feature = "native")]
fn run_tests_native(config: NativeTestRunConfig<'_>) -> bool {
    use flux::bytecode::vm::test_runner::{TestOutcome, TestResult};
    use std::process::Command;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    let exe = std::env::current_exe().unwrap_or_else(|e| {
        eprintln!("Failed to locate current executable for native test mode: {e}");
        std::process::exit(1);
    });
    let mut results = Vec::new();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    for (idx, (name, _)) in config.tests.iter().enumerate() {
        let harness_path = std::env::temp_dir().join(format!(
            "flux_native_test_{}_{}_{}.flx",
            std::process::id(),
            unique,
            idx
        ));
        let harness_source = format!("{}\n\nfn main() {{ {name}(); }}\n", config.source);
        if let Err(e) = std::fs::write(&harness_path, harness_source) {
            eprintln!(
                "Failed to write native test harness {}: {e}",
                harness_path.display()
            );
            std::process::exit(1);
        }

        let start = Instant::now();
        let mut cmd = Command::new(&exe);
        if config.use_native {
            cmd.arg("--native");
        }
        cmd.arg("--no-cache");
        if config.enable_optimize {
            cmd.arg("--optimize");
        }
        if config.strict_mode {
            cmd.arg("--strict");
        }
        for root in config.roots {
            cmd.arg("--root").arg(root);
        }
        cmd.arg(&harness_path);
        cmd.env("NO_COLOR", "1");
        let output = cmd.output();
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

        let outcome = match output {
            Ok(output) if output.status.success() => TestOutcome::Pass,
            Ok(output) => {
                let mut text = String::new();
                text.push_str(&String::from_utf8_lossy(&output.stdout));
                text.push_str(&String::from_utf8_lossy(&output.stderr));
                TestOutcome::Fail(text.trim().to_string())
            }
            Err(err) => TestOutcome::Fail(format!(
                "failed to run native test harness for {} (from {}): {}",
                name, config.source_path, err
            )),
        };

        let _ = std::fs::remove_file(&harness_path);
        results.push(TestResult {
            name: name.clone(),
            elapsed_ms,
            outcome,
        });
    }

    print_test_report(config.file_name, &results)
}

#[cfg(test)]
mod tests {
    use super::{filter_tests_by_name, should_use_native_test_backend};
    use crate::driver::{backend::Backend, test_support::base_flags};

    #[test]
    fn default_test_backend_uses_vm() {
        let flags = base_flags();

        assert!(!should_use_native_test_backend(&flags));
    }

    #[test]
    fn native_test_backend_uses_native_when_selected() {
        let mut flags = base_flags();
        flags.backend.selected = Backend::Native;

        #[cfg(feature = "native")]
        assert!(should_use_native_test_backend(&flags));
        #[cfg(not(feature = "native"))]
        assert!(!should_use_native_test_backend(&flags));
    }

    #[test]
    fn filter_tests_keeps_matching_names_only() {
        let tests = vec![
            ("test_alpha".to_string(), 1),
            ("test_beta".to_string(), 2),
            ("helper".to_string(), 3),
        ];

        let filtered = filter_tests_by_name(tests, Some("beta"));

        assert_eq!(filtered, vec![("test_beta".to_string(), 2)]);
    }

    #[test]
    fn filter_tests_returns_empty_when_no_names_match() {
        let tests = vec![("test_alpha".to_string(), 1), ("test_beta".to_string(), 2)];

        let filtered = filter_tests_by_name(tests, Some("gamma"));

        assert!(filtered.is_empty());
    }
}
