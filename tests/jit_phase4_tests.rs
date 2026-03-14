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

// ---------------------------------------------------------------------------
// Some / Left / Right expressions
// ---------------------------------------------------------------------------

#[test]
fn jit_some_expression() {
    let result = run_jit(
        r#"
let x = Some(42)
x
"#,
    );
    assert_eq!(result, Value::Some(Rc::new(Value::Integer(42))));
}

#[test]
fn jit_left_expression() {
    let result = run_jit(
        r#"
let x = Left(1)
x
"#,
    );
    assert_eq!(result, Value::Left(Rc::new(Value::Integer(1))));
}

#[test]
fn jit_right_expression() {
    let result = run_jit(
        r#"
let x = Right("hello")
x
"#,
    );
    assert_eq!(result, Value::Right(Rc::new(Value::String("hello".into()))));
}

// ---------------------------------------------------------------------------
// Array literals
// ---------------------------------------------------------------------------

#[test]
fn jit_array_literal_empty() {
    let result = run_jit("[||]");
    assert_eq!(result, Value::Array(Rc::new(vec![])));
}

#[test]
fn jit_array_literal() {
    let result = run_jit("[|1, 2, 3|]");
    assert_eq!(
        result,
        Value::Array(Rc::new(vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
        ]))
    );
}

// ---------------------------------------------------------------------------
// List literals (cons chains)
// ---------------------------------------------------------------------------

#[test]
fn jit_list_literal() {
    // list(1, 2, 3) builds a cons chain: Cons(1, Cons(2, Cons(3, None)))
    // We verify by matching on the list
    let result = run_jit(
        r#"
fn first(l) {
    match l {
        [h | _] -> h,
        _ -> None
    }
}
first(list(10, 20, 30))
"#,
    );
    assert_eq!(result, Value::Integer(10));
}

#[test]
fn jit_list_literal_sum() {
    let result = run_jit(
        r#"
fn my_sum(l, acc) {
    match l {
        [h | t] -> my_sum(t, acc + h),
        _ -> acc
    }
}
my_sum(list(1, 2, 3, 4), 0)
"#,
    );
    assert_eq!(result, Value::Integer(10));
}

// ---------------------------------------------------------------------------
// Hash maps
// ---------------------------------------------------------------------------

#[test]
fn jit_hash_literal_and_index() {
    let result = run_jit(
        r#"
let m = {"x": 10, "y": 20}
m["x"]
"#,
    );
    assert_eq!(result, Value::Some(Rc::new(Value::Integer(10))));
}

#[test]
fn jit_hash_missing_key() {
    let result = run_jit(
        r#"
let m = {"a": 1}
m["z"]
"#,
    );
    assert_eq!(result, Value::None);
}

// ---------------------------------------------------------------------------
// Index expressions
// ---------------------------------------------------------------------------

#[test]
fn jit_array_index_in_bounds() {
    let result = run_jit(
        r#"
let arr = [|10, 20, 30|]
arr[1]
"#,
    );
    assert_eq!(result, Value::Some(Rc::new(Value::Integer(20))));
}

#[test]
fn jit_array_index_out_of_bounds() {
    let result = run_jit(
        r#"
let arr = [|1, 2|]
arr[5]
"#,
    );
    assert_eq!(result, Value::None);
}

// ---------------------------------------------------------------------------
// String interpolation
// ---------------------------------------------------------------------------

#[test]
#[ignore = "JIT: string interpolation Cranelift verifier error (proposal 0102)"]
fn jit_string_interpolation_basic() {
    let result = run_jit(
        r##"
let name = "world"
"hello #{name}"
"##,
    );
    assert_eq!(result, Value::String("hello world".into()));
}

