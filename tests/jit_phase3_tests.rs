#![cfg(feature = "jit")]

use std::rc::Rc;

use flux::diagnostics::render_diagnostics;
use flux::jit::{JitOptions, jit_compile_and_run};
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
    let opts = JitOptions::default();
    jit_compile_and_run(&program, &interner, &opts).unwrap().0
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
    let opts = JitOptions::default();
    match jit_compile_and_run(&program, &interner, &opts) {
        Ok((value, _)) => panic!("expected JIT error, got value: {}", value),
        Err(err) => err,
    }
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
fn jit_named_function_call_arity3_uses_register_abi() {
    let result = run_jit(
        r#"
fn add3(a, b, c) { a + b + c }
add3(2, 3, 4)
"#,
    );
    assert_eq!(result, Value::Integer(9));
}

#[test]
fn jit_named_function_call_arity4_uses_register_abi() {
    let result = run_jit(
        r#"
fn add4(a, b, c, d) { a + b + c + d }
add4(1, 2, 3, 4)
"#,
    );
    assert_eq!(result, Value::Integer(10));
}

#[test]
fn jit_zero_arity_function_keeps_array_abi() {
    let result = run_jit(
        r#"
fn forty_two() { 42 }
forty_two()
"#,
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn jit_infix_integer_arithmetic_and_comparison_work() {
    let result = run_jit(
        r#"
let x = 7 + 5
if x == 12 { x * 2 } else { 0 }
"#,
    );
    assert_eq!(result, Value::Integer(24));
}

#[test]
fn jit_infix_float_arithmetic_and_comparison_work() {
    let result = run_jit(
        r#"
let x = 1.5 + 2.5
if x > 3.0 { x / 2.0 } else { 0.0 }
"#,
    );
    assert_eq!(result, Value::Float(2.0));
}

#[test]
fn jit_numeric_primops_lower_correctly() {
    let result = run_jit("iadd(imul(3, 4), 5)");
    assert_eq!(result, Value::Integer(17));

    let result = run_jit("fdiv(fadd(1.5, 2.5), 2.0)");
    assert_eq!(result, Value::Float(2.0));
}

#[test]
fn jit_integer_division_by_zero_from_primop_errors() {
    let err = run_jit_err("idiv(10, 0)");
    assert!(err.contains("division by zero"), "{err}");
}

#[test]
fn jit_arity5_function_keeps_array_abi() {
    let result = run_jit(
        r#"
fn add5(a, b, c, d, e) { a + b + c + d + e }
add5(1, 2, 3, 4, 5)
"#,
    );
    assert_eq!(result, Value::Integer(15));
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
fn jit_integer_locals_flow_through_arithmetic() {
    let result = run_jit(
        r#"
let x = 10
let y = x + 20
y + x
"#,
    );
    assert_eq!(result, Value::Integer(40));
}

#[test]
fn jit_integer_if_branches_roundtrip_through_boxed_join() {
    let result = run_jit(
        r#"
fn choose(flag) {
    if flag { 10 + 20 } else { 1 + 2 }
}
choose(true)
"#,
    );
    assert_eq!(result, Value::Integer(30));
}

#[test]
fn jit_nested_non_nullary_adt_construction_works() {
    let result = run_jit(
        r#"
type Expr = Var(Int) | Val(Int) | Add(Expr, Expr)

fn mk(n) {
    if n == 0 {
        Val(1)
    } else {
        Add(Val(n), mk(n - 1))
    }
}

fn eval(expr) {
    match expr {
        Var(_) -> 0,
        Val(v) -> v,
        Add(left, right) -> eval(left) + eval(right),
    }
}

eval(mk(8))
"#,
    );
    assert_eq!(result, Value::Integer(37));
}

#[test]
fn jit_nested_constructor_patterns_check_inner_shapes() {
    let result = run_jit(
        r#"
type Expr = Var(Int) | Val(Int) | Add(Expr, Expr)

fn score(expr) {
    match expr {
        Add(left, Val(right)) -> match left {
            Val(v) -> v + right,
            _ -> 0,
        },
        _ -> 0,
    }
}

score(Add(Val(4), Val(5)))
"#,
    );
    assert_eq!(result, Value::Integer(9));
}

#[test]
fn jit_nested_constructor_patterns_check_literal_fields() {
    let result = run_jit(
        r#"
type Tree = Leaf | Node(Int)
type Del = Del(Tree, Bool)
type Delmin = Delmin(Del, Int, Bool)

fn score(x: Delmin) -> Int {
    match x {
        Delmin(Del(_t, true), k, true) -> k + 100,
        Delmin(Del(_t, false), k, true) -> k + 200,
        Delmin(Del(_t, true), k, false) -> k + 300,
        Delmin(Del(_t, false), k, false) -> k + 400,
    }
}

score(Delmin(Del(Node(1), true), 7, false))
"#,
    );
    assert_eq!(result, Value::Integer(307));
}

#[test]
fn jit_integer_values_box_at_collection_boundaries() {
    let result = run_jit(
        r#"
let x = 40 + 2
let arr = [|x, x + 1|]
arr[1]
"#,
    );
    assert_eq!(result, Value::Some(Rc::new(Value::Integer(43))));
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
fn jit_base_value_callable_works() {
    let result = run_jit(
        r#"
let f = len
f("flux")
"#,
    );
    assert_eq!(result, Value::Integer(4));
}

#[test]
fn jit_named_function_value_for_higher_order_base_works() {
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

#[test]
fn jit_boolean_if_join_stays_unboxed_through_nested_control_flow() {
    let result = run_jit(
        r#"
let b = if 1 < 2 { true } else { false }
if b { 1 } else { 0 }
"#,
    );
    assert_eq!(result, Value::Integer(1));
}

#[test]
fn jit_nested_boolean_if_joins_preserve_bool_kind() {
    let result = run_jit(
        r#"
let b =
    if 1 < 2 {
        if 2 < 3 { true } else { false }
    } else {
        false
    }
if b { 1 } else { 0 }
"#,
    );
    assert_eq!(result, Value::Integer(1));
}

#[test]
fn jit_boolean_short_circuit_result_feeds_control_flow() {
    let result = run_jit(
        r#"
let b = (1 < 2) && (2 < 3)
if b { 1 } else { 0 }
"#,
    );
    assert_eq!(result, Value::Integer(1));
}

#[test]
fn jit_boxed_bool_if_condition_uses_boolean_semantics() {
    let result = run_jit(
        r#"
fn is_zero(n) { n == 0 }
if is_zero(0) { 1 } else { 0 }
"#,
    );
    assert_eq!(result, Value::Integer(1));
}

#[test]
fn jit_truthy_short_circuit_with_non_boolean_values_stays_boxed() {
    let result = run_jit(
        r#"
let x = 1 && 2
let y = None || 7
x + y
"#,
    );
    assert_eq!(result, Value::Integer(9));
}

#[test]
fn jit_non_bool_if_condition_still_uses_runtime_truthiness() {
    let result = run_jit(
        r#"
if None { 1 } else { 0 }
"#,
    );
    assert_eq!(result, Value::Integer(0));
}

#[test]
fn jit_boolean_boxes_when_crossing_collection_boundary() {
    let result = run_jit(
        r#"
let xs = [if 1 < 2 { true } else { false }]
if first(xs) { 1 } else { 0 }
"#,
    );
    assert_eq!(result, Value::Integer(1));
}

#[test]
fn jit_boxed_bool_match_guard_uses_boolean_semantics() {
    let result = run_jit(
        r#"
fn is_target(n) { n == 1 }
match list(1, 2) {
    [h | t] if is_target(h) -> 42,
    _ -> 0
}
"#,
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn jit_non_bool_match_guard_still_uses_runtime_truthiness() {
    let result = run_jit(
        r#"
match list(1, 2) {
    [h | t] if None -> 99,
    _ -> 42
}
"#,
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn jit_boxed_bool_short_circuit_uses_boolean_semantics() {
    let result = run_jit(
        r#"
fn yes() { true }
let b = yes() && true
if b { 1 } else { 0 }
"#,
    );
    assert_eq!(result, Value::Integer(1));
}
