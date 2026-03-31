use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    time::Instant,
};

use flux::syntax::program::Program;
use flux::{
    ast::{collect_free_vars_in_program, find_tail_calls},
    bytecode::vm::{
        VM,
        test_runner::{collect_test_functions, print_test_report, run_tests},
    },
    bytecode::{
        bytecode_cache::{
            BytecodeCache, hash_bytes, hash_cache_key, hash_file, module_cache::ModuleBytecodeCache,
        },
        compiler::Compiler,
        op_code::disassemble,
    },
    diagnostics::{
        DEFAULT_MAX_ERRORS, Diagnostic, DiagnosticPhase, DiagnosticsAggregator,
        quality::module_skipped_note, render_diagnostics_json, render_display_path,
    },
    runtime::value::Value,
    syntax::{
        formatter::format_source, lexer::Lexer, linter::Linter, module_graph::ModuleGraph,
        parser::Parser,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticOutputFormat {
    Text,
    Json,
    JsonCompact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreDumpMode {
    None,
    Readable,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AetherDumpMode {
    None,
    Summary,
    Debug,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraceBackend {
    Vm,
}

fn main() {
    let mut args: Vec<String> = env::args().collect();
    let verbose = args.iter().any(|arg| arg == "--verbose");
    let leak_detector = args.iter().any(|arg| arg == "--leak-detector");
    let trace = args.iter().any(|arg| arg == "--trace");
    let trace_aether = args.iter().any(|arg| arg == "--trace-aether");
    let no_cache = args.iter().any(|arg| arg == "--no-cache");
    let roots_only = args.iter().any(|arg| arg == "--roots-only");
    let enable_optimize = args.iter().any(|arg| arg == "--optimize" || arg == "-O");
    let enable_analyze = args.iter().any(|arg| arg == "--analyze" || arg == "-A");
    let show_stats = args.iter().any(|arg| arg == "--stats");
    let test_mode = args.iter().any(|arg| arg == "--test");
    let strict_mode = args.iter().any(|arg| arg == "--strict");
    let all_errors = args.iter().any(|arg| arg == "--all-errors");
    let dump_aether = if args.iter().any(|arg| arg == "--dump-aether=debug") {
        AetherDumpMode::Debug
    } else if args.iter().any(|arg| arg == "--dump-aether") {
        AetherDumpMode::Summary
    } else {
        AetherDumpMode::None
    };
    let dump_lir = args.iter().any(|arg| arg == "--dump-lir");
    #[cfg(feature = "core_to_llvm")]
    let dump_lir_llvm = args.iter().any(|arg| arg == "--dump-lir-llvm");
    #[cfg(not(feature = "core_to_llvm"))]
    let dump_lir_llvm = false;
    #[cfg(feature = "native")]
    let use_core_to_llvm = args
        .iter()
        .any(|arg| arg == "--core-to-llvm" || arg == "--native");
    #[cfg(not(feature = "native"))]
    let use_core_to_llvm = false;
    let emit_llvm = args.iter().any(|arg| arg == "--emit-llvm");
    let emit_binary = args.iter().any(|arg| arg == "--emit-binary");
    let mut roots = Vec::new();
    if verbose {
        args.retain(|arg| arg != "--verbose");
    }
    if leak_detector {
        args.retain(|arg| arg != "--leak-detector");
    }
    if trace {
        args.retain(|arg| arg != "--trace");
    }
    if trace_aether {
        args.retain(|arg| arg != "--trace-aether");
    }
    if no_cache {
        args.retain(|arg| arg != "--no-cache");
    }
    if roots_only {
        args.retain(|arg| arg != "--roots-only");
    }
    if enable_optimize {
        args.retain(|arg| arg != "--optimize" && arg != "-O");
    }
    if enable_analyze {
        args.retain(|arg| arg != "--analyze" && arg != "-A");
    }
    if show_stats {
        args.retain(|arg| arg != "--stats");
    }
    if test_mode {
        args.retain(|arg| arg != "--test");
    }
    if args.iter().any(|arg| arg == "--strict") {
        args.retain(|arg| arg != "--strict");
    }
    args.retain(|arg| arg != "--no-strict");
    if all_errors {
        args.retain(|arg| arg != "--all-errors");
    }
    if dump_aether != AetherDumpMode::None {
        args.retain(|arg| arg != "--dump-aether" && arg != "--dump-aether=debug");
    }
    if dump_lir {
        args.retain(|arg| arg != "--dump-lir");
    }
    if dump_lir_llvm {
        args.retain(|arg| arg != "--dump-lir-llvm");
    }
    if use_core_to_llvm {
        args.retain(|arg| arg != "--core-to-llvm" && arg != "--native");
    }
    if emit_llvm {
        args.retain(|arg| arg != "--emit-llvm");
    }
    if emit_binary {
        args.retain(|arg| arg != "--emit-binary");
    }
    let output_path = extract_output_path(&mut args);
    let dump_core = match extract_dump_core_mode(&mut args) {
        Some(value) => value,
        None => return,
    };
    let diagnostics_format = match extract_diagnostic_format(&mut args) {
        Some(value) => value,
        None => return,
    };
    let max_errors = match extract_max_errors(&mut args) {
        Some(value) => value,
        None => return,
    };
    let test_filter = match extract_test_filter(&mut args) {
        Some(value) => value,
        None => return,
    };
    if !extract_roots(&mut args, &mut roots) {
        return;
    }

    if trace_aether
        && (!matches!(dump_core, CoreDumpMode::None)
            || dump_aether != AetherDumpMode::None
            || test_mode)
    {
        eprintln!(
            "Error: --trace-aether only supports normal program execution. Use --dump-aether for report-only output."
        );
        return;
    }

    if args.len() < 2 {
        print_help();
        return;
    }

    if is_flx_file(&args[1]) {
        if test_mode {
            run_test_file(
                &args[1],
                roots_only,
                enable_optimize,
                enable_analyze,
                max_errors,
                &roots,
                test_filter.as_deref(),
                strict_mode,
                diagnostics_format,
                all_errors,
                use_core_to_llvm,
            );
        } else {
            run_file(
                &args[1],
                verbose,
                leak_detector,
                trace,
                no_cache,
                roots_only,
                enable_optimize,
                enable_analyze,
                max_errors,
                &roots,
                show_stats,
                trace_aether,
                strict_mode,
                diagnostics_format,
                all_errors,
                dump_core,
                dump_aether,
                dump_lir,
                dump_lir_llvm,
                use_core_to_llvm,
                emit_llvm,
                emit_binary,
                output_path.clone(),
            );
        }
        return;
    }

    match args[1].as_str() {
        "-h" | "--help" | "help" => {
            print_help();
        }
        "run" => {
            if args.len() < 3 {
                eprintln!("Usage: flux run <file.flx>");
                return;
            }

            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            if test_mode {
                run_test_file(
                    &args[2],
                    roots_only,
                    enable_optimize,
                    enable_analyze,
                    max_errors,
                    &roots,
                    test_filter.as_deref(),
                    strict_mode,
                    diagnostics_format,
                    all_errors,
                    use_core_to_llvm,
                );
            } else {
                run_file(
                    &args[2],
                    verbose,
                    leak_detector,
                    trace,
                    no_cache,
                    roots_only,
                    enable_optimize,
                    enable_analyze,
                    max_errors,
                    &roots,
                    show_stats,
                    trace_aether,
                    strict_mode,
                    diagnostics_format,
                    all_errors,
                    dump_core,
                    dump_aether,
                    dump_lir,
                    dump_lir_llvm,
                    use_core_to_llvm,
                    emit_llvm,
                    emit_binary,
                    output_path,
                );
            }
        }
        "tokens" => {
            if args.len() < 3 {
                eprintln!("Usage: flux tokens <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            show_tokens(&args[2]);
        }
        "bytecode" => {
            if args.len() < 3 {
                eprintln!("Usage: flux bytecode <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            show_bytecode(
                &args[2],
                enable_optimize,
                enable_analyze,
                max_errors,
                strict_mode,
                diagnostics_format,
            );
        }
        "lint" => {
            if args.len() < 3 {
                eprintln!("Usage: flux lint <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            lint_file(&args[2], max_errors, diagnostics_format);
        }
        "fmt" => {
            if args.len() < 3 {
                eprintln!("Usage: flux fmt [--check] <file.flx>");
                return;
            }
            let check = args.iter().any(|arg| arg == "--check");
            let file = if check { &args[3] } else { &args[2] };
            if check && args.len() < 4 {
                eprintln!("Usage: flux fmt --check <file.flx>");
                return;
            }
            if !is_flx_file(file) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    file
                );
                return;
            }
            fmt_file(file, check);
        }
        "cache-info" => {
            if args.len() < 3 {
                eprintln!("Usage: flux cache-info <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            show_cache_info(&args[2], &roots);
        }
        "cache-info-file" => {
            if args.len() < 3 {
                eprintln!("Usage: flux cache-info-file <file.fxc>");
                return;
            }
            show_cache_info_file(&args[2]);
        }
        "interface-info" => {
            if args.len() < 3 {
                eprintln!("Usage: flux interface-info <file.flxi>");
                return;
            }
            if !args[2].ends_with(".flxi") {
                eprintln!(
                    "Error: expected a `.flxi` file, got `{}`. Pass a Flux interface file like `path/to/module.flxi`.",
                    args[2]
                );
                return;
            }
            show_interface_info_file(&args[2]);
        }
        "analyze-free-vars" | "free-vars" => {
            if args.len() < 3 {
                eprintln!("Usage: flux analyze-free-vars <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            analyze_free_vars(&args[2], max_errors, diagnostics_format);
        }
        "analyze-tail-calls" | "analyze-tails-calls" | "tail-calls" => {
            if args.len() < 3 {
                eprintln!("Usage: flux analyze-tail-calls <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!(
                    "Error: expected a `.flx` file, got `{}`. Pass a Flux source file like `path/to/file.flx`.",
                    args[2]
                );
                return;
            }
            analyze_tail_calls(&args[2], max_errors, diagnostics_format);
        }
        "parity-check" => {
            let parity_args: Vec<String> = args[2..].to_vec();
            flux::parity::cli::run_parity_check(&parity_args);
        }
        _ => {
            eprintln!(
                "Error: unknown command or invalid input `{}`. Pass a `.flx` file or a valid subcommand.",
                args[1]
            );
        }
    }
}

fn print_help() {
    println!(
        "\
Flux CLI

Usage:
  flux <file.flx>
  flux run <file.flx>
  flux tokens <file.flx>
  flux bytecode <file.flx>
  flux lint <file.flx>
  flux fmt [--check] <file.flx>
  flux cache-info <file.flx>
  flux cache-info-file <file.fxc>
  flux interface-info <file.flxi>
  flux analyze-free-vars <file.flx>
  flux analyze-tail-calls <file.flx>
  flux parity-check <file-or-dir> [--ways vm,llvm] [--root <path> ...]
  flux <file.flx> --root <path> [--root <path> ...]
  flux run <file.flx> --root <path> [--root <path> ...]

Flags:
  --verbose          Show cache status (hit/miss/store)
  --trace            Print VM instruction trace
  --trace-aether     Print Aether report plus backend/execution path, then run
  --test             Run test_* functions and report results
  --test-filter <s>  Only run tests whose names contain <s>
  --leak-detector    Print approximate allocation stats after run
  --no-cache         Disable bytecode cache for this run
  --optimize, -O     Enable AST optimizations (desugar + constant fold)
  --analyze, -A      Enable analysis passes (free vars + tail calls)
  --format <f>       Diagnostics format: text|json|json-compact (default: text)
  --max-errors <n>   Limit displayed errors (default: 50)
  --root <path>      Add a module root (can be repeated)
  --roots-only       Use only explicitly provided --root values
  --stats            Print execution analytics (parse/compile/execute times, module info)
  --strict           Enable strict type/effect boundary checks
  --all-errors       Show diagnostics from all phases (disable stage-aware filtering)
  --dump-core        Lower to Flux Core IR, print a readable dump, and exit
  --dump-core=debug  Lower to Flux Core IR, print a raw debug dump, and exit
  --dump-aether      Show Aether memory model report (per-function reuse/drop stats)
  --dump-aether=debug
                    Show detailed Aether debug report (borrow signatures, call modes, dup/drop, reuse)
  --native           Compile via Core IR → LLVM text IR → native binary (requires LLVM tools)
  --core-to-llvm     Alias for --native
  --emit-llvm        Emit LLVM IR text (.ll) to stdout (with --native)
  --emit-binary      Compile to native binary via opt + llc + cc (with --native)
  -o <path>          Output path for --emit-llvm or --emit-binary
  -h, --help         Show this help message

Optimization & Analysis:
  --optimize         Apply transformations (faster bytecode)
  --analyze          Collect analysis data (free vars, tail calls)
  -O -A              Both optimization and analysis
"
    );
}

#[allow(clippy::too_many_arguments)]
fn run_file(
    path: &str,
    verbose: bool,
    leak_detector: bool,
    trace: bool,
    no_cache: bool,
    roots_only: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    max_errors: usize,
    extra_roots: &[std::path::PathBuf],
    show_stats: bool,
    trace_aether: bool,
    strict_mode: bool,
    diagnostics_format: DiagnosticOutputFormat,
    all_errors: bool,
    dump_core: CoreDumpMode,
    dump_aether: AetherDumpMode,
    dump_lir: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] dump_lir_llvm: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] use_core_to_llvm: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] emit_llvm: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] emit_binary: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] output_path: Option<String>,
) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let source_hash = hash_bytes(source.as_bytes());
            let entry_path = Path::new(path);
            let roots = collect_roots(entry_path, extra_roots, roots_only);
            let roots_hash = roots_cache_hash(&roots);
            let base_cache_key = hash_cache_key(&source_hash, &roots_hash);
            let strict_hash = hash_bytes(if strict_mode {
                b"strict=1"
            } else {
                b"strict=0"
            });
            let cache_key = hash_cache_key(&base_cache_key, &strict_hash);
            let cache = BytecodeCache::new(Path::new("target").join("flux"));
            if !no_cache
                && !use_core_to_llvm
                && !emit_llvm
                && !emit_binary
                && matches!(dump_core, CoreDumpMode::None)
                && dump_aether == AetherDumpMode::None
                && !dump_lir
                && !dump_lir_llvm
                && !trace_aether
            {
                if let Some(bytecode) =
                    cache.load(Path::new(path), &cache_key, env!("CARGO_PKG_VERSION"))
                {
                    if verbose {
                        eprintln!("cache: hit (bytecode loaded)");
                    }
                    let functions_count = count_bytecode_functions(&bytecode.constants);
                    let instruction_bytes = bytecode.instructions.len();
                    let mut vm = VM::new(bytecode);
                    vm.set_trace(trace);
                    let exec_start = Instant::now();
                    if let Err(err) = vm.run() {
                        eprintln!("{}", err);
                        std::process::exit(1);
                    }
                    let execute_ms = exec_start.elapsed().as_secs_f64() * 1000.0;
                    if leak_detector {
                        print_leak_stats();
                    }
                    if show_stats {
                        print_stats(&RunStats {
                            parse_ms: None,
                            compile_ms: None,
                            compile_backend: Some("bytecode"),
                            execute_ms,
                            execute_backend: "vm",
                            cached: true,
                            module_count: None,
                            source_lines: source.lines().count(),
                            globals_count: None,
                            functions_count: Some(functions_count),
                            instruction_bytes: Some(instruction_bytes),
                        });
                    }
                    return;
                }
                if verbose {
                    let reason = cache
                        .load_failure_reason(Path::new(path), &cache_key, env!("CARGO_PKG_VERSION"))
                        .unwrap_or("cache file not found");
                    eprintln!("cache: miss (compiling: {reason})");
                }
            }

            let parse_start = Instant::now();
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();

            // --- Collect all diagnostics into a single pool ---
            let mut all_diagnostics: Vec<Diagnostic> = Vec::new();
            let mut parse_warnings = parser.take_warnings();
            tag_diagnostics(&mut parse_warnings, DiagnosticPhase::Parse);
            for diag in &mut parse_warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }
            all_diagnostics.append(&mut parse_warnings);

            // Entry file parse errors: collect but do NOT exit early.
            let entry_has_errors = !parser.errors.is_empty();
            if entry_has_errors {
                tag_diagnostics(&mut parser.errors, DiagnosticPhase::Parse);
                for diag in &mut parser.errors {
                    if diag.file().is_none() {
                        diag.set_file(path.to_string());
                    }
                }
                all_diagnostics.append(&mut parser.errors);
            }

            // Auto-import Flow library modules (Proposal 0120/0121 Phase 4).
            // Dump/analysis surfaces should see the same enriched program that
            // normal compilation executes, otherwise `--dump-core` and related
            // commands become semantically inconsistent with real runs.
            // Only skip the injection for `--trace-aether`, which is intended
            // to show the direct execution path without extra dump-only noise.
            let mut program = program;
            if !trace_aether {
                inject_flow_prelude(&mut program, &mut parser, use_core_to_llvm);
            }

            let interner = parser.take_interner();
            let entry_path = Path::new(path);
            let roots = collect_roots(entry_path, extra_roots, roots_only);

            // --- Build module graph (always returns, may have diagnostics) ---
            let graph_result =
                ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, &roots);
            let parse_ms = parse_start.elapsed().as_secs_f64() * 1000.0;
            let mut graph_diags = graph_result.diagnostics;
            tag_diagnostics(&mut graph_diags, DiagnosticPhase::ModuleGraph);
            all_diagnostics.extend(graph_diags);

            // Track all failed modules (parse + validation failures from graph).
            let mut failed: HashSet<PathBuf> = graph_result.failed_modules;
            if entry_has_errors && let Ok(canon) = std::fs::canonicalize(entry_path) {
                failed.insert(canon);
            }

            let module_count = graph_result.graph.module_count();
            let is_multimodule = module_count > 1;
            let graph = graph_result.graph;

            // --- Compile valid modules, suppress cascade ---
            let compile_start = Instant::now();
            let mut compiler = Compiler::new_with_interner(path, graph_result.interner);
            compiler.set_strict_mode(strict_mode);
            let entry_canonical = std::fs::canonicalize(entry_path).ok();
            let mut preloaded_interfaces: HashSet<PathBuf> = HashSet::new();
            let module_cache = ModuleBytecodeCache::new(Path::new("target").join("flux"));
            let allow_cached_module_bytecode = !use_core_to_llvm
                && !emit_llvm
                && !emit_binary
                && matches!(dump_core, CoreDumpMode::None)
                && dump_aether == AetherDumpMode::None
                && !dump_lir
                && !dump_lir_llvm
                && !trace_aether;

            // Sort topo_order to compile Flow library modules first.
            // This ensures all modules can access Flow functions (map, filter, etc.)
            // without explicit imports — like Haskell's implicit Prelude.
            let mut ordered_nodes = graph.topo_order();
            ordered_nodes.sort_by_key(|node| {
                let is_flow = node.path.to_string_lossy().contains("lib/Flow/")
                    || node.path.to_string_lossy().contains("lib\\Flow\\");
                if is_flow { 0 } else { 1 }
            });

            for node in ordered_nodes {
                // Skip entry if it had parse errors (it is in topo_order but
                // should not be compiled).
                if entry_has_errors
                    && let Some(ref canon) = entry_canonical
                    && &node.path == canon
                {
                    continue;
                }

                // Cascade suppression: skip if any dependency failed.
                let failed_dep = node
                    .imports
                    .iter()
                    .find(|e| failed.contains(&e.target_path));
                if let Some(dep) = failed_dep {
                    failed.insert(node.path.clone());
                    let display = render_display_path(&node.path.to_string_lossy()).into_owned();
                    all_diagnostics.push(module_skipped_note(
                        display.clone(),
                        display,
                        dep.name.clone(),
                    ));
                    continue;
                }

                if !no_cache {
                    for dep in &node.imports {
                        if !preloaded_interfaces.insert(dep.target_path.clone()) {
                            continue;
                        }
                        let Ok(dep_source) = std::fs::read_to_string(&dep.target_path) else {
                            continue;
                        };
                        let Some(interface) =
                            flux::bytecode::compiler::module_interface::load_valid_interface(
                                &dep.target_path,
                                &dep_source,
                            )
                        else {
                            continue;
                        };
                        compiler.preload_module_interface(&interface);
                        if verbose {
                            eprintln!(
                                "interface: loaded {} from {}",
                                interface.module_name,
                                dep.target_path.display()
                            );
                        }
                    }
                }

                compiler.set_file_path(node.path.to_string_lossy().to_string());
                let is_entry_module = entry_canonical.as_ref().is_some_and(|p| p == &node.path);
                let is_flow_library = node.path.to_string_lossy().contains("lib/Flow/")
                    || node.path.to_string_lossy().contains("lib\\Flow\\");
                let module_source = std::fs::read_to_string(&node.path).unwrap_or_default();
                let module_source_hash = hash_bytes(module_source.as_bytes());
                let module_strict_hash = if is_flow_library {
                    hash_bytes(b"strict=0")
                } else {
                    strict_hash
                };
                let module_cache_key = hash_cache_key(&module_source_hash, &module_strict_hash);
                let module_deps: Vec<(String, [u8; 32])> = node
                    .imports
                    .iter()
                    .filter_map(|dep| {
                        hash_file(&dep.target_path)
                            .ok()
                            .map(|hash| (dep.target_path.to_string_lossy().to_string(), hash))
                    })
                    .collect();

                if !no_cache
                    && allow_cached_module_bytecode
                    && !is_entry_module
                    && let Some(cached) =
                        module_cache.load(&node.path, &module_cache_key, env!("CARGO_PKG_VERSION"))
                {
                    compiler.hydrate_cached_module_bytecode(&cached);
                    if verbose {
                        eprintln!("module-cache: hit ({})", node.path.display());
                    }
                    continue;
                }
                compiler.set_strict_require_main(is_entry_module);
                // Disable strict mode for Flow library modules — they use
                // polymorphic signatures that strict mode can't validate yet.
                if is_flow_library {
                    compiler.set_strict_mode(false);
                }
                let module_snapshot = compiler.module_cache_snapshot();
                let compile_result =
                    compiler.compile_with_opts(&node.program, enable_optimize, enable_analyze);
                if is_flow_library {
                    compiler.set_strict_mode(strict_mode);
                }
                let mut compiler_warnings = compiler.take_warnings();
                tag_diagnostics(&mut compiler_warnings, DiagnosticPhase::Validation);
                for diag in &mut compiler_warnings {
                    if diag.file().is_none() {
                        diag.set_file(node.path.to_string_lossy().to_string());
                    }
                }
                all_diagnostics.append(&mut compiler_warnings);

                if let Err(mut diags) = compile_result {
                    tag_diagnostics(&mut diags, DiagnosticPhase::TypeCheck);
                    for diag in &mut diags {
                        if diag.file().is_none() {
                            diag.set_file(node.path.to_string_lossy().to_string());
                        }
                    }
                    all_diagnostics.append(&mut diags);
                    continue;
                }

                if !no_cache && allow_cached_module_bytecode && !is_entry_module {
                    let cached_module = compiler.build_cached_module_bytecode(module_snapshot);
                    if let Err(e) = module_cache.store(
                        &node.path,
                        &module_cache_key,
                        env!("CARGO_PKG_VERSION"),
                        &cached_module,
                        &module_deps,
                    ) && verbose
                    {
                        eprintln!(
                            "warning: could not write module cache file for {}: {e}",
                            node.path.display()
                        );
                    }
                }

                // Save module interface (.flxi) for non-entry modules.
                // Entry module doesn't need an interface — it's the consumer.
                if !no_cache
                    && !is_entry_module
                    && let Some((module_name, module_sym)) =
                        extract_module_name_and_sym(&node.program, &compiler.interner)
                {
                    match compiler.lower_aether_report_program(&node.program, enable_optimize) {
                        Ok(core) => {
                            let interface =
                                flux::bytecode::compiler::module_interface::build_interface(
                                    &module_name,
                                    module_sym,
                                    &module_source_hash,
                                    &core,
                                    compiler.cached_member_schemes(),
                                    &compiler.module_function_visibility,
                                    &compiler.interner,
                                );
                            compiler.preload_module_interface(&interface);
                            let iface_path =
                                flux::bytecode::compiler::module_interface::interface_path(
                                    &node.path,
                                );
                            if let Err(e) =
                                flux::bytecode::compiler::module_interface::save_interface(
                                    &iface_path,
                                    &interface,
                                )
                                && verbose
                            {
                                eprintln!(
                                    "warning: could not write interface file {}: {e}",
                                    iface_path.display()
                                );
                            }
                        }
                        Err(e) if verbose => {
                            eprintln!(
                                "warning: could not build interface for {}: {}",
                                node.path.display(),
                                e.message().unwrap_or("unknown Core lowering error")
                            );
                        }
                        Err(_) => {}
                    }
                }
            }

            // --- One unified report ---
            if !all_diagnostics.is_empty() {
                let report = DiagnosticsAggregator::new(&all_diagnostics)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(is_multimodule)
                    .with_max_errors(Some(max_errors))
                    .with_stage_filtering(!all_errors)
                    .report();
                if report.counts.errors > 0 {
                    emit_diagnostics(
                        &all_diagnostics,
                        Some(path),
                        Some(source.as_str()),
                        is_multimodule,
                        max_errors,
                        diagnostics_format,
                        all_errors,
                        true,
                    );
                    std::process::exit(1);
                }
                emit_diagnostics(
                    &all_diagnostics,
                    Some(path),
                    Some(source.as_str()),
                    is_multimodule,
                    max_errors,
                    diagnostics_format,
                    all_errors,
                    true,
                );
            }

            // Build merged program from all modules for dump/analysis surfaces that
            // need whole-program visibility.
            let merged_program = if is_multimodule
                && (dump_aether != AetherDumpMode::None
                    || !matches!(dump_core, CoreDumpMode::None)
                    || dump_lir
                    || dump_lir_llvm)
            {
                let mut merged = Program::new();
                for node in graph.topo_order() {
                    merged.statements.extend(node.program.statements.clone());
                }
                merged
            } else {
                program.clone()
            };

            if dump_aether != AetherDumpMode::None {
                match compiler.dump_aether_report(
                    &merged_program,
                    enable_optimize,
                    dump_aether == AetherDumpMode::Debug,
                ) {
                    Ok(report) => println!("{report}"),
                    Err(diag) => {
                        emit_diagnostics(
                            &[diag],
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                }
                return;
            }

            if !matches!(dump_core, CoreDumpMode::None) {
                let dumped = compiler.dump_core_with_opts(
                    &merged_program,
                    enable_optimize,
                    match dump_core {
                        CoreDumpMode::Readable => flux::core::display::CoreDisplayMode::Readable,
                        CoreDumpMode::Debug => flux::core::display::CoreDisplayMode::Debug,
                        CoreDumpMode::None => unreachable!("checked above"),
                    },
                );
                match dumped {
                    Ok(dumped) => println!("{dumped}"),
                    Err(diag) => {
                        emit_diagnostics(
                            &[diag],
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                }
                return;
            }

            if dump_lir {
                let dumped = compiler.dump_lir(&merged_program, enable_optimize);
                match dumped {
                    Ok(dumped) => println!("{dumped}"),
                    Err(diag) => {
                        emit_diagnostics(
                            &[diag],
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                }
                return;
            }

            // --- LIR → LLVM IR dump (Proposal 0132 Phase 7) ---
            #[cfg(feature = "core_to_llvm")]
            if dump_lir_llvm {
                match compiler.dump_lir_llvm(&merged_program, enable_optimize) {
                    Ok(ir_text) => println!("{ir_text}"),
                    Err(diag) => {
                        emit_diagnostics(
                            &[diag],
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                }
                return;
            }

            // --- LIR → LLVM native execution path (Proposal 0132) ---
            #[cfg(feature = "core_to_llvm")]
            if use_core_to_llvm || emit_llvm || emit_binary {
                // Build merged program from all modules.
                let mut native_program = Program::new();
                for node in graph.topo_order() {
                    native_program
                        .statements
                        .extend(node.program.statements.clone());
                }

                // Re-run HM type inference on the merged program so all
                // modules' types are available for Core IR lowering.
                compiler.infer_expr_types_for_program(&native_program);

                // AST → Core IR → Aether → LIR → LLVM IR module.
                eprintln!("[lir→llvm] Compiling via LIR → LLVM native backend...");
                let mut llvm_module =
                    match compiler.lower_to_lir_llvm_module(&native_program, enable_optimize) {
                        Ok(m) => m,
                        Err(diag) => {
                            emit_diagnostics(
                                &[diag],
                                Some(path),
                                Some(source.as_str()),
                                is_multimodule,
                                max_errors,
                                diagnostics_format,
                                all_errors,
                                true,
                            );
                            std::process::exit(1);
                        }
                    };

                // Inject target triple and data layout.
                llvm_module.target_triple = Some(flux::core_to_llvm::target::host_triple());
                llvm_module.data_layout = flux::core_to_llvm::target::host_data_layout();

                let ll_text = flux::core_to_llvm::render_module(&llvm_module);

                if emit_llvm {
                    if let Some(ref out) = output_path {
                        if let Err(e) = std::fs::write(out, &ll_text) {
                            eprintln!("Failed to write LLVM IR: {e}");
                            std::process::exit(1);
                        }
                        println!("Emitted LLVM IR: {out}");
                    } else {
                        println!("{ll_text}");
                    }
                    return;
                }

                if emit_binary {
                    let runtime_lib_dir = locate_runtime_lib_dir();
                    let out = output_path
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| {
                            std::path::PathBuf::from(path.strip_suffix(".flx").unwrap_or(path))
                        });
                    let config = flux::core_to_llvm::pipeline::PipelineConfig {
                        ll_text,
                        opt_level: if enable_optimize { 2 } else { 0 },
                        output_path: Some(out.clone()),
                        runtime_lib_dir,
                    };
                    match flux::core_to_llvm::pipeline::compile_to_binary(&config) {
                        Ok(flux::core_to_llvm::pipeline::PipelineResult::EmittedBinary {
                            path: bin_path,
                        }) => {
                            println!("Emitted binary: {}", bin_path.display());
                        }
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("core_to_llvm pipeline failed: {e}");
                            std::process::exit(1);
                        }
                    }
                    return;
                }

                // Default: compile and run.
                let runtime_lib_dir = locate_runtime_lib_dir();
                let config = flux::core_to_llvm::pipeline::PipelineConfig {
                    ll_text,
                    opt_level: if enable_optimize { 2 } else { 0 },
                    output_path: None,
                    runtime_lib_dir,
                };
                let exec_start = Instant::now();
                match flux::core_to_llvm::pipeline::compile_and_run(&config) {
                    Ok(flux::core_to_llvm::pipeline::PipelineResult::Executed { exit_code }) => {
                        let execute_ms = exec_start.elapsed().as_secs_f64() * 1000.0;
                        if show_stats {
                            let compile_ms =
                                compile_start.elapsed().as_secs_f64() * 1000.0 - execute_ms;
                            print_stats(&RunStats {
                                parse_ms: Some(parse_ms),
                                compile_ms: Some(compile_ms),
                                compile_backend: Some("llvm"),
                                execute_ms,
                                execute_backend: "native",
                                cached: false,
                                module_count: Some(module_count),
                                source_lines: source.lines().count(),
                                globals_count: None,
                                functions_count: None,
                                instruction_bytes: None,
                            });
                        }
                        if exit_code != 0 {
                            std::process::exit(exit_code);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("core_to_llvm execution failed: {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }

            let compile_ms = compile_start.elapsed().as_secs_f64() * 1000.0;
            let bytecode = compiler.bytecode();
            let globals_count = compiler.symbol_table.num_definitions;
            let functions_count = count_bytecode_functions(&bytecode.constants);
            let instruction_bytes = bytecode.instructions.len();
            if trace_aether {
                match compiler.render_aether_report(&program, enable_optimize, false) {
                    Ok(report) => print_aether_trace(
                        path,
                        TraceBackend::Vm,
                        "AST -> Core -> CFG -> bytecode -> VM",
                        Some("disabled"),
                        enable_optimize,
                        enable_analyze,
                        strict_mode,
                        Some(module_count),
                        &report,
                    ),
                    Err(diag) => {
                        emit_diagnostics(
                            &[diag],
                            Some(path),
                            Some(source.as_str()),
                            is_multimodule,
                            max_errors,
                            diagnostics_format,
                            all_errors,
                            true,
                        );
                        std::process::exit(1);
                    }
                }
            }

            let mut deps = Vec::new();
            for dep in graph.imported_files() {
                if let Ok(hash) = hash_file(Path::new(&dep)) {
                    deps.push((dep, hash));
                }
            }
            if !no_cache {
                let stored = cache
                    .store(
                        Path::new(path),
                        &cache_key,
                        env!("CARGO_PKG_VERSION"),
                        &bytecode,
                        &deps,
                    )
                    .is_ok();
                if verbose && stored {
                    eprintln!("cache: stored");
                }
            }

            eprintln!("[cfg→vm] Running via CFG → bytecode VM backend...");
            let mut vm = VM::new(bytecode);
            vm.set_trace(trace);
            let exec_start = Instant::now();
            if let Err(err) = vm.run() {
                eprintln!("{}", err);
                std::process::exit(1);
            }
            let execute_ms = exec_start.elapsed().as_secs_f64() * 1000.0;
            if leak_detector {
                print_leak_stats();
            }
            if show_stats {
                print_stats(&RunStats {
                    parse_ms: Some(parse_ms),
                    compile_ms: Some(compile_ms),
                    compile_backend: Some("bytecode"),
                    execute_ms,
                    execute_backend: "vm",
                    cached: false,
                    module_count: Some(module_count),
                    source_lines: source.lines().count(),
                    globals_count: Some(globals_count),
                    functions_count: Some(functions_count),
                    instruction_bytes: Some(instruction_bytes),
                });
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

// ─── Test Runner ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn run_test_file(
    path: &str,
    roots_only: bool,
    enable_optimize: bool,
    enable_analyze: bool,
    max_errors: usize,
    extra_roots: &[std::path::PathBuf],
    test_filter: Option<&str>,
    strict_mode: bool,
    diagnostics_format: DiagnosticOutputFormat,
    all_errors: bool,
    #[cfg_attr(not(feature = "core_to_llvm"), allow(unused))] use_core_to_llvm: bool,
) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            std::process::exit(1);
        }
    };

    let entry_path = Path::new(path);
    let roots = collect_roots(entry_path, extra_roots, roots_only);

    // --- Parse ---
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let mut program = parser.parse_program();

    let mut all_diagnostics: Vec<Diagnostic> = Vec::new();
    let mut parse_warnings = parser.take_warnings();
    tag_diagnostics(&mut parse_warnings, DiagnosticPhase::Parse);
    for diag in &mut parse_warnings {
        if diag.file().is_none() {
            diag.set_file(path.to_string());
        }
    }
    all_diagnostics.append(&mut parse_warnings);

    if !parser.errors.is_empty() {
        tag_diagnostics(&mut parser.errors, DiagnosticPhase::Parse);
        for diag in &mut parser.errors {
            if diag.file().is_none() {
                diag.set_file(path.to_string());
            }
        }
        emit_diagnostics(
            &parser.errors,
            Some(path),
            Some(source.as_str()),
            false,
            max_errors,
            diagnostics_format,
            all_errors,
            true,
        );
        std::process::exit(1);
    }

    // Auto-import Flow library for test mode too.
    inject_flow_prelude(&mut program, &mut parser, use_core_to_llvm);

    let interner = parser.take_interner();

    // --- Build module graph ---
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, &roots);
    let mut graph_diags = graph_result.diagnostics;
    tag_diagnostics(&mut graph_diags, DiagnosticPhase::ModuleGraph);
    all_diagnostics.extend(graph_diags);

    let failed = graph_result.failed_modules;
    let module_count = graph_result.graph.module_count();
    let is_multimodule = module_count > 1;
    let graph = graph_result.graph;

    // --- Compile ---
    let mut compiler = Compiler::new_with_interner(path, graph_result.interner);
    compiler.set_strict_mode(strict_mode);
    let entry_canonical = std::fs::canonicalize(entry_path).ok();
    for node in graph.topo_order() {
        if node.imports.iter().any(|e| failed.contains(&e.target_path)) {
            continue;
        }
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        let is_entry_module = entry_canonical.as_ref().is_some_and(|p| p == &node.path);
        let is_flow_library = node.path.to_string_lossy().contains("lib/Flow/")
            || node.path.to_string_lossy().contains("lib\\Flow\\");
        compiler.set_strict_require_main(is_entry_module);
        if is_flow_library {
            compiler.set_strict_mode(false);
        }
        let compile_result =
            compiler.compile_with_opts(&node.program, enable_optimize, enable_analyze);
        if is_flow_library {
            compiler.set_strict_mode(strict_mode);
        }
        let mut compiler_warnings = compiler.take_warnings();
        tag_diagnostics(&mut compiler_warnings, DiagnosticPhase::Validation);
        for diag in &mut compiler_warnings {
            if diag.file().is_none() {
                diag.set_file(node.path.to_string_lossy().to_string());
            }
        }
        all_diagnostics.append(&mut compiler_warnings);

        if let Err(mut diags) = compile_result {
            tag_diagnostics(&mut diags, DiagnosticPhase::TypeCheck);
            for diag in &mut diags {
                if diag.file().is_none() {
                    diag.set_file(node.path.to_string_lossy().to_string());
                }
            }
            all_diagnostics.append(&mut diags);
        }
    }

    if !all_diagnostics.is_empty() {
        let report = DiagnosticsAggregator::new(&all_diagnostics)
            .with_default_source(path, source.as_str())
            .with_file_headers(is_multimodule)
            .with_max_errors(Some(max_errors))
            .with_stage_filtering(!all_errors)
            .report();
        if report.counts.errors > 0 {
            emit_diagnostics(
                &all_diagnostics,
                Some(path),
                Some(source.as_str()),
                is_multimodule,
                max_errors,
                diagnostics_format,
                all_errors,
                true,
            );
            std::process::exit(1);
        }
        emit_diagnostics(
            &all_diagnostics,
            Some(path),
            Some(source.as_str()),
            is_multimodule,
            max_errors,
            diagnostics_format,
            all_errors,
            true,
        );
    }

    // --- Collect test functions ---
    let mut tests = collect_test_functions(&compiler.symbol_table, &compiler.interner);
    if let Some(filter) = test_filter {
        tests.retain(|(name, _)| name.contains(filter));
    }

    if tests.is_empty() {
        let file_name = entry_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path);
        println!("Running tests in {}\n", file_name);
        if let Some(filter) = test_filter {
            println!("No test functions found matching filter `{}`.", filter);
        } else {
            println!("No test functions found (define functions named `test_*`).");
        }
        return;
    }

    // --- Run tests ---
    let file_name = entry_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);

    #[cfg(feature = "native")]
    let all_passed = if use_core_to_llvm {
        run_tests_native(
            file_name,
            path,
            &source,
            &roots,
            &tests,
            enable_optimize,
            strict_mode,
        )
    } else {
        let bytecode = compiler.bytecode();
        let mut vm = VM::new(bytecode);
        if let Err(err) = vm.run() {
            eprintln!("Error during test setup: {}", err);
            std::process::exit(1);
        }

        let results = run_tests(&mut vm, tests);
        print_test_report(file_name, &results)
    };

    #[cfg(not(feature = "native"))]
    let all_passed = {
        let bytecode = compiler.bytecode();
        let mut vm = VM::new(bytecode);
        if let Err(err) = vm.run() {
            eprintln!("Error during test setup: {}", err);
            std::process::exit(1);
        }

        let results = run_tests(&mut vm, tests);
        print_test_report(file_name, &results)
    };

    if !all_passed {
        std::process::exit(1);
    }
}

#[cfg(feature = "native")]
fn run_tests_native(
    file_name: &str,
    source_path: &str,
    source: &str,
    roots: &[PathBuf],
    tests: &[(String, usize)],
    enable_optimize: bool,
    strict_mode: bool,
) -> bool {
    use flux::bytecode::vm::test_runner::{TestOutcome, TestResult, print_test_report};
    use std::fs;
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

    for (idx, (name, _)) in tests.iter().enumerate() {
        let harness_path = std::env::temp_dir().join(format!(
            "flux_native_test_{}_{}_{}.flx",
            std::process::id(),
            unique,
            idx
        ));
        let harness_source = format!("{source}\n\nfn main() {{ {name}(); }}\n");
        if let Err(e) = fs::write(&harness_path, harness_source) {
            eprintln!(
                "Failed to write native test harness {}: {e}",
                harness_path.display()
            );
            std::process::exit(1);
        }

        let start = Instant::now();
        let mut cmd = Command::new(&exe);
        cmd.arg("--native").arg("--no-cache");
        if enable_optimize {
            cmd.arg("--optimize");
        }
        if strict_mode {
            cmd.arg("--strict");
        }
        for root in roots {
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
                name, source_path, err
            )),
        };

        let _ = fs::remove_file(&harness_path);
        results.push(TestResult {
            name: name.clone(),
            elapsed_ms,
            outcome,
        });
    }

    print_test_report(file_name, &results)
}

// ─── Analytics ───────────────────────────────────────────────────────────────

struct RunStats {
    parse_ms: Option<f64>,
    compile_ms: Option<f64>,
    compile_backend: Option<&'static str>,
    execute_ms: f64,
    execute_backend: &'static str,
    cached: bool,
    module_count: Option<usize>,
    source_lines: usize,
    globals_count: Option<usize>,
    functions_count: Option<usize>,
    instruction_bytes: Option<usize>,
}

#[allow(clippy::too_many_arguments)]
fn print_aether_trace(
    path: &str,
    backend: TraceBackend,
    pipeline: &str,
    cache: Option<&str>,
    optimize: bool,
    analyze: bool,
    strict: bool,
    module_count: Option<usize>,
    report: &str,
) {
    let backend_name = match backend {
        TraceBackend::Vm => "vm",
    };

    eprintln!();
    eprintln!("── Aether Trace ──");
    eprintln!("file: {}", path);
    eprintln!("backend: {}", backend_name);
    eprintln!("pipeline: {}", pipeline);
    if let Some(cache_mode) = cache {
        eprintln!("cache: {}", cache_mode);
    }
    eprintln!("optimize: {}", if optimize { "on" } else { "off" });
    eprintln!("analyze: {}", if analyze { "on" } else { "off" });
    eprintln!("strict: {}", if strict { "on" } else { "off" });
    if let Some(count) = module_count {
        eprintln!("modules: {}", count);
    }
    eprintln!("────────────────────────");
    eprintln!("{report}");
}

fn count_bytecode_functions(constants: &[flux::runtime::value::Value]) -> usize {
    use flux::runtime::value::Value;
    constants
        .iter()
        .filter(|v| matches!(v, Value::Function(_)))
        .count()
}

fn print_stats(stats: &RunStats) {
    let total_ms =
        stats.parse_ms.unwrap_or(0.0) + stats.compile_ms.unwrap_or(0.0) + stats.execute_ms;

    let w = 46usize;
    eprintln!();
    eprintln!("  ── Flux Analytics {}", "─".repeat(w - 19));

    if let Some(ms) = stats.parse_ms {
        eprintln!("  {:<20} {:>8.2} ms", "parse", ms);
    }

    if stats.cached {
        eprintln!("  {:<20} {:>12}", "compile", "(cached)");
    } else if let Some(ms) = stats.compile_ms {
        eprintln!(
            "  {:<20} {:>8.2} ms  [{}]",
            "compile",
            ms,
            stats.compile_backend.unwrap_or("unknown")
        );
    }

    eprintln!(
        "  {:<20} {:>8.2} ms  [{}]",
        "execute", stats.execute_ms, stats.execute_backend
    );
    eprintln!("  {:<20} {:>8.2} ms", "total", total_ms);
    eprintln!();

    if let Some(n) = stats.module_count {
        eprintln!("  {:<20} {:>8}", "modules", n);
    }
    eprintln!("  {:<20} {:>8}", "source lines", stats.source_lines);
    if let Some(n) = stats.globals_count {
        eprintln!("  {:<20} {:>8}", "globals", n);
    }
    if let Some(n) = stats.functions_count {
        eprintln!("  {:<20} {:>8}", "functions", n);
    }
    if let Some(n) = stats.instruction_bytes {
        eprintln!("  {:<20} {:>8} bytes", "instructions", n);
    }
    eprintln!("  {}", "─".repeat(w - 2));
}

fn print_leak_stats() {
    let stats = flux::runtime::leak_detector::snapshot();
    println!(
        "\nLeak stats (approx):\n  compiled_functions: {}\n  closures: {}\n  arrays: {}\n  hashes: {}\n  somes: {}",
        stats.compiled_functions, stats.closures, stats.arrays, stats.hashes, stats.somes
    );
}

/// Locate the Flux C runtime library directory (`runtime/c/`).
///
/// Searches relative to the running executable and common development paths.
/// Returns `None` if not found (linker will search system paths).
#[cfg(feature = "native")]
fn locate_runtime_lib_dir() -> Option<std::path::PathBuf> {
    // Find the runtime/c source directory relative to the executable or cwd.
    let candidates = {
        let mut v = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            let mut dir = exe.parent().map(Path::to_path_buf);
            for _ in 0..5 {
                if let Some(ref d) = dir {
                    v.push(d.join("runtime").join("c"));
                    dir = d.parent().map(Path::to_path_buf);
                }
            }
        }
        v.push(std::path::PathBuf::from("runtime/c"));
        v
    };

    for candidate in &candidates {
        // Check if source directory exists (has flux_rt.h).
        if candidate.join("flux_rt.h").exists() {
            // Auto-build libflux_rt.a if missing or stale.
            #[cfg(feature = "native")]
            if let Err(e) = flux::core_to_llvm::pipeline::ensure_runtime_lib(candidate) {
                eprintln!("Warning: failed to build C runtime: {e}");
            }
            let lib_exists = if cfg!(windows) {
                candidate.join("flux_rt.lib").exists()
            } else {
                candidate.join("libflux_rt.a").exists()
            };
            if lib_exists {
                return Some(candidate.clone());
            }
        }
    }
    None
}

fn extract_output_path(args: &mut Vec<String>) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-o" {
            args.remove(i);
            if i < args.len() {
                return Some(args.remove(i));
            }
            eprintln!("Error: -o requires an output path argument.");
            return None;
        }
        i += 1;
    }
    None
}

fn extract_dump_core_mode(args: &mut Vec<String>) -> Option<CoreDumpMode> {
    let mut mode = CoreDumpMode::None;
    let mut i = 0;
    while i < args.len() {
        let next_mode = if args[i] == "--dump-core" {
            args.remove(i);
            CoreDumpMode::Readable
        } else if let Some(value) = args[i].strip_prefix("--dump-core=") {
            let parsed = match value {
                "debug" => CoreDumpMode::Debug,
                "" => {
                    eprintln!("Error: --dump-core expects no value or `debug`.");
                    return None;
                }
                _ => {
                    eprintln!("Error: --dump-core expects no value or `debug`.");
                    return None;
                }
            };
            args.remove(i);
            parsed
        } else {
            i += 1;
            continue;
        };
        mode = next_mode;
    }
    Some(mode)
}

fn extract_diagnostic_format(args: &mut Vec<String>) -> Option<DiagnosticOutputFormat> {
    let mut format = DiagnosticOutputFormat::Text;
    let mut i = 0;
    while i < args.len() {
        let value = if args[i] == "--format" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --format <text|json|json-compact>");
                return None;
            }
            let v = args.remove(i + 1);
            args.remove(i);
            v
        } else if let Some(v) = args[i].strip_prefix("--format=") {
            let v = v.to_string();
            args.remove(i);
            v
        } else {
            i += 1;
            continue;
        };

        format = match value.as_str() {
            "text" => DiagnosticOutputFormat::Text,
            "json" => DiagnosticOutputFormat::Json,
            "json-compact" => DiagnosticOutputFormat::JsonCompact,
            _ => {
                eprintln!("Error: --format expects one of: text, json, json-compact.");
                return None;
            }
        };
    }
    Some(format)
}

fn extract_max_errors(args: &mut Vec<String>) -> Option<usize> {
    let mut max_errors = DEFAULT_MAX_ERRORS;
    let mut i = 0;
    while i < args.len() {
        let value_str = if args[i] == "--max-errors" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --max-errors <n>");
                return None;
            }
            let v = args.remove(i + 1);
            args.remove(i);
            v
        } else if let Some(v) = args[i].strip_prefix("--max-errors=") {
            let v = v.to_string();
            args.remove(i);
            v
        } else {
            i += 1;
            continue;
        };
        match value_str.parse::<usize>() {
            Ok(parsed) => max_errors = parsed,
            Err(_) => {
                eprintln!("Error: --max-errors expects a non-negative integer.");
                return None;
            }
        }
    }
    Some(max_errors)
}