#[test]
#[ignore = "JIT: string interpolation Cranelift verifier error (proposal 0102)"]
fn jit_string_interpolation_expression() {
    let result = run_jit(r##""result: #{1 + 2}""##);
    assert_eq!(result, Value::String("result: 3".into()));
}

#[test]
#[ignore = "JIT: string interpolation Cranelift verifier error (proposal 0102)"]
fn jit_string_interpolation_multiple() {
    let result = run_jit(
        r##"
let a = 1
let b = 2
"#{a} + #{b} = #{a + b}"
"##,
    );
    assert_eq!(result, Value::String("1 + 2 = 3".into()));
}

// ---------------------------------------------------------------------------
// Pattern matching: None / EmptyList
// ---------------------------------------------------------------------------

#[test]
fn jit_match_none_pattern() {
    let result = run_jit(
        r#"
fn check(x) {
    match x {
        None -> "nothing",
        _ -> "something"
    }
}
check(None)
"#,
    );
    assert_eq!(result, Value::String("nothing".into()));
}

#[test]
fn jit_match_empty_list_pattern() {
    let result = run_jit(
        r#"
fn len(l) {
    match l {
        [] -> 0,
        [_ | t] -> 1 + len(t)
    }
}
len(list(1, 2, 3))
"#,
    );
    assert_eq!(result, Value::Integer(3));
}

// ---------------------------------------------------------------------------
// Pattern matching: Some / Left / Right
// ---------------------------------------------------------------------------

#[test]
fn jit_match_some_pattern() {
    let result = run_jit(
        r#"
fn unwrap_or(opt, default) {
    match opt {
        Some(x) -> x,
        _ -> default
    }
}
unwrap_or(Some(42), 0)
"#,
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn jit_match_some_none_dispatch() {
    let result = run_jit(
        r#"
fn unwrap_or(opt, default) {
    match opt {
        Some(x) -> x,
        _ -> default
    }
}
unwrap_or(None, 99)
"#,
    );
    assert_eq!(result, Value::Integer(99));
}

#[test]
fn jit_match_left_right_pattern() {
    let result = run_jit(
        r#"
fn describe(e) {
    match e {
        Left(x) -> x * 10,
        Right(x) -> x + 100,
        _ -> 0
    }
}
describe(Left(5))
"#,
    );
    assert_eq!(result, Value::Integer(50));
}

#[test]
fn jit_match_right_branch() {
    let result = run_jit(
        r#"
fn describe(e) {
    match e {
        Left(x) -> x * 10,
        Right(x) -> x + 100,
        _ -> 0
    }
}
describe(Right(7))
"#,
    );
    assert_eq!(result, Value::Integer(107));
}

// ---------------------------------------------------------------------------
// Pattern matching: Literal patterns
// ---------------------------------------------------------------------------

#[test]
fn jit_match_literal_integer() {
    let result = run_jit(
        r#"
fn describe(n) {
    match n {
        0 -> "zero",
        1 -> "one",
        _ -> "other"
    }
}
describe(1)
"#,
    );
    assert_eq!(result, Value::String("one".into()));
}

#[test]
fn jit_match_literal_string() {
    let result = run_jit(
        r#"
fn greet(name) {
    match name {
        "Alice" -> "Hi Alice!",
        "Bob" -> "Hey Bob!",
        _ -> "Hello stranger"
    }
}
greet("Alice")
"#,
    );
    assert_eq!(result, Value::String("Hi Alice!".into()));
}

#[test]
fn jit_match_literal_fallthrough() {
    let result = run_jit(
        r#"
fn describe(n) {
    match n {
        0 -> "zero",
        1 -> "one",
        _ -> "other"
    }
}
describe(42)
"#,
    );
    assert_eq!(result, Value::String("other".into()));
}

// ---------------------------------------------------------------------------
// Integration: combining features
// ---------------------------------------------------------------------------

#[test]
fn jit_array_index_match_some() {
    let result = run_jit(
        r#"
let arr = [|10, 20, 30|]
match arr[1] {
    Some(x) -> x,
    _ -> -1
}
"#,
    );
    assert_eq!(result, Value::Integer(20));
}

#[test]
fn jit_hash_index_match_none() {
    let result = run_jit(
        r#"
let m = {"a": 1}
match m["missing"] {
    Some(x) -> x,
    _ -> -1
}
"#,
    );
    assert_eq!(result, Value::Integer(-1));
}

#[test]
fn jit_nested_some_pattern() {
    let result = run_jit(
        r#"
let x = Some(Some(5))
match x {
    Some(Some(n)) -> n,
    _ -> 0
}
"#,
    );
    assert_eq!(result, Value::Integer(5));
}

// ---------------------------------------------------------------------------
// CFG fast-path: typed Int param unboxing (Proposal 2)
// ---------------------------------------------------------------------------
// These functions have annotated Int parameters so the IR emits IAdd/ISub/etc.
// and the eligibility checker can seed those params as JitValueKind::Int.
// The CFG body compiler then loads the raw i64 payload instead of boxing.

#[test]
fn jit_cfg_typed_add_int_params() {
    let result = run_jit(
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
add(3, 4)
"#,
    );
    assert_eq!(result, Value::Integer(7));
}

#[test]
fn jit_cfg_typed_mul_int_params() {
    let result = run_jit(
        r#"
fn mul(a: Int, b: Int) -> Int {
    a * b
}
mul(6, 7)
"#,
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn jit_cfg_typed_sub_negative_result() {
    let result = run_jit(
        r#"
fn sub(a: Int, b: Int) -> Int {
    a - b
}
sub(3, 10)
"#,
    );
    assert_eq!(result, Value::Integer(-7));
}

#[test]
fn jit_cfg_typed_self_tail_call_sum() {
    // Self-tail-recursive function with Int params — exercises the deferred
    // entry-block sealing fix so that loop variable phis are built correctly.
    let result = run_jit(
        r#"
fn acc_sum(acc: Int, n: Int) -> Int {
    if n <= 0 {
        acc
    } else {
        acc_sum(acc + n, n - 1)
    }
}
acc_sum(0, 10)
"#,
    );
    assert_eq!(result, Value::Integer(55));
}

// ---------------------------------------------------------------------------
// Proposal 4: unit ADT interning
// ---------------------------------------------------------------------------

#[test]
fn jit_unit_adt_basic() {
    // A nullary constructor should produce a valid AdtUnit value.
    let result = run_jit(
        r#"
type Color = Red | Green | Blue
Red
"#,
    );
    assert_eq!(result, Value::AdtUnit(Rc::from("Red")));
}

#[test]
fn jit_unit_adt_match() {
    // Match on a unit ADT constructor returned from a function.
    let result = run_jit(
        r#"
type Color = Red | Green | Blue
fn pick(n: Int) -> Color {
    if n == 0 { Red } else { Green }
}
match pick(0) {
    Red -> 1,
    Green -> 2,
    _ -> 3
}
"#,
    );
    assert_eq!(result, Value::Integer(1));
}

#[test]
fn jit_unit_adt_repeated_use() {
    // The same unit constructor used multiple times in a loop — exercises the
    // interning cache path (second call returns the cached pointer).
    let result = run_jit(
        r#"
type Flag = On | Off
fn count_on(n: Int) -> Int {
    if n <= 0 {
        0
    } else {
        let f = On
        match f {
            On -> 1 + count_on(n - 1),
            _ -> count_on(n - 1)
        }
    }
}
count_on(5)
"#,
    );
    assert_eq!(result, Value::Integer(5));
}

// ---------------------------------------------------------------------------
// Proposal 5: base function call with tagged (unboxed) arguments
// ---------------------------------------------------------------------------

#[test]
fn jit_base_call_int_arg() {
    // `len` receives an array arg (Boxed). Exercises the tagged helper path.
    let result = run_jit(r#"len([|1, 2, 3|])"#);
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn jit_base_call_string_arg() {
    // `len` with a string — Boxed arg path through tagged helper.
    let result = run_jit(r#"len("hello")"#);
    assert_eq!(result, Value::Integer(5));
}

#[test]
fn jit_base_call_two_args() {
    // `min` takes two args; exercises the multi-arg tagged path.
    let result = run_jit(r#"min(7, 3)"#);
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn jit_base_call_unboxed_int_arg() {
    // The argument `n + 1` produces JitValueKind::Int — the primary case
    // being optimised: the tagged helper avoids rt_force_boxed per arg.
    let result = run_jit(
        r#"
fn make_arr(n: Int) -> Int {
    len([|n + 1, n + 2|])
}
make_arr(10)
"#,
    );
    assert_eq!(result, Value::Integer(2));
}

// ---------------------------------------------------------------------------
// Proposal 6: Mutual tail-call trampoline
// ---------------------------------------------------------------------------

#[test]
fn jit_mutual_tco_is_even_small() {
    // Basic mutual tail-call: is_even / is_odd, small input.
    let result = run_jit(
        r#"
fn is_even(n) { if n == 0 { true } else { is_odd(n - 1) } }
fn is_odd(n)  { if n == 0 { false } else { is_even(n - 1) } }
is_even(10)
"#,
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn jit_mutual_tco_is_odd_small() {
    let result = run_jit(
        r#"
fn is_even(n) { if n == 0 { true } else { is_odd(n - 1) } }
fn is_odd(n)  { if n == 0 { false } else { is_even(n - 1) } }
is_odd(7)
"#,
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn jit_mutual_tco_deep_even() {
    // Deep mutual recursion — would overflow the native stack without trampolining.
    let result = run_jit(
        r#"
fn is_even(n) { if n == 0 { true } else { is_odd(n - 1) } }
fn is_odd(n)  { if n == 0 { false } else { is_even(n - 1) } }
is_even(10000)
"#,
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn jit_mutual_tco_deep_odd() {
    let result = run_jit(
        r#"
fn is_even(n) { if n == 0 { true } else { is_odd(n - 1) } }
fn is_odd(n)  { if n == 0 { false } else { is_even(n - 1) } }
is_odd(9999)
"#,
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn jit_mutual_tco_three_way() {
    // Three-way mutual tail-call: a → b → c → a
    let result = run_jit(
        r#"
fn fa(n) { if n <= 0 { 0 } else { fb(n - 1) } }
fn fb(n) { if n <= 0 { 0 } else { fc(n - 1) } }
fn fc(n) { if n <= 0 { 0 } else { fa(n - 1) } }
fa(300)
"#,
    );
    assert_eq!(result, Value::Integer(0));
}

#[test]
fn jit_base_call_to_string_int() {
    // `to_string` on an Int — exercises the tagged arg path for a
    // function that returns String.
    let result = run_jit(r#"to_string(42)"#);
    assert_eq!(result, Value::String("42".into()));
}
