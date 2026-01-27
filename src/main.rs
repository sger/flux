use std::{env, fs};

use flux::{
    bytecode::compiler::Compiler,
    frontend::{lexer::Lexer, parser::Parser},
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

            run_file(&args[2])
        }
        "tokens" => {
            if args.len() < 3 {
                eprintln!("Usage: flux tokens <file.flx>");
                return;
            }
            show_tokens(&args[2]);
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
                for err in &parser.errors {
                    eprintln!("Parse error: {}", err);
                }
                return;
            }

            let mut compiler = Compiler::new_with_file_path(path);
            if let Err(err) = compiler.compile(&program) {
                eprintln!("{}", err);
                return;
            }

            println!("{:?}", compiler.bytecode());

            let mut vm = VM::new(compiler.bytecode());
            if let Err(err) = vm.run() {
                eprintln!("Runtime error: {}", err);
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
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
