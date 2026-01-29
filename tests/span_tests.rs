use flux::frontend::{lexer::Lexer, parser::Parser, position::Position};

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
    assert_eq!(span.start, Position::new(1, 1));
    assert_eq!(span.end, Position::new(1, 6));
}
