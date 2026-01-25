use crate::frontend::lexer::Lexer;

mod frontend;

fn main() {
    // Minimal usage so the module is not dead-code.
    // Replace later with your real REPL/CLI.
    let mut lexer = Lexer::new("let x = 1;");
    let _ = lexer.next_token();
}
