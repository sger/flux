use crate::frontend::{
    interner::Interner, lexer::Lexer, parser::Parser, program::Program, statement::Statement,
    token::Token, token_type::TokenType,
};

use super::{is_pascal_case_ident, is_uppercase_ident};

fn parse_ok(input: &str) -> (Program, Interner) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );

    let interner = parser.take_interner();
    (program, interner)
}

#[test]
fn parses_module_statement() {
    let (program, interner) = parse_ok("module Foo { let x = 1; }");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Module { name, body, .. } => {
            assert_eq!(interner.resolve(*name), "Foo");
            assert_eq!(body.statements.len(), 1);
        }
        _ => panic!("expected module statement"),
    }
}

#[test]
fn parses_import_with_alias() {
    let (program, interner) = parse_ok("import Foo.Bar as Baz");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Import { name, alias, .. } => {
            assert_eq!(interner.resolve(*name), "Foo.Bar");
            assert_eq!(alias.map(|a| interner.resolve(a)), Some("Baz"));
        }
        _ => panic!("expected import statement"),
    }
}

#[test]
fn parses_import_without_alias() {
    let (program, interner) = parse_ok("import Foo");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Import { name, alias, .. } => {
            assert_eq!(interner.resolve(*name), "Foo");
            assert!(alias.is_none());
        }
        _ => panic!("expected import statement"),
    }
}

#[test]
fn invalid_keyword_reports_error() {
    let lexer = Lexer::new("fn add() { }");
    let mut parser = Parser::new(lexer);
    let _ = parser.parse_program();
    assert!(!parser.errors.is_empty());
}

#[test]
fn uppercase_and_pascal_case_helpers() {
    let upper = Token::new(TokenType::Ident, "Foo", 0, 0);
    let lower = Token::new(TokenType::Ident, "foo", 0, 0);
    let all_caps = Token::new(TokenType::Ident, "FOO", 0, 0);

    assert!(is_uppercase_ident(&upper));
    assert!(!is_uppercase_ident(&lower));

    assert!(is_pascal_case_ident(&upper));
    assert!(!is_pascal_case_ident(&all_caps));
}

#[test]
fn parse_program_span_covers_all_tokens() {
    let lexer = Lexer::new("let x = 1; let y = 2;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty());

    let span = program.span();
    assert_eq!(span.start.line, 1);
    assert!(span.end.line >= span.start.line);
}
