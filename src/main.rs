use std::{
    collections::HashSet,
    env, fs, io,
    path::{Path, PathBuf},
};

use flux::{
    ast::{collect_free_vars_in_program, find_tail_calls},
    bytecode::{
        bytecode_cache::{BytecodeCache, hash_bytes, hash_cache_key, hash_file},
        compiler::Compiler,
        op_code::disassemble,
    },
    diagnostics::{DEFAULT_MAX_ERRORS, Diagnostic, DiagnosticsAggregator, position::Span},
    runtime::{gc::GcHeap, value::Value, vm::VM},
    syntax::{
        formatter::format_source, interner::Interner, lexer::Lexer, linter::Linter,
        module_graph::ModuleGraph, parser::Parser,
    },
};
#[cfg(feature = "jit")]
use flux::syntax::program::Program;

fn main() {
    let mut args: Vec<String> = env::args().collect();
    let verbose = args.iter().any(|arg| arg == "--verbose");
    let leak_detector = args.iter().any(|arg| arg == "--leak-detector");
    let trace = args.iter().any(|arg| arg == "--trace");
    let no_cache = args.iter().any(|arg| arg == "--no-cache");
    let roots_only = args.iter().any(|arg| arg == "--roots-only");
    let enable_optimize = args.iter().any(|arg| arg == "--optimize" || arg == "-O");
    let enable_analyze = args.iter().any(|arg| arg == "--analyze" || arg == "-A");
    let no_gc = args.iter().any(|arg| arg == "--no-gc");
    let gc_telemetry = args.iter().any(|arg| arg == "--gc-telemetry");
    #[cfg(feature = "jit")]
    let use_jit = args.iter().any(|arg| arg == "--jit");
    #[cfg(not(feature = "jit"))]
    let use_jit = false;
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
    if no_gc {
        args.retain(|arg| arg != "--no-gc");
    }
    if gc_telemetry {
        args.retain(|arg| arg != "--gc-telemetry");
    }
    if use_jit {
        args.retain(|arg| arg != "--jit");
    }
    let gc_threshold = match extract_gc_threshold(&mut args) {
        Some(value) => value,
        None => return,
    };
    let max_errors = match extract_max_errors(&mut args) {
        Some(value) => value,
        None => return,
    };
    if !extract_roots(&mut args, &mut roots) {
        return;
    }

    if args.len() < 2 {
        print_help();
        return;
    }

    if is_flx_file(&args[1]) {
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
            no_gc,
            gc_threshold,
            gc_telemetry,
            use_jit,
        );
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
                eprintln!("Error: file must have .flx extension: {}", args[2]);
                return;
            }
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
                no_gc,
                gc_threshold,
                gc_telemetry,
                use_jit,
            )
        }
        "tokens" => {
            if args.len() < 3 {
                eprintln!("Usage: flux tokens <file.flx>");
                return;
            }
            if !is_flx_file(&args[2]) {
                eprintln!("Error: file must have .flx extension: {}", args[2]);
                return;
            }
            show_tokens(&args[2]);
        }
        "bytecode" => {
            if args.len() < 3 {
                eprintln!("Usage: flux bytecode <file.flx>");
                return;
            }
            show_bytecode(&args[2], enable_optimize, enable_analyze, max_errors);
        }
        "lint" => {
            if args.len() < 3 {
                eprintln!("Usage: flux lint <file.flx>");
                return;
            }
            lint_file(&args[2], max_errors);
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
            fmt_file(file, check);
        }
        "cache-info" => {
            if args.len() < 3 {
                eprintln!("Usage: flux cache-info <file.flx>");
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
        "analyze-free-vars" | "free-vars" => {
            if args.len() < 3 {
                eprintln!("Usage: flux analyze-free-vars <file.flx>");
                return;
            }
            analyze_free_vars(&args[2], max_errors);
        }
        "analyze-tail-calls" | "analyze-tails-calls" | "tail-calls" => {
            if args.len() < 3 {
                eprintln!("Usage: flux analyze-tail-calls <file.flx>");
                return;
            }
            analyze_tail_calls(&args[2], max_errors);
        }
        "repl" => {
            repl(trace);
        }
        _ => {}
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
  flux analyze-free-vars <file.flx>
  flux analyze-tail-calls <file.flx>
  flux repl
  flux <file.flx> --root <path> [--root <path> ...]
  flux run <file.flx> --root <path> [--root <path> ...]

Flags:
  --verbose          Show cache status (hit/miss/store)
  --trace            Print VM instruction trace
  --leak-detector    Print approximate allocation stats after run
  --no-cache         Disable bytecode cache for this run
  --optimize, -O     Enable AST optimizations (desugar + constant fold)
  --analyze, -A      Enable analysis passes (free vars + tail calls)
  --max-errors <n>   Limit displayed errors (default: 50)
  --root <path>      Add a module root (can be repeated)
  --roots-only       Use only explicitly provided --root values
  --gc-telemetry     Print GC telemetry report after execution (requires --features gc-telemetry)
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
    no_gc: bool,
    gc_threshold: Option<usize>,
    gc_telemetry: bool,
    #[cfg_attr(not(feature = "jit"), allow(unused))] use_jit: bool,
) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let source_hash = hash_bytes(source.as_bytes());
            let entry_path = Path::new(path);
            let roots = collect_roots(entry_path, extra_roots, roots_only);
            let roots_hash = roots_cache_hash(&roots);
            let cache_key = hash_cache_key(&source_hash, &roots_hash);
            let cache = BytecodeCache::new(Path::new("target").join("flux"));
            if !no_cache {
                if let Some(bytecode) =
                    cache.load(Path::new(path), &cache_key, env!("CARGO_PKG_VERSION"))
                {
                    if verbose {
                        eprintln!("cache: hit (bytecode loaded)");
                    }
                    let mut vm = VM::new(bytecode);
                    vm.set_trace(trace);
                    if no_gc {
                        vm.set_gc_enabled(false);
                    }
                    if let Some(threshold) = gc_threshold {
                        vm.set_gc_threshold(threshold);
                    }
                    if let Err(err) = vm.run() {
                        eprintln!("{}", err);
                        std::process::exit(1);
                    }
                    #[cfg(feature = "gc-telemetry")]
                    if gc_telemetry {
                        println!("\n{}", vm.gc_telemetry_report());
                    }
                    #[cfg(not(feature = "gc-telemetry"))]
                    if gc_telemetry {
                        eprintln!(
                            "Warning: --gc-telemetry requires building with `--features gc-telemetry`"
                        );
                    }
                    if leak_detector {
                        print_leak_stats();
                    }
                    return;
                }
                if verbose {
                    eprintln!("cache: miss (compiling)");
                }
            }

            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();

            // --- Collect all diagnostics into a single pool ---
            let mut all_diagnostics: Vec<Diagnostic> = Vec::new();
            let mut parse_warnings = parser.take_warnings();
            for diag in &mut parse_warnings {
                if diag.file().is_none() {
                    diag.set_file(path.to_string());
                }
            }
            all_diagnostics.append(&mut parse_warnings);

            // Entry file parse errors: collect but do NOT exit early.
            let entry_has_errors = !parser.errors.is_empty();
            if entry_has_errors {
                for diag in &mut parser.errors {
                    if diag.file().is_none() {
                        diag.set_file(path.to_string());
                    }
                }
                all_diagnostics.append(&mut parser.errors);
            }

            let interner = parser.take_interner();
            let entry_path = Path::new(path);
            let roots = collect_roots(entry_path, extra_roots, roots_only);

            // --- Build module graph (always returns, may have diagnostics) ---
            let graph_result =
                ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, &roots);
            all_diagnostics.extend(graph_result.diagnostics);

            // Track all failed modules (parse + validation failures from graph).
            let mut failed: HashSet<PathBuf> = graph_result.failed_modules;
            if entry_has_errors && let Ok(canon) = std::fs::canonicalize(entry_path) {
                failed.insert(canon);
            }

            let is_multimodule = graph_result.graph.module_count() > 1;
            let graph = graph_result.graph;

            // --- Compile valid modules, suppress cascade ---
            let mut compiler = Compiler::new_with_interner(path, graph_result.interner);
            let entry_canonical = std::fs::canonicalize(entry_path).ok();
            for node in graph.topo_order() {
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
                    // GHC-style skip note
                    all_diagnostics.push(Diagnostic::make_note(
                        "MODULE SKIPPED",
                        format!(
                            "Module `{}` was skipped because its dependency `{}` has errors.",
                            node.path.to_string_lossy(),
                            dep.name,
                        ),
                        node.path.to_string_lossy().to_string(),
                        Span::default(),
                    ));
                    continue;
                }

                compiler.set_file_path(node.path.to_string_lossy().to_string());
                if let Err(mut diags) =
                    compiler.compile_with_opts(&node.program, enable_optimize, enable_analyze)
                {
                    for diag in &mut diags {
                        if diag.file().is_none() {
                            diag.set_file(node.path.to_string_lossy().to_string());
                        }
                    }
                    all_diagnostics.append(&mut diags);
                    continue;
                }
            }

            // --- One unified report ---
            if !all_diagnostics.is_empty() {
                let report = DiagnosticsAggregator::new(&all_diagnostics)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(is_multimodule)
                    .with_max_errors(Some(max_errors))
                    .report();
                if report.counts.errors > 0 {
                    eprintln!("{}", report.rendered);
                    std::process::exit(1);
                }
                eprintln!("{}", report.rendered);
            }

            // --- JIT execution path ---
            #[cfg(feature = "jit")]
            if use_jit {
                // JIT must see the same module set as the VM path (entry + imports).
                let mut jit_program = Program::new();
                for node in graph.topo_order() {
                    jit_program
                        .statements
                        .extend(node.program.statements.clone());
                }

                match flux::jit::jit_compile_and_run(&jit_program, &compiler.interner) {
                    Ok(_result) => {}
                    Err(err) => {
                        eprintln!("{}", err);
                        std::process::exit(1);
                    }
                }
                if leak_detector {
                    print_leak_stats();
                }
                return;
            }

            let bytecode = compiler.bytecode();

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

            let mut vm = VM::new(bytecode);
            vm.set_trace(trace);
            if no_gc {
                vm.set_gc_enabled(false);
            }
            if let Some(threshold) = gc_threshold {
                vm.set_gc_threshold(threshold);
            }
            if let Err(err) = vm.run() {
                eprintln!("{}", err);
                std::process::exit(1);
            }
            #[cfg(feature = "gc-telemetry")]
            if gc_telemetry {
                println!("\n{}", vm.gc_telemetry_report());
            }
            #[cfg(not(feature = "gc-telemetry"))]
            if gc_telemetry {
                eprintln!(
                    "Warning: --gc-telemetry requires building with `--features gc-telemetry`"
                );
            }
            if leak_detector {
                print_leak_stats();
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn print_leak_stats() {
    let stats = flux::runtime::leak_detector::snapshot();
    println!(
        "\nLeak stats (approx):\n  compiled_functions: {}\n  closures: {}\n  arrays: {}\n  hashes: {}\n  somes: {}",
        stats.compiled_functions, stats.closures, stats.arrays, stats.hashes, stats.somes
    );
}

fn extract_gc_threshold(args: &mut Vec<String>) -> Option<Option<usize>> {
    let mut threshold = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--gc-threshold" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --gc-threshold <n>");
                return None;
            }
            let value = args.remove(i + 1);
            args.remove(i);
            match value.parse::<usize>() {
                Ok(parsed) => {
                    threshold = Some(parsed);
                }
                Err(_) => {
                    eprintln!("Error: --gc-threshold expects a non-negative integer.");
                    return None;
                }
            }
            continue;
        }
        i += 1;
    }
    Some(threshold)
}

