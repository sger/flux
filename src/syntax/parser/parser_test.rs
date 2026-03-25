use crate::diagnostics::{missing_comma, unclosed_delimiter, unexpected_token};
use crate::syntax::{
    expression::Expression, interner::Interner, lexer::Lexer, parser::Parser, program::Program,
    statement::Statement, token::Token, token_type::TokenType,
};

use super::{
    RecoveryBoundary, is_pascal_case_ident, is_structural_parse_diagnostic_code, is_uppercase_ident,
};

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

fn parse_with_errors(input: &str) -> (Program, Parser) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    (program, parser)
}

#[test]
fn requested_recovery_boundary_is_consumed_once() {
    let lexer = Lexer::new("let x = 1");
    let mut parser = Parser::new(lexer);

    parser.request_recovery_boundary(RecoveryBoundary::Statement);
    assert_eq!(
        parser.take_requested_recovery_boundary(),
        Some(RecoveryBoundary::Statement)
    );
    assert_eq!(parser.take_requested_recovery_boundary(), None);
}

#[test]
fn structural_parser_diagnostics_are_classified_centrally() {
    assert!(is_structural_parse_diagnostic_code(Some("E034")));
    assert!(is_structural_parse_diagnostic_code(Some("E071")));
    assert!(is_structural_parse_diagnostic_code(Some("E076")));
    assert!(!is_structural_parse_diagnostic_code(Some("E030")));
    assert!(!is_structural_parse_diagnostic_code(Some("E073")));
}

#[test]
fn structural_diagnostic_starts_suppression_and_recovery_clears_it() {
    let lexer = Lexer::new("let x = 1");
    let mut parser = Parser::new(lexer);
    parser.begin_statement_recovery();

    assert!(parser.push_parser_diagnostic(unclosed_delimiter(
        crate::diagnostics::position::Span::new(
            parser.current_token.position,
            parser.current_token.position
        ),
        "(",
        ")",
        None,
    )));
    assert!(parser.recovery_state.structural_root().is_some());
    assert!(!parser.push_parser_diagnostic(missing_comma(
        parser.current_token.span(),
        "arguments",
        "`f(a, b)`",
    )));

    parser.request_recovery_boundary(RecoveryBoundary::Statement);
    parser.synchronize_recovery_boundary(RecoveryBoundary::Statement);

    assert!(parser.recovery_state.structural_root().is_none());
    assert!(parser.push_parser_diagnostic(unexpected_token(
        parser.current_token.span(),
        "non-structural followup after recovery",
    )));
}

#[test]
fn single_parser_context_keeps_single_breadcrumb() {
    let lexer = Lexer::new("let x = 1");
    let mut parser = Parser::new(lexer);
    let _context = parser.enter_parser_context(super::ParserContext::Function("main".to_string()));

    assert_eq!(
        parser.current_parser_breadcrumb().as_deref(),
        Some("function `main`")
    );
}

#[test]
fn nested_parser_contexts_render_outer_to_inner_chain() {
    let lexer = Lexer::new("let x = 1");
    let mut parser = Parser::new(lexer);
    let _fn_context =
        parser.enter_parser_context(super::ParserContext::Function("main".to_string()));
    let _lambda_context = parser.enter_parser_context(super::ParserContext::Lambda);
    let _match_context = parser.enter_parser_context(super::ParserContext::MatchExpression);

    assert_eq!(
        parser.current_parser_breadcrumb().as_deref(),
        Some("function `main` > lambda expression > `match` expression")
    );
}

