//! Proposal 0201: a `(` or `[` that starts a line begins a new expression.

use super::*;

fn function_body(program: &Program) -> &[Statement] {
    match &program.statements[0] {
        Statement::Function { body, .. } => &body.statements,
        other => panic!("expected a function, got {other:?}"),
    }
}

fn let_value(statement: &Statement) -> &Expression {
    match statement {
        Statement::Let { value, .. } => value,
        other => panic!("expected a let, got {other:?}"),
    }
}

fn expression_of(statement: &Statement) -> &Expression {
    match statement {
        Statement::Expression { expression, .. } => expression,
        other => panic!("expected an expression statement, got {other:?}"),
    }
}

fn parse_errors(input: &str) -> usize {
    let mut parser = Parser::new(Lexer::new(input));
    let _ = parser.parse_program();
    parser.errors.len()
}

#[test]
fn paren_led_line_after_let_is_the_block_value() {
    let (program, _) = parse("fn area(w: Int, h: Int) -> Int {\n    let a = w * h\n    (a + 1)\n}");
    let body = function_body(&program);
    assert_eq!(body.len(), 2, "{body:?}");
    assert!(matches!(let_value(&body[0]), Expression::Infix { .. }));
    assert!(matches!(expression_of(&body[1]), Expression::Infix { .. }));
}

#[test]
fn paren_led_line_after_perform_is_the_block_value() {
    let source = "fn both_ops() -> Int with E {\n    let x: Int = perform E.a(1)\n    let y: Int = perform E.b(1)\n    ((x * 1000) + y)\n}";
    let (program, _) = parse(source);
    let body = function_body(&program);
    assert_eq!(body.len(), 3, "{body:?}");
    assert!(matches!(let_value(&body[1]), Expression::Perform { .. }));
    assert!(matches!(expression_of(&body[2]), Expression::Infix { .. }));
}

#[test]
fn bracket_led_line_after_let_is_a_list_literal() {
    let (program, _) = parse("let xs = ys\n[1, 2]");
    assert_eq!(program.statements.len(), 2, "{:?}", program.statements);
    assert!(matches!(let_value(&program.statements[0]), Expression::Identifier { .. }));
    assert!(matches!(expression_of(&program.statements[1]), Expression::ListLiteral { .. }));
}

#[test]
fn paren_led_line_after_call_statement_is_a_new_statement() {
    let (program, _) = parse("print(1)\n(2)");
    assert_eq!(program.statements.len(), 2, "{:?}", program.statements);
}

#[test]
fn paren_led_line_inside_call_arguments_needs_a_comma() {
    // The rule holds inside delimiters too: `bar` ends, and `(1)` is a
    // second argument with no comma before it.
    assert!(parse_errors("foo(\n    bar\n    (1)\n)") > 0);
}

#[test]
fn curried_call_and_nested_index_on_one_line_still_continue() {
    let (program, _) = parse("f(1)(2)\nxs[0][1]");
    assert_eq!(program.statements.len(), 2, "{:?}", program.statements);
    match expression_of(&program.statements[0]) {
        Expression::Call { function, .. } => {
            assert!(matches!(function.as_ref(), Expression::Call { .. }))
        }
        other => panic!("expected a call, got {other:?}"),
    }
    match expression_of(&program.statements[1]) {
        Expression::Index { left, .. } => {
            assert!(matches!(left.as_ref(), Expression::Index { .. }))
        }
        other => panic!("expected an index, got {other:?}"),
    }
}

#[test]
fn call_and_index_opened_on_the_callee_line_span_lines() {
    let (program, _) =
        parse("let total = sum_all(\n    first,\n    second,\n)\nlet item = table[\n    0\n]");
    assert_eq!(program.statements.len(), 2, "{:?}", program.statements);
    assert!(matches!(let_value(&program.statements[0]), Expression::Call { .. }));
    assert!(matches!(let_value(&program.statements[1]), Expression::Index { .. }));
}

#[test]
fn call_after_a_multi_line_string_argument_still_continues() {
    let (program, _) = parse("let g = f(\"\"\"\na\nb\n\"\"\")(1)");
    assert_eq!(program.statements.len(), 1, "{:?}", program.statements);
    match let_value(&program.statements[0]) {
        Expression::Call { function, .. } => {
            assert!(matches!(function.as_ref(), Expression::Call { .. }))
        }
        other => panic!("expected a call, got {other:?}"),
    }
}

#[test]
fn operator_led_lines_still_continue() {
    let source = "let r = xs\n    |> map(f)\n    |> filter(g)\nlet ok = a\n    && b\n    || c\nlet n = a\n    + b\nlet m = Array\n    .range(1, 3)";
    let (program, _) = parse(source);
    assert_eq!(program.statements.len(), 4, "{:?}", program.statements);
    for statement in &program.statements {
        assert!(!matches!(let_value(statement), Expression::Identifier { .. }), "{statement:?}");
    }
}

#[test]
fn perform_and_handle_read_their_own_paren_across_a_line_break() {
    let source = "fn g() -> Int with E {\n    perform E.a\n    (1)\n}\nlet h = g()\n    handle State\n    (0) {\n        get(resume, s) -> resume(s, s)\n    }";
    let (program, _) = parse(source);
    assert_eq!(program.statements.len(), 2, "{:?}", program.statements);
    let body = function_body(&program);
    assert_eq!(body.len(), 1, "{body:?}");
    assert!(matches!(expression_of(&body[0]), Expression::Perform { .. }));
    assert!(matches!(let_value(&program.statements[1]), Expression::Handle { .. }));
}