fn extract_max_errors(args: &mut Vec<String>) -> Option<usize> {
    let mut max_errors = DEFAULT_MAX_ERRORS;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--max-errors" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --max-errors <n>");
                return None;
            }
            let value = args.remove(i + 1);
            args.remove(i);
            match value.parse::<usize>() {
                Ok(parsed) => {
                    max_errors = parsed;
                }
                Err(_) => {
                    eprintln!("Error: --max-errors expects a non-negative integer.");
                    return None;
                }
            }
            continue;
        }
        i += 1;
    }
    Some(max_errors)
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
            continue;
        }
        i += 1;
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

fn show_bytecode(path: &str, enable_optimize: bool, enable_analyze: bool, max_errors: usize) {
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
                let report = DiagnosticsAggregator::new(&parser.errors)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(false)
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                let report = DiagnosticsAggregator::new(&warnings)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(false)
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
            }

            let interner = parser.take_interner();
            let mut compiler = Compiler::new_with_interner(path, interner);
            if let Err(diags) =
                compiler.compile_with_opts(&program, enable_optimize, enable_analyze)
            {
                let report = DiagnosticsAggregator::new(&diags)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(false)
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
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

fn lint_file(path: &str, max_errors: usize) {
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
                let report = DiagnosticsAggregator::new(&parser.errors)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(false)
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                let report = DiagnosticsAggregator::new(&warnings)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(false)
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
            }

            let interner = parser.take_interner();
            let lints = Linter::new(Some(path.to_string()), &interner).lint(&program);
            if !lints.is_empty() {
                let report = DiagnosticsAggregator::new(&lints)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(false)
                    .with_max_errors(Some(max_errors))
                    .report();
                println!("{}", report.rendered);
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

fn analyze_free_vars(path: &str, max_errors: usize) {
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
                let report = DiagnosticsAggregator::new(&parser.errors)
                    .with_default_source(path, source.as_str())
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                let report = DiagnosticsAggregator::new(&warnings)
                    .with_default_source(path, source.as_str())
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
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

fn analyze_tail_calls(path: &str, max_errors: usize) {
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
                let report = DiagnosticsAggregator::new(&parser.errors)
                    .with_default_source(path, source.as_str())
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
                std::process::exit(1);
            }

            if !warnings.is_empty() {
                let report = DiagnosticsAggregator::new(&warnings)
                    .with_default_source(path, source.as_str())
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
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
    let cache_key = hash_cache_key(&source_hash, &roots_hash);
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

fn hex_string(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

fn repl(trace: bool) {
    use io::Write;

    println!(
        "Flux REPL v{} (type :help for help, :quit to exit)",
        env!("CARGO_PKG_VERSION")
    );

    let stdin = io::stdin();
    let mut reader = stdin.lock();

    // Bootstrap compiler to register builtins in the symbol table.
    let bootstrap = Compiler::new_with_interner("<repl>", Interner::new());
    let (mut symbol_table, mut constants, mut interner) = bootstrap.take_state();
    let mut globals: Vec<Value> = vec![Value::None; 65536];
    let mut gc_heap = GcHeap::new();

    loop {
        print!("flux> ");
        io::stdout().flush().unwrap();

        let input = match read_repl_input(&mut reader) {
            Some(input) => input,
            None => break, // EOF
        };

        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }

        match trimmed {
            ":quit" | ":q" => break,
            ":help" | ":h" => {
                print_repl_help();
                continue;
            }
            _ => {}
        }

        // --- Parse ---
        let lexer = Lexer::new_with_interner(&input, interner);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let mut warnings = parser.take_warnings();
        for diag in &mut warnings {
            if diag.file().is_none() {
                diag.set_file("<repl>");
            }
        }

        if !parser.errors.is_empty() {
            let report = DiagnosticsAggregator::new(&parser.errors)
                .with_default_source("<repl>", &input)
                .with_file_headers(false)
                .report();
            eprintln!("{}", report.rendered);
            interner = parser.take_interner();
            continue;
        }

        if !warnings.is_empty() {
            let report = DiagnosticsAggregator::new(&warnings)
                .with_default_source("<repl>", &input)
                .with_file_headers(false)
                .report();
            eprintln!("{}", report.rendered);
        }

        interner = parser.take_interner();

        // --- Compile ---
        let mut compiler = Compiler::new_with_state(symbol_table, constants, interner);
        compiler.set_file_path("<repl>");

        if let Err(errs) = compiler.compile(&program) {
            let report = DiagnosticsAggregator::new(&errs)
                .with_default_source("<repl>", &input)
                .with_file_headers(false)
                .report();
            eprintln!("{}", report.rendered);
            let state = compiler.take_state();
            symbol_table = state.0;
            constants = state.1;
            interner = state.2;
            continue;
        }

        let bytecode = compiler.bytecode();
        let state = compiler.take_state();
        symbol_table = state.0;
        constants = state.1;
        interner = state.2;

        // --- Execute ---
        let mut vm = VM::new(bytecode);
        vm.set_trace(trace);
        std::mem::swap(&mut vm.globals, &mut globals);
        std::mem::swap(&mut vm.gc_heap, &mut gc_heap);

        match vm.run() {
            Ok(()) => {
                let result = vm.last_popped_stack_elem();
                if !matches!(result, Value::None) {
                    println!("{}", result);
                }
            }
            Err(err) => {
                eprintln!("{}", err);
            }
        }

        // Persist VM state for next iteration
        std::mem::swap(&mut vm.globals, &mut globals);
        std::mem::swap(&mut vm.gc_heap, &mut gc_heap);
    }

    println!("Goodbye!");
}

fn read_repl_input(reader: &mut impl io::BufRead) -> Option<String> {
    use io::Write;

    let mut input = String::new();
    let mut depth: i32 = 0;

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                if input.is_empty() {
                    return None;
                }
                return Some(input);
            }
            Ok(_) => {
                for ch in line.chars() {
                    match ch {
                        '{' | '(' | '[' => depth += 1,
                        '}' | ')' | ']' => depth -= 1,
                        _ => {}
                    }
                }
                input.push_str(&line);

                if depth <= 0 {
                    return Some(input);
                }

                print!("  ... ");
                io::stdout().flush().unwrap();
            }
            Err(_) => return None,
        }
    }
}

fn print_repl_help() {
    println!(
        "\
Commands:
  :quit, :q    Exit the REPL
  :help, :h    Show this help message

Enter Flux expressions or statements.
Multi-line input: unmatched braces trigger continuation prompt.
Expression results are printed automatically."
    );
}
