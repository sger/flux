use std::{env, fs, path::Path};

use flux::{
    bytecode::{compiler::Compiler, op_code::disassemble},
    frontend::{diagnostic::render_diagnostics, lexer::Lexer, parser::Parser},
    runtime::vm::VM,
};

mod bytecode;
mod frontend;
mod runtime;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        return;
    }

    match args[1].as_str() {
        "run" => {
            if args.len() < 3 {
                eprintln!("Usage: flux run <file.flx>");
                return;
            }

            if !is_flx_file(&args[2]) {
                eprintln!("Error: file must have .flx extension: {}", args[2]);
                return;
            }
            run_file(&args[2])
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
        _ => {}
    }
}

fn run_file(path: &str) {
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

            let mut vm = VM::new(compiler.bytecode());
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
