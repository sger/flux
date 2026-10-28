use super::*;

const SEMICOLON_VERIFICATION_PROGRAM: &str = r#"fn f(a, b, c, d) {
    print(a);
    print(b);
    print(c);
    print(d);
}

print(1 2 3);         // missing commas between call arguments
[true false true];    // missing commas between array items
f(10, 20 30, 40);     // missing comma in the middle argument gap

let parsed_after_errors = "parser should still reach this statement";
print(parsed_after_errors);

print(1 2 3)          // also test without semicolon
let test = "this compiles"
let test2 = "this compiles";
"#;

// ========================================================================
// Regression tests for optional semicolons
// ========================================================================
// Current parser behavior:
// - Semicolons are optional for top-level statements.
// - Statements are separated by parser progress; semicolons are accepted but
//   not required for the cases covered below.
// See: examples/guide/semicolons.flx for comprehensive examples
// ========================================================================

#[test]
fn test_optional_semicolons_let_statements() {
    let (program, interner) = parse("let x = 5\nlet y = 10");
    assert_eq!(program.statements.len(), 2);

    match &program.statements[0] {
        Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "x"),
        _ => panic!("expected Let statement"),
    }
    match &program.statements[1] {
        Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "y"),
        _ => panic!("expected Let statement"),
    }
}

#[test]
fn test_optional_semicolons_expressions() {
    let (program, _interner) = parse("1 + 2\n3 + 4");
    assert_eq!(program.statements.len(), 2);
}

#[test]
fn test_optional_semicolons_function_calls() {
    let (program, _interner) = parse("print(\"Hello\")\nprint(\"World\")");
    assert_eq!(program.statements.len(), 2);
}

#[test]
fn test_top_level_call_with_and_without_semicolon_both_parse() {
    let (program, _interner) = parse("print(\"hi\")\nprint(\"there\");");
    assert_eq!(program.statements.len(), 2);
}

#[test]
fn test_top_level_let_with_and_without_semicolon_both_parse() {
    let (program, interner) = parse("let test = \"this compiles\"\nlet test2 = \"this compiles\";");
    assert_eq!(program.statements.len(), 2);
    match &program.statements[0] {
        Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "test"),
        _ => panic!("expected Let statement"),
    }
    match &program.statements[1] {
        Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "test2"),
        _ => panic!("expected Let statement"),
    }
}

#[test]
fn test_semicolon_verification_program_recovers_and_parses_late_statements() {
    let lexer = Lexer::new(SEMICOLON_VERIFICATION_PROGRAM);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let missing_comma_count = parser.errors.iter().filter(|d| d.code() == Some("E073")).count();
    assert_eq!(missing_comma_count, 7, "expected missing-comma diagnostics at all intended gaps");
    assert!(
        parser.errors.iter().all(|d| d.code() == Some("E073")),
        "verification program should avoid generic cascade diagnostics"
    );
    assert!(
        !parser
            .errors
            .iter()
            .any(|d| { d.message().is_some_and(|m| m.to_ascii_lowercase().contains("semicolon")) }),
        "expected no missing-semicolon diagnostics"
    );

    assert!(
            program.statements.iter().any(
                |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "parsed_after_errors")
            ),
            "expected recovery to keep parsing `parsed_after_errors` binding"
        );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "test")
        ),
        "expected recovery to keep parsing `test` binding without semicolon"
    );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "test2")
        ),
        "expected recovery to keep parsing `test2` binding with semicolon"
    );
    assert!(
            program.statements.iter().any(|stmt| {
                matches!(
                    stmt,
                    Statement::Expression {
                        expression: Expression::Call { function, arguments, .. },
                        ..
                    } if matches!(&**function, Expression::Identifier { name, .. } if interner.resolve(*name) == "print")
                        && arguments.len() == 1
                        && matches!(&arguments[0], Expression::Identifier { name, .. } if interner.resolve(*name) == "parsed_after_errors")
                )
            }),
            "expected recovery to keep parsing `print(parsed_after_errors)` call"
        );
}

#[test]
fn test_mixed_semicolons() {
    // Mix of statements with and without semicolons
    let (program, _interner) = parse("let x = 5;\nlet y = 10\nprint(x)\nprint(y);");
    assert_eq!(program.statements.len(), 4);
}

#[test]
fn test_optional_semicolons_return() {
    let (program, _interner) = parse("return 5\nreturn 10");
    assert_eq!(program.statements.len(), 2);

    match &program.statements[0] {
        Statement::Return { .. } => {}
        _ => panic!("expected Return statement"),
    }
}

