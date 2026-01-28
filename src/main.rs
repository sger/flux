use std::{env, fs, path::Path};

use flux::{
    bytecode::{
        bytecode_cache::{BytecodeCache, hash_bytes, hash_file},
        compiler::Compiler,
        op_code::disassemble,
    },
    frontend::{
        diagnostic::render_diagnostics, formatter::format_source, lexer::Lexer, linter::Linter,
        parser::Parser,
    },
    runtime::vm::VM,
};

fn main() {
    let mut args: Vec<String> = env::args().collect();
    let verbose = args.iter().any(|arg| arg == "--verbose");
    if verbose {
        args.retain(|arg| arg != "--verbose");
    }

    if args.len() < 2 {
        print_help();
        return;
    }

    if is_flx_file(&args[1]) {
        run_file(&args[1], verbose);
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
            run_file(&args[2], verbose)
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
            show_bytecode(&args[2]);
        }
        "lint" => {
            if args.len() < 3 {
                eprintln!("Usage: flux lint <file.flx>");
                return;
            }
            lint_file(&args[2]);
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
            show_cache_info(&args[2]);
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

Flags:
  --verbose   Show cache status (hit/miss/store)
  -h, --help  Show this help message
"
    );
}

fn run_file(path: &str, verbose: bool) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let source_hash = hash_bytes(source.as_bytes());
            let cache = BytecodeCache::new(Path::new("target").join("flux"));
            if let Some(bytecode) =
                cache.load(Path::new(path), &source_hash, env!("CARGO_PKG_VERSION"))
            {
                if verbose {
                    eprintln!("cache: hit (bytecode loaded)");
                }
                let mut vm = VM::new(bytecode);
                if let Err(err) = vm.run() {
                    eprintln!("Runtime error: {}", err);
                }
                return;
            }
            if verbose {
                eprintln!("cache: miss (compiling)");
            }

            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();

            if !parser.errors.is_empty() {
                eprintln!(
                    "{}",
                    render_diagnostics(&parser.errors, Some(&source), Some(path))
                );
                return;
            }

            let mut compiler = Compiler::new_with_file_path(path);
            if let Err(diags) = compiler.compile(&program) {
                eprintln!("{}", render_diagnostics(&diags, Some(&source), Some(path)));
                return;
            }

            let bytecode = compiler.bytecode();

            let mut deps = Vec::new();
            for dep in compiler.imported_files() {
                if let Ok(hash) = hash_file(Path::new(&dep)) {
                    deps.push((dep, hash));
                }
            }
            let stored = cache
                .store(
                    Path::new(path),
                    &source_hash,
                    env!("CARGO_PKG_VERSION"),
                    &bytecode,
                    &deps,
                )
                .is_ok();
            if verbose && stored {
                eprintln!("cache: stored");
            }

            let mut vm = VM::new(bytecode);
            if let Err(err) = vm.run() {
                eprintln!("Runtime error: {}", err);
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
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

fn show_bytecode(path: &str) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();

            if !parser.errors.is_empty() {
                eprintln!(
                    "{}",
                    render_diagnostics(&parser.errors, Some(&source), Some(path))
                );
                return;
            }

            let mut compiler = Compiler::new_with_file_path(path);
            if let Err(diags) = compiler.compile(&program) {
                eprintln!("{}", render_diagnostics(&diags, Some(&source), Some(path)));
                return;
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
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

fn lint_file(path: &str) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let lexer = Lexer::new(&source);
            let mut parser = Parser::new(lexer);
            let program = parser.parse_program();

            if !parser.errors.is_empty() {
                eprintln!(
                    "{}",
                    render_diagnostics(&parser.errors, Some(&source), Some(path))
                );
                return;
            }

            let lints = Linter::new(Some(path.to_string())).lint(&program);
            if !lints.is_empty() {
                println!("{}", render_diagnostics(&lints, Some(&source), Some(path)));
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

fn show_cache_info(path: &str) {
    let cache = BytecodeCache::new(Path::new("target").join("flux"));
    let source = match fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) => {
            eprintln!("Error reading {}: {}", path, e);
            return;
        }
    };
    let source_hash = hash_bytes(source.as_bytes());
    let info = cache.inspect(Path::new(path), &source_hash);
    match info {
        Some(info) => {
            println!("cache file: {}", info.cache_path.display());
            println!("format version: {}", info.format_version);
            println!("compiler version: {}", info.compiler_version);
            println!("source hash: {}", hex_string(&info.source_hash));
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
            println!("source hash: {}", hex_string(&info.source_hash));
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
