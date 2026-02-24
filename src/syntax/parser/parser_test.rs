use crate::syntax::{
    expression::Expression, interner::Interner, lexer::Lexer, parser::Parser, program::Program,
    statement::Statement, token::Token, token_type::TokenType,
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
        Statement::Import {
            name,
            alias,
            except,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "Foo.Bar");
            assert_eq!(alias.map(|a| interner.resolve(a)), Some("Baz"));
            assert!(except.is_empty());
        }
        _ => panic!("expected import statement"),
    }
}

#[test]
fn parses_import_without_alias() {
    let (program, interner) = parse_ok("import Foo");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Import {
            name,
            alias,
            except,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "Foo");
            assert!(alias.is_none());
            assert!(except.is_empty());
        }
        _ => panic!("expected import statement"),
    }
}

#[test]
fn parses_import_base_with_except() {
    let (program, interner) = parse_ok("import Base except [print, len]");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Import {
            name,
            alias,
            except,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "Base");
            assert!(alias.is_none());
            let names: Vec<&str> = except.iter().map(|sym| interner.resolve(*sym)).collect();
            assert_eq!(names, vec!["print", "len"]);
        }
        _ => panic!("expected import statement"),
    }
}

#[test]
fn parses_import_non_base_with_except() {
    let (program, interner) = parse_ok("import Foo except [bar]");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Import {
            name,
            alias,
            except,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "Foo");
            assert!(alias.is_none());
            let names: Vec<&str> = except.iter().map(|sym| interner.resolve(*sym)).collect();
            assert_eq!(names, vec!["bar"]);
        }
        _ => panic!("expected import statement"),
    }
}

#[test]
fn fn_keyword_parses_function_statement() {
    let (program, interner) = parse_ok("fn add() { }");
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Function { name, .. } => assert_eq!(interner.resolve(*name), "add"),
        _ => panic!("expected function statement"),
    }
}

#[test]
fn invalid_function_keyword_mentions_fn() {
    let lexer = Lexer::new("function add() { }");
    let mut parser = Parser::new(lexer);
    let _ = parser.parse_program();
    assert!(!parser.errors.is_empty(), "expected parser error");
    let err = &parser.errors[0];
    assert!(
        err.message()
            .is_some_and(|m| m.contains("Flux uses `fn` for function declarations"))
    );
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

#[test]
fn parses_typed_let_statement() {
    let (program, interner) = parse_ok("let x: Int = 1;");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Let {
            name,
            type_annotation: Some(ty),
            ..
        } => {
            assert_eq!(interner.resolve(*name), "x");
            assert_eq!(ty.display_with(&interner), "Int");
        }
        _ => panic!("expected typed let statement"),
    }
}

#[test]
fn parses_typed_function_signature_with_effects() {
    let (program, interner) = parse_ok("fn add(a: Int, b: Int) -> Int with IO, Time { a + b }");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Function {
            name,
            parameters,
            parameter_types,
            return_type,
            effects,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "add");
            assert_eq!(parameters.len(), 2);
            assert_eq!(
                parameter_types
                    .iter()
                    .map(|ty| ty.as_ref().map(|t| t.display_with(&interner)))
                    .collect::<Vec<_>>(),
                vec![Some("Int".to_string()), Some("Int".to_string())]
            );
            assert_eq!(
                return_type
                    .as_ref()
                    .map(|ty| ty.display_with(&interner))
                    .as_deref(),
                Some("Int")
            );
            assert_eq!(
                effects
                    .iter()
                    .map(|e| e.display_with(&interner))
                    .collect::<Vec<_>>(),
                vec!["IO".to_string(), "Time".to_string()]
            );
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn parses_lambda_parameter_annotation() {
    let (program, interner) = parse_ok("let inc = \\(x: Int) -> x + 1;");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Let { value, .. } => match value {
            Expression::Function {
                parameter_types, ..
            } => {
                assert_eq!(parameter_types.len(), 1);
                assert_eq!(
                    parameter_types[0]
                        .as_ref()
                        .map(|t| t.display_with(&interner))
                        .as_deref(),
                    Some("Int")
                );
            }
            _ => panic!("expected function expression"),
        },
        _ => panic!("expected let statement"),
    }
}

#[test]
fn parses_generic_function_one_type_param() {
    let (program, interner) = parse_ok("fn identity<T>(x: T) -> T { x }");
    match &program.statements[0] {
        Statement::Function {
            name,
            type_params,
            parameters,
            parameter_types,
            return_type,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "identity");
            assert_eq!(type_params.len(), 1);
            assert_eq!(interner.resolve(type_params[0]), "T");
            assert_eq!(parameters.len(), 1);
            assert_eq!(
                parameter_types[0]
                    .as_ref()
                    .map(|t| t.display_with(&interner))
                    .as_deref(),
                Some("T")
            );
            assert_eq!(
                return_type
                    .as_ref()
                    .map(|t| t.display_with(&interner))
                    .as_deref(),
                Some("T")
            );
        }
        _ => panic!("expected generic function"),
    }
}

#[test]
fn parses_generic_function_two_type_params() {
    let (program, interner) = parse_ok("fn pair<A, B>(a: A, b: B) -> (A, B) { (a, b) }");
    match &program.statements[0] {
        Statement::Function {
            type_params,
            parameters,
            ..
        } => {
            assert_eq!(type_params.len(), 2);
            assert_eq!(interner.resolve(type_params[0]), "A");
            assert_eq!(interner.resolve(type_params[1]), "B");
            assert_eq!(parameters.len(), 2);
        }
        _ => panic!("expected generic function"),
    }
}

#[test]
fn parses_non_generic_function_has_empty_type_params() {
    let (program, _) = parse_ok("fn f(x: Int) -> Int { x }");
    match &program.statements[0] {
        Statement::Function { type_params, .. } => {
            assert!(type_params.is_empty());
        }
        _ => panic!("expected function"),
    }
}