#[test]
fn test_optional_semicolons_if_statements() {
    let (program, _interner) = parse("if (x > 0) { print(\"positive\") }\nprint(\"done\")");
    assert_eq!(program.statements.len(), 2);
}

#[test]
fn test_optional_semicolons_multiple_expressions() {
    let (program, _interner) = parse("1 + 2\n3 * 4\n5 - 6\n7 / 8");
    assert_eq!(program.statements.len(), 4);
}

#[test]
fn missing_colon_let_annotation_has_targeted_message_and_recovers() {
    let input = "let x Int = 1;\nlet y = 2;";
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(parser.errors.iter().any(|d| {
        d.message().is_some_and(|m| {
            m.contains("Missing `:` in let binding type annotation") && m.contains("`x: Type`")
        })
    }));
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "y")
        )
    );
}

#[test]
fn missing_colon_function_param_has_targeted_message() {
    let input = "fn f(x Int) -> Int { x }";
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let _program = parser.parse_program();

    assert!(parser.errors.iter().any(|d| {
        d.message().is_some_and(|m| m.contains("Missing `:` in function parameter type annotation"))
    }));
}

#[test]
fn missing_colon_lambda_param_has_targeted_message() {
    let input = "let g = \\x Int -> x;\nlet after = 1;";
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(parser.errors.iter().any(|d| {
        d.message().is_some_and(|m| m.contains("Missing `:` in lambda parameter type annotation"))
    }));
    assert!(program.statements.iter().any(
        |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "after")
    ));
}

#[test]
fn missing_colon_effect_op_has_targeted_message() {
    let input = "effect Console { print String -> Unit }";
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let _program = parser.parse_program();

    assert!(parser.errors.iter().any(|d| {
        d.message().is_some_and(|m| m.contains("Missing `:` in effect operation signature"))
    }));
}

#[test]
fn bodiless_effect_declares_phantom_label() {
    // Proposal 0161: `effect Name` (no body) declares a phantom effect label.
    let input = "effect Console\nfn main() { () }";
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(parser.errors.is_empty(), "unexpected errors: {:?}", parser.errors);
    assert!(program.statements.iter().any(|stmt| matches!(
        stmt,
        Statement::EffectDecl { name, ops, .. }
            if interner.resolve(*name) == "Console" && ops.is_empty()
    )));
}

#[test]
fn transparent_type_aliases_parse_without_breaking_effect_aliases() {
    let (program, interner) = parse(
        r#"
alias Async = <Suspend | Fork>
alias Stream<a> = () -> Option<a> with Async
alias AsyncFn<a, b, e> = (a) -> b with <Async | e>
public alias Bytes = String
"#,
    );

    assert!(matches!(program.statements[0], Statement::EffectAlias { .. }));
    assert!(matches!(
        &program.statements[1],
        Statement::TypeAlias(alias)
            if interner.resolve(alias.name) == "Stream"
                && alias.params.len() == 1
    ));
    assert!(matches!(
        &program.statements[2],
        Statement::TypeAlias(alias)
            if interner.resolve(alias.name) == "AsyncFn"
                && alias.params.len() == 3
    ));
    assert!(matches!(
        &program.statements[3],
        Statement::TypeAlias(alias)
            if alias.is_public && interner.resolve(alias.name) == "Bytes"
    ));
}

#[test]
fn parameterized_handle_parses_initializer_and_state_param_shape() {
    let (program, interner) = parse(
        r#"
let result = program() handle State(0) {
    get(resume, state) -> resume(state, state),
    set(resume, value, state) -> resume((), value)
};
"#,
    );

    let Statement::Let { value, .. } = &program.statements[0] else {
        panic!("expected let statement");
    };
    let Expression::Handle { expr, effect, parameter, arms, .. } = value else {
        panic!("expected handle expression, got {value:?}");
    };

    assert!(matches!(expr.as_ref(), Expression::Call { .. }));
    assert_eq!(interner.resolve(*effect), "State");
    assert!(matches!(parameter.as_deref(), Some(Expression::Integer { value: 0, .. })));
    assert_eq!(arms.len(), 2);
    assert_eq!(interner.resolve(arms[0].operation_name), "get");
    assert_eq!(arms[0].params.len(), 1);
    assert_eq!(interner.resolve(arms[0].params[0]), "state");
    assert_eq!(interner.resolve(arms[1].operation_name), "set");
    assert_eq!(arms[1].params.len(), 2);
    assert_eq!(interner.resolve(arms[1].params[0]), "value");
    assert_eq!(interner.resolve(arms[1].params[1]), "state");
}

