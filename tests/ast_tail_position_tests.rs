use flux::{
    ast::tail_position::find_tail_calls,
    syntax::{lexer::Lexer, parser::Parser, program::Program},
};

fn parse(input: &str) -> Program {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "Parser errors: {:?}",
        parser.errors
    );
    program
}

#[test]
fn simple_tail_call() {
    let program = parse("fun f(n) { f(n - 1); }");
    let tail_calls = find_tail_calls(&program);
    assert_eq!(tail_calls.len(), 1);
}

#[test]
fn non_tail_call_in_infix() {
    // f(n-1) + 1 — the call result is used in addition, not in tail position
    let program = parse("fun f(n) { f(n - 1) + 1; }");
    let tail_calls = find_tail_calls(&program);
    assert!(
        tail_calls.is_empty(),
        "Expected no tail calls, got: {}",
        tail_calls.len()
    );
}

#[test]
fn tail_call_in_if_branches() {
    let program = parse(
        r#"
        fun f(n) {
            if n == 0 {
                0;
            } else {
                f(n - 1);
            };
        }
    "#,
    );
    let tail_calls = find_tail_calls(&program);
    // f(n-1) in else branch is a tail call
    assert_eq!(tail_calls.len(), 1);
}

#[test]
fn tail_call_in_match_arm() {
    let program = parse(
        r#"
        fun f(n) {
            match n {
                0 -> 0,
                _ -> f(n - 1),
            }
        }
    "#,
    );
    let tail_calls = find_tail_calls(&program);
    assert_eq!(tail_calls.len(), 1);
}

#[test]
fn tail_call_via_return() {
    let program = parse(
        r#"
        fun f(n) {
            return f(n - 1);
        }
    "#,
    );
    let tail_calls = find_tail_calls(&program);
    assert_eq!(tail_calls.len(), 1);
}

#[test]
fn non_tail_call_in_let_binding() {
    // The call result is bound to x, not in tail position
    let program = parse(
        r#"
        fun f(n) {
            let x = f(n - 1);
            x;
        }
    "#,
    );
    let tail_calls = find_tail_calls(&program);
    assert!(
        tail_calls.is_empty(),
        "Expected no tail calls, got: {}",
        tail_calls.len()
    );
}

#[test]
fn multiple_tail_calls_in_branches() {
    let program = parse(
        r#"
        fun f(n) {
            if n == 0 {
                g(0);
            } else {
                f(n - 1);
            };
        }
    "#,
    );
    let tail_calls = find_tail_calls(&program);
    // Both g(0) and f(n-1) are in tail position
    assert_eq!(tail_calls.len(), 2);
}

#[test]
fn nested_if_tail_propagation() {
    let program = parse(
        r#"
        fun f(n) {
            if n == 0 {
                0;
            } else {
                if n == 1 {
                    1;
                } else {
                    f(n - 2);
                };
            };
        }
    "#,
    );
    let tail_calls = find_tail_calls(&program);
    assert_eq!(tail_calls.len(), 1);
}

#[test]
fn call_in_condition_not_tail() {
    // The call is the condition of the if, not in tail position
    let program = parse(
        r#"
        fun f(n) {
            if g(n) {
                1;
            } else {
                0;
            };
        }
    "#,
    );
    let tail_calls = find_tail_calls(&program);
    assert!(
        tail_calls.is_empty(),
        "Expected no tail calls, got: {}",
        tail_calls.len()
    );
}

#[test]
fn lambda_has_own_tail_context() {
    // The lambda body has its own tail context
    let program = parse(
        r#"
        let f = \n -> g(n);
    "#,
    );
    let tail_calls = find_tail_calls(&program);
    assert_eq!(tail_calls.len(), 1);
}

#[test]
fn non_last_statement_not_tail() {
    // First expression statement is not the last one — not in tail position
    let program = parse(
        r#"
        fun f(n) {
            g(n);
            0;
        }
    "#,
    );
    let tail_calls = find_tail_calls(&program);
    assert!(
        tail_calls.is_empty(),
        "Expected no tail calls, got: {}",
        tail_calls.len()
    );
}

#[test]
fn call_in_argument_not_tail() {
    // g(n) is an argument to f, not in tail position itself
    let program = parse(
        r#"
        fun f(n) {
            f(g(n));
        }
    "#,
    );
    let tail_calls = find_tail_calls(&program);
    // f(g(n)) is a tail call, but g(n) is not
    assert_eq!(tail_calls.len(), 1);
}