fn tag_diagnostics(diags: &mut [Diagnostic], phase: DiagnosticPhase) {
    for diag in diags {
        if diag.phase().is_none() {
            *diag = diag.clone().with_phase(phase);
        }
    }
}

fn should_show_file_headers(diagnostics: &[Diagnostic], requested: bool) -> bool {
    if requested {
        return true;
    }

    let mut files = std::collections::BTreeSet::new();
    for diag in diagnostics {
        if let Some(file) = diag.file() {
            files.insert(file);
            if files.len() > 1 {
                return true;
            }
        }
    }

    false
}

#[allow(clippy::too_many_arguments)]
fn emit_diagnostics(
    diagnostics: &[Diagnostic],
    default_file: Option<&str>,
    default_source: Option<&str>,
    show_file_headers: bool,
    max_errors: usize,
    format: DiagnosticOutputFormat,
    all_errors: bool,
    text_to_stderr: bool,
) {
    let show_file_headers = should_show_file_headers(diagnostics, show_file_headers);
    let mut agg = DiagnosticsAggregator::new(diagnostics)
        .with_file_headers(show_file_headers)
        .with_max_errors(Some(max_errors))
        .with_stage_filtering(!all_errors);
    if let Some(file) = default_file {
        if let Some(source) = default_source {
            agg = agg.with_default_source(file.to_string(), source.to_string());
        } else {
            agg = agg.with_default_file(file.to_string());
        }
    }

    match format {
        DiagnosticOutputFormat::Text => {
            let rendered = agg.report().rendered;
            if text_to_stderr {
                eprintln!("{}", rendered);
            } else {
                println!("{}", rendered);
            }
        }
        DiagnosticOutputFormat::Json => {
            let rendered = render_diagnostics_json(
                diagnostics,
                default_file,
                Some(max_errors),
                !all_errors,
                true,
            );
            eprintln!("{}", rendered);
        }
        DiagnosticOutputFormat::JsonCompact => {
            let rendered = render_diagnostics_json(
                diagnostics,
                default_file,
                Some(max_errors),
                !all_errors,
                false,
            );
            eprintln!("{}", rendered);
        }
    }
}