#[test]
fn deep_parser_contexts_truncate_middle_of_breadcrumb_chain() {
    let lexer = Lexer::new("let x = 1");
    let mut parser = Parser::new(lexer);
    let _module_context =
        parser.enter_parser_context(super::ParserContext::Module("Outer".to_string()));
    let _fn_context =
        parser.enter_parser_context(super::ParserContext::Function("main".to_string()));
    let _lambda_context = parser.enter_parser_context(super::ParserContext::Lambda);
    let _match_context = parser.enter_parser_context(super::ParserContext::MatchExpression);

    assert_eq!(
        parser.current_parser_breadcrumb().as_deref(),
        Some("module `Outer` > ... > lambda expression > `match` expression")
    );
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
fn parses_import_exposing_all() {
    let (program, interner) = parse_ok("import Math exposing (..)");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Import {
            name,
            alias,
            except,
            exposing,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "Math");
            assert!(alias.is_none());
            assert!(except.is_empty());
            assert_eq!(*exposing, crate::syntax::statement::ImportExposing::All);
        }
        _ => panic!("expected import statement"),
    }
}

#[test]
fn parses_import_exposing_selective() {
    let (program, interner) = parse_ok("import Math exposing (square, cube)");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Import {
            name,
            exposing,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "Math");
            match exposing {
                crate::syntax::statement::ImportExposing::Names(names) => {
                    let resolved: Vec<&str> =
                        names.iter().map(|n| interner.resolve(*n)).collect();
                    assert_eq!(resolved, vec!["square", "cube"]);
                }
                _ => panic!("expected Names exposing"),
            }
        }
        _ => panic!("expected import statement"),
    }
}

#[test]
fn parses_import_alias_with_exposing() {
    let (program, interner) = parse_ok("import Math as M exposing (square)");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Import {
            name,
            alias,
            exposing,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "Math");
            assert_eq!(alias.map(|a| interner.resolve(a)), Some("M"));
            match exposing {
                crate::syntax::statement::ImportExposing::Names(names) => {
                    let resolved: Vec<&str> =
                        names.iter().map(|n| interner.resolve(*n)).collect();
                    assert_eq!(resolved, vec!["square"]);
                }
                _ => panic!("expected Names exposing"),
            }
        }
        _ => panic!("expected import statement"),
    }
}

