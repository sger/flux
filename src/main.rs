use std::{
    env, fs,
    path::{Path, PathBuf},
};

use flux::{
    bytecode::{
        bytecode_cache::{BytecodeCache, hash_bytes, hash_cache_key, hash_file},
        compiler::Compiler,
        op_code::disassemble,
    },
    runtime::{value::Value, vm::VM},
    syntax::{
        diagnostics::{DEFAULT_MAX_ERRORS, Diagnostic, DiagnosticsAggregator},
        formatter::format_source,
        lexer::Lexer,
        linter::Linter,
        module_graph::ModuleGraph,
        parser::Parser,
    },
};

fn main() {
    let mut args: Vec<String> = env::args().collect();
    let verbose = args.iter().any(|arg| arg == "--verbose");
    let leak_detector = args.iter().any(|arg| arg == "--leak-detector");
    let trace = args.iter().any(|arg| arg == "--trace");
    let no_cache = args.iter().any(|arg| arg == "--no-cache");
    let roots_only = args.iter().any(|arg| arg == "--roots-only");
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
            max_errors,
            &roots,
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
                max_errors,
                &roots,
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
            show_bytecode(&args[2], max_errors);
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
  flux <file.flx> --root <path> [--root <path> ...]
  flux run <file.flx> --root <path> [--root <path> ...]

Flags:
  --verbose   Show cache status (hit/miss/store)
  --trace  Print VM instruction trace
  --leak-detector  Print approximate allocation stats after run
  --no-cache  Disable bytecode cache for this run
  --max-errors <n>  Limit displayed errors (default: 50)
  --root <path>  Add a module root (can be repeated)
  --roots-only  Use only explicitly provided --root values
  -h, --help  Show this help message
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
    max_errors: usize,
    extra_roots: &[std::path::PathBuf],
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
                    if let Err(err) = vm.run() {
                        eprintln!("{}", err);
                        std::process::exit(1);
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

            if !parser.errors.is_empty() {
                let report = DiagnosticsAggregator::new(&parser.errors)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(false)
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
                std::process::exit(1);
            }

            let interner = parser.take_interner();
            let entry_path = Path::new(path);
            let roots = collect_roots(entry_path, extra_roots, roots_only);

            let (graph, interner) = match ModuleGraph::build_with_entry_and_roots(
                entry_path, &program, interner, &roots,
            ) {
                Ok(result) => result,
                Err(diags) => {
                    let report = DiagnosticsAggregator::new(&diags)
                        .with_file_headers(false)
                        .with_max_errors(Some(max_errors))
                        .report();
                    eprintln!("{}", report.rendered);
                    std::process::exit(1);
                }
            };

            let mut compiler = Compiler::new_with_interner(path, interner);
            let mut compile_errors: Vec<Diagnostic> = Vec::new();
            for node in graph.topo_order() {
                compiler.set_file_path(node.path.to_string_lossy().to_string());
                if let Err(mut diags) = compiler.compile(&node.program) {
                    for diag in &mut diags {
                        if diag.file().is_none() {
                            diag.set_file(node.path.to_string_lossy().to_string());
                        }
                    }
                    compile_errors.append(&mut diags);
                    break;
                }
            }
            if !compile_errors.is_empty() {
                let report = DiagnosticsAggregator::new(&compile_errors)
                    .with_file_headers(false)
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
                std::process::exit(1);
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
            if let Err(err) = vm.run() {
                eprintln!("{}", err);
                std::process::exit(1);
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

fn show_bytecode(path: &str, max_errors: usize) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();

            if !parser.errors.is_empty() {
                let report = DiagnosticsAggregator::new(&parser.errors)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(false)
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
                std::process::exit(1);
            }

            let interner = parser.take_interner();
            let mut compiler = Compiler::new_with_interner(path, interner);
            if let Err(diags) = compiler.compile(&program) {
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

            if !parser.errors.is_empty() {
                let report = DiagnosticsAggregator::new(&parser.errors)
                    .with_default_source(path, source.as_str())
                    .with_file_headers(false)
                    .with_max_errors(Some(max_errors))
                    .report();
                eprintln!("{}", report.rendered);
                std::process::exit(1);
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