/// Inject auto-imports for Flow library modules into the program AST.
///
/// Currently injects: `import Flow.Option exposing (..)`
///
/// Uses a mini-parser to parse the synthetic import so symbols are
/// correctly interned in the same interner used for the rest of compilation.
/// Flow library modules to auto-import.  Each entry is
/// `(module_name, file_name)` — the file is checked for existence before
/// injecting `import <module_name> exposing (..)`.
const FLOW_PRELUDE_MODULES: &[(&str, &str)] = &[
    ("Flow.Option", "Option.flx"),
    ("Flow.List", "List.flx"),
    ("Flow.String", "String.flx"),
    ("Flow.Numeric", "Numeric.flx"),
    ("Flow.IO", "IO.flx"),
    ("Flow.Assert", "Assert.flx"),
];

fn inject_flow_prelude(
    program: &mut Program,
    parser: &mut flux::syntax::parser::Parser,
    native_mode: bool,
) {
    let flow_dir = Path::new("lib").join("Flow");
    if !flow_dir.exists() {
        return;
    }

    // Both VM and native backends inject all Flow modules.
    // The native/LLVM backend compiles lib/Flow/*.flx through Core IR
    // alongside user code, enabling cross-module inlining.
    let _ = native_mode; // used by both paths now
    let modules: &[(&str, &str)] = FLOW_PRELUDE_MODULES;

    // Collect the set of already-imported Flow modules.
    let interner = parser.interner();
    let existing_imports: Vec<String> = program
        .statements
        .iter()
        .filter_map(|stmt| {
            if let flux::syntax::statement::Statement::Import { name, .. } = stmt {
                interner.try_resolve(*name).map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    // Build the synthetic import source for all missing modules.
    let mut imports = Vec::new();
    for &(module_name, file_name) in modules {
        if existing_imports.iter().any(|s| s == module_name) {
            continue;
        }
        if !flow_dir.join(file_name).exists() {
            continue;
        }
        if module_name == "Flow.List" {
            imports.push(format!("import {module_name} except [concat, delete]"));
        } else {
            imports.push(format!("import {module_name} exposing (..)"));
        }
    }

    if imports.is_empty() {
        return;
    }

    let prelude_source = imports.join("\n");
    let main_interner = parser.take_interner();
    let prelude_lexer =
        flux::syntax::lexer::Lexer::new_with_interner(&prelude_source, main_interner);
    let mut prelude_parser = flux::syntax::parser::Parser::new(prelude_lexer);
    let prelude_program = prelude_parser.parse_program();

    let enriched_interner = prelude_parser.take_interner();
    parser.restore_interner(enriched_interner);

    // Prepend the synthetic imports to the program.
    let mut new_statements = prelude_program.statements;
    new_statements.append(&mut program.statements);
    program.statements = new_statements;
}

/// Extract the module name and symbol from a program's top-level `module Name { ... }` statement.
fn extract_module_name_and_sym(
    program: &Program,
    interner: &flux::syntax::interner::Interner,
) -> Option<(String, flux::syntax::Identifier)> {
    for stmt in &program.statements {
        if let flux::syntax::statement::Statement::Module { name, .. } = stmt {
            return Some((interner.resolve(*name).to_string(), *name));
        }
    }
    None
}

fn extract_test_filter(args: &mut Vec<String>) -> Option<Option<String>> {
    let mut test_filter: Option<String> = None;
    let mut i = 1usize;
    while i < args.len() {
        if args[i] == "--test-filter" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --test --test-filter <pattern>");
                return None;
            }
            test_filter = Some(args.remove(i + 1));
            args.remove(i);
        } else if let Some(v) = args[i].strip_prefix("--test-filter=") {
            test_filter = Some(v.to_string());
            args.remove(i);
        } else {
            i += 1;
        }
    }
    Some(test_filter)
}

fn extract_roots(args: &mut Vec<String>, roots: &mut Vec<std::path::PathBuf>) -> bool {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--root" {
            if i + 1 >= args.len() {
                eprintln!(
                    "Usage: flux <file.flx> --root <path> [--root <path> ...]\n       flux run <file.flx> --root <path> [--root <path> ...]"
                );
                return false;
            }
            let path = args.remove(i + 1);
            args.remove(i);
            roots.push(std::path::PathBuf::from(path));
        } else if let Some(v) = args[i].strip_prefix("--root=") {
            let path = v.to_string();
            args.remove(i);
            roots.push(std::path::PathBuf::from(path));
        } else {
            i += 1;
        }
    }
    true
}

