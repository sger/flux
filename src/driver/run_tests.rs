use std::path::PathBuf;
use std::{fs, path::Path};

#[cfg(feature = "llvm")]
use super::backend_policy::should_run_tests_native;
use super::{
    flags::DriverFlags,
    frontend::{collect_roots, inject_flow_prelude},
    module_compile::{effective_module_strictness, tag_module_diagnostics},
    session::DriverSession,
    shared::{
        DriverDiagnosticConfig, emit_diagnostics_or_exit, sort_stdlib_first, tag_and_attach_file,
    },
    support::shared::{DiagnosticRenderRequest, emit_diagnostics},
};
use crate as flux;
use flux::{
    compiler::Compiler,
    diagnostics::{Diagnostic, DiagnosticPhase},
    syntax::{lexer::Lexer, module_graph::ModuleGraph, parser::Parser},
    vm::VM,
    vm::test_runner::{collect_test_functions, print_test_report, run_tests},
};
#[cfg(any(feature = "llvm", test))]
use flux::{
    diagnostics::position::Position,
    syntax::{token::Token, token_type::TokenType},
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
#[cfg_attr(not(feature = "llvm"), allow(dead_code))]
fn should_use_native_test_backend(flags: &DriverFlags) -> bool {
    #[cfg(feature = "llvm")]
    {
        should_run_tests_native(flags)
    }

    #[cfg(not(feature = "llvm"))]
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

#[cfg(any(feature = "llvm", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeTestHarnessSource {
    Generated(String),
    OriginalSource,
}

#[cfg(any(feature = "llvm", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceRewriteRange {
    start: usize,
    end: usize,
}

#[cfg(any(feature = "llvm", test))]
fn build_native_test_harness_source(
    source: &str,
    test_name: &str,
    hidden_main_name: &str,
) -> Result<NativeTestHarnessSource, String> {
    let analysis = analyze_top_level_main_usage(source)?;
    match analysis {
        TopLevelMainAnalysis::NoMain => Ok(NativeTestHarnessSource::Generated(
            synthetic_test_harness_source(source, test_name),
        )),
        TopLevelMainAnalysis::SingleMain {
            main_name_range,
            has_additional_main_references,
        } => {
            if has_additional_main_references {
                return Err(
                    "native test harness rewriting does not support additional `main` references yet; remove explicit `main` references or run tests without `--native`.".to_string(),
                );
            }
            let rewritten = rewrite_source_range(source, main_name_range, hidden_main_name);
            Ok(NativeTestHarnessSource::Generated(
                synthetic_test_harness_source(&rewritten, test_name),
            ))
        }
        TopLevelMainAnalysis::MultipleMains => Ok(NativeTestHarnessSource::OriginalSource),
    }
}

#[cfg(any(feature = "llvm", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopLevelMainAnalysis {
    NoMain,
    SingleMain {
        main_name_range: SourceRewriteRange,
        has_additional_main_references: bool,
    },
    MultipleMains,
}

#[cfg(any(feature = "llvm", test))]
fn analyze_top_level_main_usage(source: &str) -> Result<TopLevelMainAnalysis, String> {
    let mut lexer = Lexer::new(source);
    let mut brace_depth = 0usize;
    let mut expect_function_name = false;
    let mut function_name_depth = 0usize;
    let mut previous_token_type = None;
    let mut top_level_main_ranges = Vec::new();
    let mut has_additional_main_references = false;

    loop {
        let token = lexer.next_token();
        let token_type = token.token_type;

        if token_type == TokenType::Eof {
            break;
        }

        let is_function_name = expect_function_name && token_type == TokenType::Ident;
        if expect_function_name {
            expect_function_name = false;
            if is_function_name && token.literal.as_str() == "main" && function_name_depth == 0 {
                top_level_main_ranges.push(token_rewrite_range(source, &token)?);
            }
        } else if token_type == TokenType::Fn {
            expect_function_name = true;
            function_name_depth = brace_depth;
        } else if token_type == TokenType::Ident
            && token.literal.as_str() == "main"
            && previous_token_type != Some(TokenType::Dot)
        {
            has_additional_main_references = true;
        }

        match token_type {
            TokenType::LBrace => brace_depth += 1,
            TokenType::RBrace => brace_depth = brace_depth.saturating_sub(1),
            _ => {}
        }

        previous_token_type = Some(token_type);
    }

    Ok(match top_level_main_ranges.len() {
        0 => TopLevelMainAnalysis::NoMain,
        1 => TopLevelMainAnalysis::SingleMain {
            main_name_range: top_level_main_ranges[0],
            has_additional_main_references,
        },
        _ => TopLevelMainAnalysis::MultipleMains,
    })
}

#[cfg(any(feature = "llvm", test))]
fn synthetic_test_harness_source(source: &str, test_name: &str) -> String {
    let source = source.trim_end_matches('\n');
    format!("{source}\n\nfn main() {{ {test_name}(); }}\n")
}

