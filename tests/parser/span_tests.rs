use flux::diagnostics::position::Position;
use flux::syntax::{lexer::Lexer, parser::Parser};

#[test]
fn span_covers_simple_infix() {
    let source = "1 + 2;";
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let stmt = program.statements.first().expect("missing statement");
    let span = stmt.span();
    assert_eq!(span.start, Position::new(1, 0));
    assert_eq!(span.end, Position::new(1, 6));
}