fn collect_roots(entry_path: &Path, extra_roots: &[PathBuf], roots_only: bool) -> Vec<PathBuf> {
    let mut roots = extra_roots.to_vec();
    if !roots_only {
        if let Some(parent) = entry_path.parent() {
            roots.push(parent.to_path_buf());
        }
        let project_src = Path::new("src");
        if project_src.exists() {
            roots.push(project_src.to_path_buf());
        }
        // Add lib/ as a root for Flow library resolution (Proposal 0120).
        // Searches: relative to cwd, then up from the executable.
        let project_lib = Path::new("lib");
        if project_lib.exists() {
            roots.push(project_lib.to_path_buf());
        }
    }
    roots
}

fn normalize_roots_for_cache(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut normalized = Vec::new();
    for root in roots {
        let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if !normalized.iter().any(|p| p == &canonical) {
            normalized.push(canonical);
        }
    }
    normalized
}

fn roots_cache_hash(roots: &[PathBuf]) -> [u8; 32] {
    let normalized = normalize_roots_for_cache(roots);
    let mut joined = String::new();
    for root in normalized {
        joined.push_str(&root.to_string_lossy());
        joined.push('\n');
    }
    hash_bytes(joined.as_bytes())
}

fn is_flx_file(path: &str) -> bool {
    Path::new(path).extension().and_then(|ext| ext.to_str()) == Some("flx")
}