#[test]
fn missing_function_return_arrow_has_targeted_message() {
    let input = "fn f() Int { 1 }\nlet ok = 1;";
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(parser.errors.iter().any(|d| {
        d.message().is_some_and(|m| m.contains("Missing `->` before function return type"))
    }));
    assert!(program.statements.iter().any(
        |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "ok")
    ));
}

#[test]
fn keyword_aliases_report_e030_with_targeted_suggestions() {
    let collect = |input: &str| -> Vec<String> {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();
        parser.errors.iter().filter_map(|d| d.message().map(ToString::to_string)).collect()
    };

    let def_msgs = collect("def foo() { 1 }");

    assert!(
        def_msgs.iter().any(|m| m.contains("Unknown keyword `def`")),
        "expected `def` alias diagnostic, got: {:#?}",
        def_msgs
    );

    let var_msgs = collect("var x = 1");
    assert!(
        var_msgs.iter().any(|m| m.contains("Unknown keyword `var`")),
        "expected `var` alias diagnostic, got: {:#?}",
        var_msgs
    );

    let case_msgs = collect("case 1 { 0 -> 0, _ -> 1 }");
    assert!(
        case_msgs.iter().any(|m| m.contains("Unknown keyword `case`")),
        "expected `case` alias diagnostic, got: {:#?}",
        case_msgs
    );
}

#[test]
fn elif_and_end_report_targeted_diagnostics() {
    let lexer = Lexer::new("if true { 1 } elif false { 2 } else { 3 }\nfn f() { 1 end }");
    let mut parser = Parser::new(lexer);
    let _program = parser.parse_program();

    let msgs: Vec<String> =
        parser.errors.iter().filter_map(|d| d.message().map(ToString::to_string)).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("Unknown keyword `elif`") || m.contains("Unknown keyword `elsif`")),
        "expected elif/elsif alias diagnostic, got: {:#?}",
        msgs
    );
    assert!(
        msgs.iter().any(|m| m.contains("`end` is not a keyword in Flux")),
        "expected end-keyword diagnostic, got: {:#?}",
        msgs
    );
}

#[test]
fn structural_context_messages_are_emitted() {
    let collect = |input: &str| -> Vec<String> {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();
        parser.errors.iter().filter_map(|d| d.message().map(ToString::to_string)).collect()
    };

    let fn_msgs = collect("fn foo -> Int { 1 }");
    assert!(
        fn_msgs.iter().any(|m| m.contains("parameter list after `foo`")),
        "expected missing-parameter-list message, got: {:#?}",
        fn_msgs
    );

    let if_msgs = collect("if true 1 else 2");
    assert!(
        if_msgs.iter().any(|m| m.contains("`if` branch needs to start with `{`")),
        "expected missing-if-brace message, got: {:#?}",
        if_msgs
    );

    let let_msgs = collect("let x 1");
    assert!(
        let_msgs.iter().any(|m| m.contains("Expected `=` after `let x`")),
        "expected missing-let-assign message, got: {:#?}",
        let_msgs
    );
}

#[test]
fn match_pipe_and_fat_arrow_emit_targeted_messages() {
    let lexer = Lexer::new(
        "match x { 0 -> 1 | 1 -> 2 }\nlet y = 3;\nmatch z { 0 => 1, _ -> 2 }\nlet q = 4;",
    );
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let msgs: Vec<String> =
        parser.errors.iter().filter_map(|d| d.message().map(ToString::to_string)).collect();
    assert!(
        msgs.iter().any(|m| m.contains("Match arms are separated by `,` in Flux, not `|`")),
        "expected `|` separator diagnostic, got: {:#?}",
        msgs
    );
    assert!(
        msgs.iter().any(|m| m.contains("needs `->`, not `=>`")),
        "expected fat-arrow diagnostic, got: {:#?}",
        msgs
    );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "y")
        ),
        "expected parser recovery after `|` separator diagnostic"
    );
}

#[test]
fn alias_words_as_identifiers_do_not_misfire_at_expression_positions() {
    let lexer = Lexer::new("let val = 1;\nlet end = 2;\nlet x = Foo.end(val);");
    let mut parser = Parser::new(lexer);
    let _program = parser.parse_program();

    let alias_diag_count = parser.errors.iter().filter(|d| d.code() == Some("E030")).count();
    assert_eq!(
        alias_diag_count, 0,
        "did not expect statement-keyword alias diagnostics for identifier usage"
    );
}
