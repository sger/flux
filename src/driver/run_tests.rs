// `PathBuf` is used only by the native test path, which forwards `--root`
// flags to a subprocess.
#[cfg(feature = "llvm")]
use std::path::PathBuf;
use std::{fs, path::Path};

#[cfg(feature = "llvm")]
use super::backend_policy::should_run_tests_native;
use super::{
    flags::DriverFlags,
    frontend::{collect_module_roots, inject_flow_prelude, validate_no_primops_import},
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
    roots: Vec<flux::syntax::module_graph::ModuleRoot>,
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
    // Package roots, so a test file can import the project's path
    // dependencies exactly as the run path does (KI-021).
    let cache_layout = crate::shared::cache_paths::resolve_cache_layout(
        entry_path,
        request.session.cache_dir_path(),
    );
    let roots = match collect_module_roots(
        entry_path,
        &request.session.roots,
        request.session.roots_only,
        cache_layout.root(),
    ) {
        Ok(roots) => roots,
        Err(message) => {
            eprintln!("error: could not resolve the project manifest: {message}");
            std::process::exit(1);
        }
    };
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

/// Build the batched harness, handling a fixture that defines its own `main`.
///
/// A single top-level `main` is renamed out of the way so the generated
/// dispatcher can take the name; a file with several `main`s is run as-is
/// rather than rewritten.
#[cfg(any(feature = "llvm", test))]
fn build_batched_native_harness(
    source: &str,
    test_names: &[String],
) -> Result<NativeTestHarnessSource, String> {
    match analyze_top_level_main_usage(source)? {
        TopLevelMainAnalysis::NoMain => Ok(NativeTestHarnessSource::Generated(
            batched_test_harness_source(source, test_names),
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
            let rewritten = rewrite_source_range(source, main_name_range, "__flux_test_user_main");
            Ok(NativeTestHarnessSource::Generated(
                batched_test_harness_source(&rewritten, test_names),
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

/// The environment variable a batched native harness reads to decide which
/// test to run.
///
/// An environment variable rather than a command-line argument because the
/// child is invoked through the `flux` driver, whose own argument parsing sits
/// between us and the program; an env var reaches the process untouched and
/// needs no passthrough convention.
#[cfg(any(feature = "llvm", test))]
pub(crate) const NATIVE_TEST_INDEX_VAR: &str = "FLUX_NATIVE_TEST_INDEX";

/// Build one harness that can run *any* test in the file, selected at runtime.
///
/// The per-test harness this replaces compiled and linked the whole module
/// graph once per test function, so a 58-test fixture paid 58 native builds —
/// about 2.6s each, near-uniform regardless of what the test computed.
/// Batching makes the build cost `O(1)` in the number of tests: the first
/// invocation compiles, the rest hit the compile cache.
///
/// Isolation is preserved. Each test still runs in its **own process**; only
/// the *binary* is shared. A test that panics, aborts, or corrupts memory takes
/// down only its own run, exactly as before — which is the property the
/// per-test harness was really buying.
///
/// The dispatch `match` is written **inline in `main`** rather than extracted
/// into a helper. The driver exempts a program's entry point from declaring an
/// effect row, but that exemption does not extend to a function `main` calls:
/// a helper calling a `with IO` test would fail with E400.
#[cfg(any(feature = "llvm", test))]
fn batched_test_harness_source(source: &str, test_names: &[String]) -> String {
    let source = source.trim_end_matches('\n');

    // Arms are keyed by index rather than by test name so that nothing in a
    // name can terminate the generated string literal.
    let arms: String = test_names
        .iter()
        .enumerate()
        .map(|(idx, name)| format!("        \"{idx}\" -> {name}(),\n"))
        .collect();

    // A distinctive alias: the fixture may already import `Flow.Env` under its
    // own name, and importing one module twice under two aliases is allowed.
    let var = NATIVE_TEST_INDEX_VAR;
    let mut out = String::with_capacity(source.len() + 512);
    out.push_str("import Flow.Env as FluxNativeTestEnv\n");
    out.push_str(source);
    out.push_str("\n\n");
    out.push_str("// Generated by the native test runner: runs the one test named\n");
    out.push_str("// by the index in the environment, defaulting to the first.\n");
    // `with Env` is declared explicitly rather than left to inference.
    //
    // A program entry point may leave its row to be inferred from what it
    // calls, which is enough when every test in the file is `with IO` — the
    // ambient row then covers the `Env` read too. But a fixture whose tests are
    // all pure (`tests/flux/stdlib_either.flx`) gives `main` nothing to infer
    // from, and the `var_or` call fails with E400. Declaring the row makes the
    // harness independent of what the fixture's tests happen to do.
    out.push_str("fn main() with Env {\n");
    out.push_str(&format!(
        "    match FluxNativeTestEnv.var_or(\"{var}\", \"0\") {{\n"
    ));
    out.push_str(&arms);
    out.push_str("        __unknown -> panic(\n");
    out.push_str("            \"native test harness: no test at index \" + __unknown\n");
    out.push_str("        ),\n");
    out.push_str("    }\n");
    out.push_str("}\n");
    out
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

    let mut primops_import_diags = validate_no_primops_import(&program, parser.interner(), path);
    if !primops_import_diags.is_empty() {
        tag_and_attach_file(&mut primops_import_diags, DiagnosticPhase::Parse, path);
        all_diagnostics.append(&mut primops_import_diags);
    }
    inject_flow_prelude(
        &mut program,
        &mut parser,
        request.flags.is_native_backend(),
        Path::new(path),
    );
    let interner = parser.take_interner();
    let graph_result =
        ModuleGraph::build_with_entry_and_module_roots(entry_path, &program, interner, &roots);
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
    let nodes_by_path: std::collections::HashMap<_, _> = ordered_nodes
        .iter()
        .map(|node| (node.path.clone(), node))
        .collect();

    for node in &ordered_nodes {
        if node.imports.iter().any(|e| failed.contains(&e.target_path)) {
            continue;
        }
        // Preload each module's dependencies, as the `run` drivers do
        // (`src/driver/run_program/modules.rs`). One `Compiler` compiles every
        // module here, but a compile does not leave the previous module's
        // public classes and instances in the place the next one reads them
        // from: `Flow.Ord`'s instances are `Eq<Int> => Ord<Int>`, and without
        // `Flow.Eq`'s instances in scope none of them can be registered, so
        // `Flow.Ord` failed to compile at all (E444 on its own methods).
        for dep in &node.imports {
            if let Some(dep_node) = nodes_by_path.get(&dep.target_path) {
                compiler.preload_dependency_program(&dep_node.program);
            }
        }
        if node.kind != flux::syntax::module_graph::ModuleKind::FlowStdlib {
            for (dep_path, dep_node) in &nodes_by_path {
                if !node.imports.iter().any(|dep| &dep.target_path == dep_path)
                    && dep_node.kind == flux::syntax::module_graph::ModuleKind::FlowStdlib
                {
                    compiler.preload_dependency_program(&dep_node.program);
                }
            }
        }
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        compiler.set_current_module_kind(node.kind);
        let module_strict_mode =
            effective_module_strictness(node.kind, entry_module_kind, request.session.strict_mode);
        compiler.set_strict_mode(module_strict_mode);
        compiler.set_strict_require_main(false);
        // record this module's record-style constructor field
        // order before compiling it, so a *later* module in the topo order can
        // desugar named-field syntax naming one of its constructors. Unlike
        // the `run` drivers, the test runner compiles every module through one
        // `Compiler` without a preload step, so without this the field order
        // for an imported constructor is never seen and the desugaring emits a
        // zero-field constructor (E082 / E085).
        compiler.preload_ctor_field_names_from_program(&node.program);
        let compile_result = compiler.compile_with_opts(
            &node.program,
            request.session.enable_optimize,
            request.session.enable_analyze,
        );
        // An effect declared here has to stay visible when a later module
        // annotates `with <Effect>` or writes a `handle` block: each compile
        // resets the effect registry from the preloaded set, so this module's
        // declarations are promoted into it. See docs/known_issues.md#ki-028.
        compiler.promote_effect_declarations();
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
            // The child resolves the project manifest itself, so only the
            // user's explicit `--root` flags are forwarded; passing the
            // resolved package roots would re-add them unscoped.
            roots: &request.session.roots,
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
    cache_dir: &Path,
) {
    // A cache directory private to this file's run.
    //
    // The children now share a cache instead of each passing `--no-cache` —
    // that is what makes the build cost O(1) rather than O(tests). Sharing it
    // any wider would not be safe: concurrent test binaries writing one cache
    // root is KI-010, and it surfaced here as an intermittent
    // `unhandled effect` from a half-written artifact.
    cmd.arg("--cache-dir").arg(cache_dir);
    if config.use_native {
        cmd.arg("--native");
    }
    // Deliberately *not* `--no-cache`: the private cache above is what lets the
    // first child compile the harness and the rest reuse it.
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

    // Prepare one harness for the whole file, before the loop.
    //
    // Write it inside the project build tree (derived from the real test
    // file's location), NOT the system temp dir. If it lived in %TEMP%, the
    // child compile couldn't find a project root and its whole cache +
    // native-scratch chain — including `program.exe` — would fall back into
    // %TEMP%, where a fresh unsigned exe trips Windows Defender's
    // malware-dropper heuristic (`Trojan:Win32/Wacatac.B!ml`).
    let harness_dir =
        flux::shared::cache_paths::resolve_cache_layout(Path::new(config.source_path), None)
            .native_dir()
            .join("test-harness");
    let _ = std::fs::create_dir_all(&harness_dir);
    let harness_path = harness_dir.join(format!(
        "flux_native_test_{}_{}.flx",
        std::process::id(),
        unique
    ));

    // One cache root per file per run, beside the harness it serves.
    let child_cache_dir = harness_dir.join(format!("cache_{}_{}", std::process::id(), unique));

    let test_names: Vec<String> = config.tests.iter().map(|(name, _)| name.clone()).collect();
    let harness = match build_batched_native_harness(config.source, &test_names) {
        Ok(harness) => harness,
        Err(err) => {
            // The whole file is unrunnable natively, so report the reason
            // against every test rather than silently running none.
            for (name, _) in config.tests {
                results.push(TestResult {
                    name: name.clone(),
                    elapsed_ms: 0.0,
                    outcome: TestOutcome::Fail(err.clone()),
                });
            }
            return print_test_report(config.file_name, &results);
        }
    };

    let generated_harness = matches!(harness, NativeTestHarnessSource::Generated(_));
    let child_source_path = if generated_harness {
        harness_path.as_path()
    } else {
        Path::new(config.source_path)
    };
    if let NativeTestHarnessSource::Generated(ref source_text) = harness
        && let Err(e) = std::fs::write(&harness_path, source_text)
    {
        eprintln!(
            "Failed to write native test harness {}: {e}",
            harness_path.display()
        );
        std::process::exit(1);
    }

    // One process per test, but one *binary* for all of them: the first
    // invocation compiles and links, the rest hit the compile cache. Isolation
    // is unchanged — a test that panics or aborts still takes down only its own
    // process.
    for (idx, (name, _)) in config.tests.iter().enumerate() {
        let start = Instant::now();
        let mut cmd = Command::new(&exe);
        append_native_test_command_args(&mut cmd, &config, child_source_path, &child_cache_dir);
        cmd.env("NO_COLOR", "1");
        if generated_harness {
            cmd.env(NATIVE_TEST_INDEX_VAR, idx.to_string());
        }
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

        results.push(TestResult {
            name: name.clone(),
            elapsed_ms,
            outcome,
        });
    }

    if generated_harness {
        let _ = std::fs::remove_file(&harness_path);
    }
    // The cache exists only to be shared between this file's children.
    let _ = std::fs::remove_dir_all(&child_cache_dir);

    print_test_report(config.file_name, &results)
}

#[cfg(test)]
mod tests {
    use super::{
        NativeTestHarnessSource, build_batched_native_harness, filter_tests_by_name,
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
    fn batched_harness_dispatches_every_test_by_index() {
        let harness = build_batched_native_harness(
            "fn test_a() { 0 }\nfn test_b() { 0 }\n",
            &["test_a".to_string(), "test_b".to_string()],
        )
        .unwrap();

        let NativeTestHarnessSource::Generated(harness) = harness else {
            panic!("expected generated harness");
        };

        assert!(harness.contains("\"0\" -> test_a(),"));
        assert!(harness.contains("\"1\" -> test_b(),"));
        // `with Env` is not optional: a fixture whose tests are all pure gives
        // `main` no ambient row to infer the `var_or` read from.
        assert!(harness.contains("fn main() with Env {"));
        assert!(harness.starts_with("import Flow.Env as FluxNativeTestEnv\n"));
    }

    #[test]
    fn batched_harness_renames_a_single_top_level_main() {
        let harness = build_batched_native_harness(
            "fn main() { 0 }\nfn test_ok() { 0 }\n",
            &["test_ok".to_string()],
        )
        .unwrap();

        let NativeTestHarnessSource::Generated(harness) = harness else {
            panic!("expected generated harness");
        };

        assert!(harness.contains("fn __flux_test_user_main() { 0 }"));
        assert!(harness.contains("fn main() with Env {"));
        assert!(!harness.contains("fn main() { 0 }"));
    }

    #[test]
    fn batched_harness_preserves_qualified_test_names() {
        let harness = build_batched_native_harness(
            "module Tests { fn test_inside() { 0 } }\n",
            &["Tests.test_inside".to_string()],
        )
        .unwrap();

        let NativeTestHarnessSource::Generated(harness) = harness else {
            panic!("expected generated harness");
        };

        assert!(harness.contains("\"0\" -> Tests.test_inside(),"));
    }

    #[test]
    fn batched_harness_rejects_additional_main_references() {
        let err = build_batched_native_harness(
            "fn main() { main() }\nfn test_ok() { 0 }\n",
            &["test_ok".to_string()],
        )
        .unwrap_err();

        assert!(err.contains("does not support additional `main` references"));
    }

    #[test]
    fn batched_harness_preserves_original_source_for_duplicate_top_level_main() {
        let harness = build_batched_native_harness(
            "fn main() { 0 }\nfn main() { 1 }\nfn test_ok() { 0 }\n",
            &["test_ok".to_string()],
        )
        .unwrap();

        assert_eq!(harness, NativeTestHarnessSource::OriginalSource);
    }

    /// An out-of-range index is a bug in the runner, not in the program under
    /// test, so the harness must fail loudly rather than silently pass.
    #[test]
    fn batched_harness_panics_on_an_unknown_index() {
        let harness =
            build_batched_native_harness("fn test_a() { 0 }\n", &["test_a".to_string()]).unwrap();

        let NativeTestHarnessSource::Generated(harness) = harness else {
            panic!("expected generated harness");
        };

        assert!(harness.contains("panic("));
        assert!(harness.contains("no test at index"));
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

        append_native_test_command_args(
            &mut cmd,
            &config,
            Path::new("rewritten.flx"),
            Path::new("cache"),
        );

        let args: Vec<_> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            args,
            vec![
                // A private cache root, not `--no-cache`: the children share one
                // compiled harness, so the first builds it and the rest reuse it.
                "--cache-dir",
                "cache",
                "--native",
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
