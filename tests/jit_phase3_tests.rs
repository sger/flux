#![cfg(feature = "jit")]

use std::rc::Rc;

use flux::diagnostics::render_diagnostics;
use flux::jit::jit_compile_and_run;
use flux::runtime::value::Value;
use flux::syntax::{lexer::Lexer, parser::Parser};

fn run_jit(input: &str) -> Value {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    assert!(
        parser.errors.is_empty(),
        "{}",
        render_diagnostics(&parser.errors, Some(input), None)
    );
    jit_compile_and_run(&program, &interner).unwrap()
}

fn run_jit_err(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    assert!(
        parser.errors.is_empty(),
        "{}",
        render_diagnostics(&parser.errors, Some(input), None)
    );
    jit_compile_and_run(&program, &interner).unwrap_err()
}

#[test]
fn jit_named_function_call_works() {
    let result = run_jit(
        r#"
fn add(a, b) { a + b }
add(2, 3)
"#,
    );
    assert_eq!(result, Value::Integer(5));
}

#[test]
fn jit_function_reads_top_level_global_let() {
    let result = run_jit(
        r#"
let LIMIT = 100
fn add_limit(x) { x + LIMIT }
add_limit(23)
"#,
    );
    assert_eq!(result, Value::Integer(123));
}

#[test]
fn jit_recursive_return_works() {
    let result = run_jit(
        r#"
fn fib(n) {
    if n < 2 { return n }
    fib(n - 1) + fib(n - 2)
}
fib(6)
"#,
    );
    assert_eq!(result, Value::Integer(8));
}

#[test]
fn jit_function_literal_captures_outer_local() {
    let result = run_jit(
        r#"
let n = 2
let f = fn(x) { x + n }
f(40)
"#,
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn jit_local_function_statement_captures_outer_local() {
    let result = run_jit(
        r#"
fn outer(x) {
    fn addy(y) { y + x }
    addy(5)
}
outer(7)
"#,
    );
    assert_eq!(result, Value::Integer(12));
}

#[test]
fn jit_higher_order_map_with_literal_closure() {
    let result = run_jit(
        r#"
let n = 1
let ys = map(list(1, 2, 3), fn(x) { x + n })
to_array(ys)
"#,
    );
    assert_eq!(
        result,
        Value::Array(Rc::new(vec![
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4)
        ]))
    );
}

#[test]
fn jit_wrong_arity_reports_error() {
    let err = run_jit_err(
        r#"
fn add(a, b) { a + b }
add(1)
"#,
    );
    assert!(err.contains("wrong number of arguments"));
}

#[test]
fn jit_tail_recursive_return_works() {
    let result = run_jit(
        r#"
fn sum_to(n, acc) {
    if n == 0 { return acc }
    return sum_to(n - 1, acc + n)
}
sum_to(200, 0)
"#,
    );
    assert_eq!(result, Value::Integer(20100));
}

#[test]
fn jit_callable_via_if_expression_works() {
    let result = run_jit(
        r#"
fn add1(x) { x + 1 }
fn add2(x) { x + 2 }
let f = if true { add1 } else { add2 }
f(40)
"#,
    );
    assert_eq!(result, Value::Integer(41));
}

#[test]
fn jit_nested_closure_capture_chain() {
    let result = run_jit(
        r#"
let a = 3
let mk = fn(x) {
    fn(y) { x + y + a }
}
let f = mk(10)
f(5)
"#,
    );
    assert_eq!(result, Value::Integer(18));
}

#[test]
fn jit_local_recursive_function_statement_works() {
    let result = run_jit(
        r#"
fn outer(n) {
    fn loop(k, acc) {
        if k == 0 { return acc }
        return loop(k - 1, acc + k)
    }
    loop(n, 0)
}
outer(10)
"#,
    );
    assert_eq!(result, Value::Integer(55));
}

#[test]
fn jit_tail_recursive_without_explicit_return_works() {
    let result = run_jit(
        r#"
fn sum_to(n, acc) {
    if n == 0 { acc } else { sum_to(n - 1, acc + n) }
}
sum_to(200, 0)
"#,
    );
    assert_eq!(result, Value::Integer(20100));
}

#[test]
fn jit_builtin_value_callable_works() {
    let result = run_jit(
        r#"
let f = len
f("flux")
"#,
    );
    assert_eq!(result, Value::Integer(4));
}

#[test]
fn jit_named_function_value_for_higher_order_builtin_works() {
    let result = run_jit(
        r#"
fn inc(x) { x + 1 }
to_array(map(list(1, 2, 3), inc))
"#,
    );
    assert_eq!(
        result,
        Value::Array(Rc::new(vec![
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4)
        ]))
    );
}

#[test]
fn jit_match_arm_pattern_binding_can_be_captured() {
    let result = run_jit(
        r#"
match list(1, 2) {
    [h | t] -> (fn() { h + len(t) })(),
    _ -> 0
}
"#,
    );
    assert_eq!(result, Value::Integer(2));
}

#[test]
fn jit_match_guards_true_and_false_paths_work() {
    let result = run_jit(
        r#"
let xs = list(1, 2)
match xs {
    [h | t] if h == 2 -> 99,
    [h | t] if h == 1 -> 42,
    _ -> 0
}
"#,
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn jit_match_guards_can_use_pattern_bound_values() {
    let result = run_jit(
        r#"
match list(3, 4) {
    [h | t] if h + len(t) == 4 -> 1,
    _ -> 0
}
"#,
    );
    assert_eq!(result, Value::Integer(1));
}

#[test]
fn jit_module_member_call_works() {
    let result = run_jit(
        r#"
module Math {
    fn add(a, b) { a + b }
}
Math.add(20, 22)
"#,
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn jit_module_member_call_via_import_alias_works() {
    let result = run_jit(
        r#"
module Math {
    fn add(a, b) { a + b }
}
import Math as M
M.add(40, 2)
"#,
    );
    assert_eq!(result, Value::Integer(42));
}