#[test]
fn parses_import_exposing_empty_list() {
    let (program, _interner) = parse_ok("import Math exposing ()");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Import { exposing, .. } => {
            assert_eq!(
                *exposing,
                crate::syntax::statement::ImportExposing::Names(vec![])
            );
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
fn parses_public_function_statement() {
    let (program, interner) = parse_ok("public fn add(a: Int, b: Int) -> Int { a + b }");
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Function {
            is_public, name, ..
        } => {
            assert!(*is_public);
            assert_eq!(interner.resolve(*name), "add");
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn parses_private_function_statement_by_default() {
    let (program, interner) = parse_ok("fn helper(x: Int) -> Int { x }");
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Function {
            is_public, name, ..
        } => {
            assert!(!*is_public);
            assert_eq!(interner.resolve(*name), "helper");
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn parses_module_with_public_and_private_functions() {
    let (program, _interner) = parse_ok(
        "module Math { public fn add(x: Int, y: Int) -> Int { x + y } fn sub(x: Int, y: Int) -> Int { x - y } }",
    );
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Module { body, .. } => {
            assert_eq!(body.statements.len(), 2);
            match &body.statements[0] {
                Statement::Function { is_public, .. } => assert!(*is_public),
                _ => panic!("expected public function statement"),
            }
            match &body.statements[1] {
                Statement::Function { is_public, .. } => assert!(!*is_public),
                _ => panic!("expected private function statement"),
            }
        }
        _ => panic!("expected module statement"),
    }
}

#[test]
fn parses_typed_function_signature_with_effect_row_ops() {
    let (program, interner) = parse_ok("fn run() -> Int with IO + Console - Console, Time { 1 }");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Function { effects, .. } => {
            assert_eq!(
                effects
                    .iter()
                    .map(|e| e.display_with(&interner))
                    .collect::<Vec<_>>(),
                vec!["IO + Console - Console".to_string(), "Time".to_string()]
            );
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn parses_open_row_tail_only_effect() {
    let (program, interner) = parse_ok("fn run() -> Int with |e { 1 }");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Function { effects, .. } => {
            assert_eq!(
                effects
                    .iter()
                    .map(|e| e.display_with(&interner))
                    .collect::<Vec<_>>(),
                vec!["|e".to_string()]
            );
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn parses_function_type_annotation_with_effect_row_ops() {
    let (program, interner) =
        parse_ok("let f: (Int) -> Int with IO | e - Console = \\(x: Int) -> x;");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Let {
            type_annotation: Some(ty),
            ..
        } => {
            assert_eq!(
                ty.display_with(&interner),
                "Int -> Int with IO + |e - Console"
            );
        }
        _ => panic!("expected typed let statement"),
    }
}

#[test]
fn rejects_implicit_row_variable_syntax() {
    let (_program, parser) = parse_with_errors("fn f() -> Int with e + IO { 1 }");
    assert!(
        !parser.errors.is_empty(),
        "expected parser error for implicit row variable syntax"
    );

    let renderer = parser
        .errors
        .iter()
        .map(|d| d.message().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        renderer.contains("Implicit row variables"),
        "expected implicit-row-variable parser diagnostic, got: {renderer}"
    );
}

#[test]
fn rejects_uppercase_row_tail_variable() {
    let (_program, parser) = parse_with_errors("fn f() -> Int with IO | E { 1 }");
    assert!(
        !parser.errors.is_empty(),
        "expected parser error for upppercase row tail variable"
    );

    let renderer = parser
        .errors
        .iter()
        .map(|d| d.message().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        renderer.contains("tail variables must be lowercase"),
        "expected lowercase-tail parser diagnostic, got: {renderer}"
    );
}

#[test]
fn rejects_missing_row_tail_variable() {
    let (_program, parser) = parse_with_errors("fn f() -> Int with IO | { 1 }");
    assert!(
        !parser.errors.is_empty(),
        "expected parser error for missing row tail variable"
    );

    let renderer = parser
        .errors
        .iter()
        .map(|d| d.message().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        renderer.contains("Expected row variable name after `|`"),
        "expected missing-row-tail parser diagnostic, got: {renderer}"
    );
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

#[test]
fn parses_fip_annotated_function() {
    let (program, _) = parse_ok("@fip fn alloc(x) { Some(x) }");
    match &program.statements[0] {
        Statement::Function { fip, .. } => {
            assert_eq!(*fip, Some(crate::syntax::statement::FipAnnotation::Fip));
        }
        _ => panic!("expected function"),
    }
}

#[test]
fn parses_fbip_annotated_function() {
    let (program, _) = parse_ok("@fbip fn bounded(x) { x }");
    match &program.statements[0] {
        Statement::Function { fip, .. } => {
            assert_eq!(*fip, Some(crate::syntax::statement::FipAnnotation::Fbip));
        }
        _ => panic!("expected function"),
    }
}

#[test]
fn rejects_unknown_function_annotation() {
    let (program, parser) = parse_with_errors("@fi fn bad() { 1 }\nlet ok = 1;");
    assert!(
        parser.errors.iter().any(|d| d
            .message()
            .unwrap_or("")
            .contains("Unknown annotation `@fi`")),
        "expected unknown function annotation diagnostic, got: {:?}",
        parser.errors
    );
    assert!(
        parser
            .errors
            .iter()
            .any(|d| d.display_title() == Some("Unknown Function Annotation")),
        "expected Unknown Function Annotation title, got: {:?}",
        parser.errors
    );
    assert!(
        program
            .statements
            .iter()
            .any(|stmt| matches!(stmt, Statement::Let { .. })),
        "expected recovery to keep parsing follow-up statements"
    );
}

#[test]
fn rejects_unknown_function_annotation_generically() {
    let (_program, parser) = parse_with_errors("@foo fn bad() { 1 }");
    assert!(
        parser.errors.iter().any(|d| d
            .message()
            .unwrap_or("")
            .contains("Unknown annotation `@foo`")),
        "expected unknown function annotation diagnostic, got: {:?}",
        parser.errors
    );
}

#[test]
fn rejects_malformed_annotated_function_declaration() {
    let (_program, parser) = parse_with_errors("@fip let x = 1");
    assert!(
        parser.errors.iter().any(|d| {
            d.display_title() == Some("Malformed Annotated Function")
                && d.message()
                    .unwrap_or("")
                    .contains("must be followed by `fn`")
        }),
        "expected malformed annotated function diagnostic, got: {:?}",
        parser.errors
    );
}

#[test]
fn parses_type_adt_sugar_simple() {
    let (program, interner) = parse_ok("type Shape = Circle(Float) | Rect(Float, Float)");
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Data {
            name,
            type_params,
            variants,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "Shape");
            assert!(type_params.is_empty());
            assert_eq!(variants.len(), 2);
            assert_eq!(interner.resolve(variants[0].name), "Circle");
            assert_eq!(variants[0].fields.len(), 1);
            assert_eq!(interner.resolve(variants[1].name), "Rect");
            assert_eq!(variants[1].fields.len(), 2);
        }
        _ => panic!("expected desugared data statement"),
    }
}

#[test]
fn parses_type_adt_sugar_generic() {
    let (program, interner) = parse_ok("type Result<T, E> = Ok(T) | Err(E)");
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Data {
            name,
            type_params,
            variants,
            ..
        } => {
            assert_eq!(interner.resolve(*name), "Result");
            assert_eq!(type_params.len(), 2);
            assert_eq!(interner.resolve(type_params[0]), "T");
            assert_eq!(interner.resolve(type_params[1]), "E");
            assert_eq!(variants.len(), 2);
        }
        _ => panic!("expected desugared data statement"),
    }
}

#[test]
fn parses_type_adt_sugar_inside_module() {
    let (program, interner) = parse_ok("module M { type Local = A | B }");
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Module { body, .. } => {
            assert_eq!(body.statements.len(), 1);
            match &body.statements[0] {
                Statement::Data { name, variants, .. } => {
                    assert_eq!(interner.resolve(*name), "Local");
                    assert_eq!(variants.len(), 2);
                    assert_eq!(interner.resolve(variants[0].name), "A");
                    assert_eq!(interner.resolve(variants[1].name), "B");
                }
                _ => panic!("expected module data statement"),
            }
        }
        _ => panic!("expected module"),
    }
}

#[test]
fn type_adt_sugar_missing_assign_reports_error() {
    let (_program, parser) = parse_with_errors("type Shape Circle(Float) | Rect(Float, Float)");
    assert!(
        !parser.errors.is_empty(),
        "expected parser errors for missing '='"
    );
}

#[test]
fn type_adt_sugar_trailing_bar_reports_error() {
    let (_program, parser) = parse_with_errors("type Shape = Circle(Float) |");
    assert!(
        !parser.errors.is_empty(),
        "expected parser errors for trailing '|'"
    );
}

#[test]
fn missing_open_brace_reports_contextual_error() {
    let (_program, parser) = parse_with_errors("fn add(a: Int, b: Int) -> Int\n    a + b\n");
    assert!(
        !parser.errors.is_empty(),
        "expected parser error for missing opening brace"
    );
    let msg = parser.errors[0].message.as_deref().unwrap_or("");
    assert!(
        msg.contains("This function body needs to start with `{`."),
        "expected contextual brace error, got: {msg}"
    );
}

#[test]
fn missing_open_brace_mentions_function_name() {
    let (_program, parser) = parse_with_errors("fn distance_tag(p: Int) -> String\n    p\n");
    assert!(!parser.errors.is_empty());
    // The diagnostic should reference the function name in a label
    let has_fn_name = parser.errors[0]
        .labels
        .iter()
        .any(|l| l.text.contains("distance_tag"));
    assert!(has_fn_name, "expected label mentioning function name");
}

#[test]
fn missing_close_brace_reports_unclosed_delimiter() {
    let (_program, parser) = parse_with_errors("fn foo() {\n    1 + 2\n");
    assert!(
        !parser.errors.is_empty(),
        "expected parser error for missing closing brace"
    );
    let code = parser.errors[0].code.as_deref().unwrap_or("");
    assert_eq!(code, "E076", "expected UNCLOSED_DELIMITER error code");
}