fn show_tokens(path: &str) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let mut lexer = Lexer::new(&source);
            println!("Tokens from {}:", path);
            println!("{}", "─".repeat(50));
            for tok in lexer.tokenize() {
                println!(
                    "{:>3}:{:<3} {:12} {:?}",
                    tok.position.line,
                    tok.position.column,
                    tok.token_type.to_string(),
                    tok.literal
                );
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn show_bytecode(
    path: &str,
    enable_optimize: bool,
    enable_analyze: bool,
    max_errors: usize,
    strict_mode: bool,
    diagnostics_format: DiagnosticOutputFormat,
) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();
            let mut warnings = parser.take_warnings();
            for diag in &mut warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }

            if !parser.errors.is_empty() {
                emit_diagnostics(
                    &parser.errors,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                emit_diagnostics(
                    &warnings,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
            }

            let interner = parser.take_interner();
            let mut compiler = Compiler::new_with_interner(path, interner);
            compiler.set_strict_mode(strict_mode);
            let compile_result =
                compiler.compile_with_opts(&program, enable_optimize, enable_analyze);
            let mut compiler_warnings = compiler.take_warnings();
            for diag in &mut compiler_warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }
            if !compiler_warnings.is_empty() {
                emit_diagnostics(
                    &compiler_warnings,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
            }
            if let Err(diags) = compile_result {
                emit_diagnostics(
                    &diags,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
                std::process::exit(1);
            }

            let bytecode = compiler.bytecode();
            println!("Bytecode from {}:", path);
            println!("{}", "─".repeat(50));
            println!("Constants:");
            for (i, c) in bytecode.constants.iter().enumerate() {
                println!("  {}: {}", i, c);
            }
            println!("\nInstructions:");
            print!("{}", disassemble(&bytecode.instructions));

            // Disassemble function constants
            for (i, c) in bytecode.constants.iter().enumerate() {
                if let Value::Function(f) = c {
                    let name = f
                        .debug_info
                        .as_ref()
                        .and_then(|d| d.name.as_deref())
                        .unwrap_or("<anonymous>");
                    println!("\nFunction <{}> (constant {}):", name, i);
                    print!("{}", disassemble(&f.instructions));
                }
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn lint_file(path: &str, max_errors: usize, diagnostics_format: DiagnosticOutputFormat) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();
            let mut warnings = parser.take_warnings();
            for diag in &mut warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }

            if !parser.errors.is_empty() {
                emit_diagnostics(
                    &parser.errors,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                emit_diagnostics(
                    &warnings,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
            }

            let interner = parser.take_interner();
            let lints = Linter::new(Some(path.to_string()), &interner).lint(&program);
            if !lints.is_empty() {
                emit_diagnostics(
                    &lints,
                    Some(path),
                    Some(source.as_str()),
                    false,
                    max_errors,
                    diagnostics_format,
                    false,
                    false,
                );
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn fmt_file(path: &str, check: bool) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let formatted = format_source(&source);
            if check {
                if source.trim() != formatted.trim() {
                    eprintln!("format: changes needed");
                    std::process::exit(1);
                }
                return;
            }

            if let Err(err) = fs::write(path, formatted) {
                eprintln!("Error writing {}: {}", path, err);
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn analyze_free_vars(path: &str, max_errors: usize, diagnostics_format: DiagnosticOutputFormat) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();
            let mut warnings = parser.take_warnings();
            for diag in &mut warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }

            if !parser.errors.is_empty() {
                emit_diagnostics(
                    &parser.errors,
                    Some(path),
                    Some(source.as_str()),
                    true,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                emit_diagnostics(
                    &warnings,
                    Some(path),
                    Some(source.as_str()),
                    true,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
            }

            let interner = parser.take_interner();
            let free_vars = collect_free_vars_in_program(&program);

            if free_vars.is_empty() {
                println!("✓ No free variables found in {}", path);
            } else {
                println!("Free variables in {}:", path);
                println!("{}", "─".repeat(50));
                let mut vars: Vec<_> = free_vars.iter().map(|sym| interner.resolve(*sym)).collect();
                vars.sort();
                for var in vars {
                    println!("  • {}", var);
                }
                println!("\nTotal: {} free variable(s)", free_vars.len());
                println!(
                    "\nℹ️  Free variables are identifiers that are referenced but not defined."
                );
                println!("   This may indicate undefined variables or missing imports.");
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn analyze_tail_calls(path: &str, max_errors: usize, diagnostics_format: DiagnosticOutputFormat) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();
            let mut warnings = parser.take_warnings();
            for diag in &mut warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }

            if !parser.errors.is_empty() {
                emit_diagnostics(
                    &parser.errors,
                    Some(path),
                    Some(source.as_str()),
                    true,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                emit_diagnostics(
                    &warnings,
                    Some(path),
                    Some(source.as_str()),
                    true,
                    max_errors,
                    diagnostics_format,
                    false,
                    true,
                );
            }

            let tail_calls = find_tail_calls(&program);

            if tail_calls.is_empty() {
                println!("✓ No tail calls found in {}", path);
                println!(
                    "\nℹ️  Tail calls are function calls in tail position that can be optimized."
                );
            } else {
                println!("Tail calls in {}:", path);
                println!("{}", "─".repeat(50));

                // Group by line
                let lines: Vec<_> = source.lines().collect();
                for (idx, call) in tail_calls.iter().enumerate() {
                    let line_num = call.span.start.line;
                    let line_text = if line_num > 0 && line_num <= lines.len() {
                        lines[line_num - 1].trim()
                    } else {
                        "<unknown>"
                    };

                    println!("  {}. Line {}: {}", idx + 1, line_num, line_text);
                }

                println!("\nTotal: {} tail call(s)", tail_calls.len());
                println!("\n✓ These calls are eligible for tail call optimization (TCO).");
                println!(
                    "  The Flux compiler automatically optimizes tail calls to avoid stack overflow."
                );
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn show_cache_info(path: &str, extra_roots: &[PathBuf]) {
    let cache = BytecodeCache::new(Path::new("target").join("flux"));
    let source = match fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };
    let source_hash = hash_bytes(source.as_bytes());
    let entry_path = Path::new(path);
    let roots = collect_roots(entry_path, extra_roots, false);
    let roots_hash = roots_cache_hash(&roots);
    let base_cache_key = hash_cache_key(&source_hash, &roots_hash);
    let strict_hash = hash_bytes(b"strict=0");
    let cache_key = hash_cache_key(&base_cache_key, &strict_hash);
    let info = cache.inspect(Path::new(path), &cache_key);
    match info {
        Some(info) => {
            println!("cache file: {}", info.cache_path.display());
            println!("format version: {}", info.format_version);
            println!("compiler version: {}", info.compiler_version);
            println!("cache key: {}", hex_string(&info.source_hash));
            println!("constants: {}", info.constants_count);
            println!("instructions: {} bytes", info.instructions_len);
            if info.deps.is_empty() {
                println!("deps: none");
            } else {
                println!("deps:");
                for (path, hash, valid) in info.deps {
                    println!(
                        "  - {} {} ({})",
                        path,
                        hex_string(&hash),
                        if valid { "ok" } else { "stale" }
                    );
                }
            }
        }
        None => {
            println!("cache: not found or invalid");
        }
    }
}

fn show_cache_info_file(path: &str) {
    let cache = BytecodeCache::new(Path::new("target").join("flux"));
    let info = cache.inspect_file(Path::new(path));
    match info {
        Some(info) => {
            println!("cache file: {}", info.cache_path.display());
            println!("format version: {}", info.format_version);
            println!("compiler version: {}", info.compiler_version);
            println!("cache key: {}", hex_string(&info.source_hash));
            println!("constants: {}", info.constants_count);
            println!("instructions: {} bytes", info.instructions_len);
            if info.deps.is_empty() {
                println!("deps: none");
            } else {
                println!("deps:");
                for (path, hash, valid) in info.deps {
                    println!(
                        "  - {} {} ({})",
                        path,
                        hex_string(&hash),
                        if valid { "ok" } else { "stale" }
                    );
                }
            }

            if let Some(bytecode) = cache.load_file(Path::new(path)) {
                println!("\nConstants:");
                for (i, c) in bytecode.constants.iter().enumerate() {
                    println!("  {}: {}", i, c);
                }
                println!("\nInstructions:");
                print!("{}", disassemble(&bytecode.instructions));
            }
        }
        None => {
            println!("cache: not found or invalid");
        }
    }
}

fn show_interface_info_file(path: &str) {
    let Some(interface) =
        flux::bytecode::compiler::module_interface::load_interface(Path::new(path))
    else {
        println!("interface: not found or invalid");
        return;
    };

    println!("interface file: {}", path);
    println!("module: {}", interface.module_name);
    println!("compiler version: {}", interface.compiler_version);
    println!("source hash: {}", interface.source_hash);
    println!("schemes: {}", interface.schemes.len());
    println!("borrow signatures: {}", interface.borrow_signatures.len());

    let mut members: Vec<_> = interface
        .schemes
        .keys()
        .chain(interface.borrow_signatures.keys())
        .cloned()
        .collect();
    members.sort();
    members.dedup();

    if members.is_empty() {
        println!("exports: none");
        return;
    }

    println!("exports:");
    for member in members {
        println!(
            "  - {}: {}",
            member,
            interface
                .schemes
                .get(&member)
                .map(format_scheme_for_cli)
                .unwrap_or_else(|| "<no scheme>".to_string())
        );
        if let Some(signature) = interface.borrow_signatures.get(&member) {
            println!(
                "    borrow: [{}] ({})",
                signature
                    .params
                    .iter()
                    .map(format_borrow_mode)
                    .collect::<Vec<_>>()
                    .join(", "),
                format_borrow_provenance(signature.provenance)
            );
        }
    }
}

fn format_scheme_for_cli(scheme: &flux::types::scheme::Scheme) -> String {
    if scheme.forall.is_empty() {
        scheme.infer_type.to_string()
    } else {
        format!("forall {:?}. {}", scheme.forall, scheme.infer_type)
    }
}

fn format_borrow_mode(mode: &flux::aether::borrow_infer::BorrowMode) -> &'static str {
    match mode {
        flux::aether::borrow_infer::BorrowMode::Owned => "Owned",
        flux::aether::borrow_infer::BorrowMode::Borrowed => "Borrowed",
    }
}

fn format_borrow_provenance(
    provenance: flux::aether::borrow_infer::BorrowProvenance,
) -> &'static str {
    match provenance {
        flux::aether::borrow_infer::BorrowProvenance::Inferred => "Inferred",
        flux::aether::borrow_infer::BorrowProvenance::BaseRuntime => "BaseRuntime",
        flux::aether::borrow_infer::BorrowProvenance::Imported => "Imported",
        flux::aether::borrow_infer::BorrowProvenance::Unknown => "Unknown",
    }
}

fn hex_string(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}