#[cfg(any(feature = "llvm", test))]
fn rewrite_source_range(source: &str, range: SourceRewriteRange, replacement: &str) -> String {
    let mut rewritten = String::with_capacity(source.len() + replacement.len());
    rewritten.push_str(&source[..range.start]);
    rewritten.push_str(replacement);
    rewritten.push_str(&source[range.end..]);
    rewritten
}

#[cfg(any(feature = "llvm", test))]
fn token_rewrite_range(source: &str, token: &Token) -> Result<SourceRewriteRange, String> {
    Ok(SourceRewriteRange {
        start: position_to_byte_offset(source, token.position)?,
        end: position_to_byte_offset(source, token.end_position)?,
    })
}

#[cfg(any(feature = "llvm", test))]
fn position_to_byte_offset(source: &str, position: Position) -> Result<usize, String> {
    if position.line == 0 {
        return Err("invalid lexer position: line 0".to_string());
    }

    let line_start = source
        .lines()
        .enumerate()
        .find_map(|(idx, line)| {
            if idx + 1 == position.line {
                Some(line)
            } else {
                None
            }
        })
        .ok_or_else(|| format!("invalid lexer line {}", position.line))?;

    let line_offset = source
        .split_inclusive('\n')
        .take(position.line.saturating_sub(1))
        .map(str::len)
        .sum::<usize>();

    let mut column_chars = 0usize;
    for (byte_idx, _) in line_start.char_indices() {
        if column_chars == position.column {
            return Ok(line_offset + byte_idx);
        }
        column_chars += 1;
    }

    if column_chars == position.column {
        return Ok(line_offset + line_start.len());
    }

    Err(format!(
        "invalid lexer column {} on line {}",
        position.column, position.line
    ))
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
    let entry_module_kind = graph.entry_node().map(|node| node.kind).unwrap_or_default();

    let mut compiler = Compiler::new_with_interner(path, graph_result.interner);
    compiler.set_strict_mode(request.session.strict_mode);

    let mut ordered_nodes = graph.topo_order();
    sort_stdlib_first(&mut ordered_nodes, |node| node.kind);

    for node in ordered_nodes {
        if node.imports.iter().any(|e| failed.contains(&e.target_path)) {
            continue;
        }
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        compiler.set_current_module_kind(node.kind);
        let module_strict_mode =
            effective_module_strictness(node.kind, entry_module_kind, request.session.strict_mode);
        compiler.set_strict_mode(module_strict_mode);
        compiler.set_strict_require_main(false);
        let compile_result = compiler.compile_with_opts(
            &node.program,
            request.session.enable_optimize,
            request.session.enable_analyze,
        );
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

    #[cfg(feature = "llvm")]
    let all_passed = if should_use_native_test_backend(request.flags) {
        run_tests_native(NativeTestRunConfig {
            file_name,
            source_path: path,
            source: &source,
            roots: &roots,
            roots_only: request.session.roots_only,
            tests: &tests,
            enable_optimize: request.session.enable_optimize,
            enable_analyze: request.session.enable_analyze,
            strict_mode: request.session.strict_mode,
            use_native: should_use_native_test_backend(request.flags),
        })
    } else {
        run_tests_vm(file_name, &compiler, tests)
    };

    #[cfg(not(feature = "llvm"))]
    let all_passed = run_tests_vm(file_name, &compiler, tests);

    if !all_passed {
        std::process::exit(1);
    }
}

#[cfg(feature = "llvm")]
struct NativeTestRunConfig<'a> {
    file_name: &'a str,
    source_path: &'a str,
    source: &'a str,
    roots: &'a [PathBuf],
    roots_only: bool,
    tests: &'a [(String, usize)],
    enable_optimize: bool,
    enable_analyze: bool,
    strict_mode: bool,
    use_native: bool,
}

#[cfg(feature = "llvm")]
fn append_native_test_command_args(
    cmd: &mut std::process::Command,
    config: &NativeTestRunConfig<'_>,
    source_path: &Path,
) {
    if config.use_native {
        cmd.arg("--native");
    }
    cmd.arg("--no-cache");
    if config.enable_optimize {
        cmd.arg("--optimize");
    }
    if config.enable_analyze {
        cmd.arg("--analyze");
    }
    if config.strict_mode {
        cmd.arg("--strict");
    }
    if config.roots_only {
        cmd.arg("--roots-only");
    }
    for root in config.roots {
        cmd.arg("--root").arg(root);
    }
    cmd.arg(source_path);
}

