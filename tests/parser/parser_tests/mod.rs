use flux::diagnostics::position::Position;
use flux::syntax::{
    expression::{Expression, Pattern},
    interner::Interner,
    lexer::Lexer,
    parser::Parser,
    program::Program,
    statement::Statement,
};

fn parse(input: &str) -> (Program, Interner) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "Parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    (program, interner)
}

mod commas_and_tuples;
mod lambdas;
mod literals;
mod recovery_fixtures;
mod semicolons_and_messages;
mod statements;