#[cfg(feature = "llvm")]
fn run_tests_native(config: NativeTestRunConfig<'_>) -> bool {
    use flux::vm::test_runner::{TestOutcome, TestResult};
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
        let start = Instant::now();
        let hidden_main_name = format!("__flux_test_user_main_{}_{}", unique, idx);
        let harness_source =
            match build_native_test_harness_source(config.source, name, &hidden_main_name) {
                Ok(harness_source) => harness_source,
                Err(err) => {
                    results.push(TestResult {
                        name: name.clone(),
                        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                        outcome: TestOutcome::Fail(err),
                    });
                    continue;
                }
            };
        let harness_path = std::env::temp_dir().join(format!(
            "flux_native_test_{}_{}_{}.flx",
            std::process::id(),
            unique,
            idx
        ));
        let mut cmd = Command::new(&exe);
        let generated_harness = matches!(harness_source, NativeTestHarnessSource::Generated(_));
        let child_source_path = if generated_harness {
            harness_path.as_path()
        } else {
            Path::new(config.source_path)
        };
        if let NativeTestHarnessSource::Generated(ref source_text) = harness_source
            && let Err(e) = std::fs::write(&harness_path, source_text) {
                eprintln!(
                    "Failed to write native test harness {}: {e}",
                    harness_path.display()
                );
                std::process::exit(1);
            }
        append_native_test_command_args(&mut cmd, &config, child_source_path);
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

        if generated_harness {
            let _ = std::fs::remove_file(&harness_path);
        }
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
    use super::{
        NativeTestHarnessSource, build_native_test_harness_source, filter_tests_by_name,
        should_use_native_test_backend,
    };
    #[cfg(feature = "llvm")]
    use super::{NativeTestRunConfig, append_native_test_command_args};
    use crate::driver::{backend::Backend, test_support::base_flags};
    #[cfg(feature = "llvm")]
    use std::path::{Path, PathBuf};

    #[test]
    fn default_test_backend_uses_vm() {
        let flags = base_flags();

        assert!(!should_use_native_test_backend(&flags));
    }

    #[test]
    fn native_test_backend_uses_native_when_selected() {
        let mut flags = base_flags();
        flags.backend.selected = Backend::Native;

        #[cfg(feature = "llvm")]
        assert!(should_use_native_test_backend(&flags));
        #[cfg(not(feature = "llvm"))]
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

    #[test]
    fn harness_builder_appends_synthetic_main_when_source_has_no_main() {
        let harness =
            build_native_test_harness_source("fn test_ok() { 0 }\n", "test_ok", "__hidden")
                .unwrap();

        assert_eq!(
            harness,
            NativeTestHarnessSource::Generated(
                "fn test_ok() { 0 }\n\nfn main() { test_ok(); }\n".to_string()
            )
        );
    }

    #[test]
    fn harness_builder_renames_single_top_level_main_before_appending_test_main() {
        let harness = build_native_test_harness_source(
            "fn main() { 0 }\nfn test_ok() { 0 }\n",
            "test_ok",
            "__flux_test_user_main_1",
        )
        .unwrap();

        let NativeTestHarnessSource::Generated(harness) = harness else {
            panic!("expected generated harness");
        };

        assert!(harness.contains("fn __flux_test_user_main_1() { 0 }"));
        assert!(harness.contains("fn main() { test_ok(); }"));
        assert!(!harness.contains("fn main() { 0 }"));
    }

    #[test]
    fn harness_builder_preserves_qualified_test_names() {
        let harness = build_native_test_harness_source(
            "module Tests { fn test_inside() { 0 } }\n",
            "Tests.test_inside",
            "__hidden",
        )
        .unwrap();

        assert_eq!(
            harness,
            NativeTestHarnessSource::Generated(
                "module Tests { fn test_inside() { 0 } }\n\nfn main() { Tests.test_inside(); }\n"
                    .to_string()
            )
        );
    }

    #[test]
    fn harness_builder_rejects_additional_main_references() {
        let err = build_native_test_harness_source(
            "fn main() { main() }\nfn test_ok() { 0 }\n",
            "test_ok",
            "__hidden",
        )
        .unwrap_err();

        assert!(err.contains("does not support additional `main` references"));
    }

    #[test]
    fn harness_builder_preserves_original_source_for_duplicate_top_level_main() {
        let harness = build_native_test_harness_source(
            "fn main() { 0 }\nfn main() { 1 }\nfn test_ok() { 0 }\n",
            "test_ok",
            "__hidden",
        )
        .unwrap();

        assert_eq!(harness, NativeTestHarnessSource::OriginalSource);
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn native_test_command_forwards_language_and_root_flags() {
        let tests = vec![("test_ok".to_string(), 0)];
        let roots = vec![PathBuf::from("tests"), PathBuf::from("lib")];
        let config = NativeTestRunConfig {
            file_name: "sample.flx",
            source_path: "sample.flx",
            source: "fn test_ok() { 0 }",
            roots: &roots,
            roots_only: true,
            tests: &tests,
            enable_optimize: true,
            enable_analyze: true,
            strict_mode: true,
            use_native: true,
        };
        let mut cmd = std::process::Command::new("flux");

        append_native_test_command_args(&mut cmd, &config, Path::new("rewritten.flx"));

        let args: Vec<_> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            args,
            vec![
                "--native",
                "--no-cache",
                "--optimize",
                "--analyze",
                "--strict",
                "--roots-only",
                "--root",
                "tests",
                "--root",
                "lib",
                "rewritten.flx",
            ]
        );
    }
}
