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

// ---------------------------------------------------------------------------
// Some / Left / Right expressions
// ---------------------------------------------------------------------------

#[test]
fn jit_some_expression() {
    let result = run_jit("Some(42)");
    assert_eq!(result, Value::Some(Rc::new(Value::Integer(42))));
}

#[test]
fn jit_left_expression() {
    let result = run_jit("Left(1)");
    assert_eq!(result, Value::Left(Rc::new(Value::Integer(1))));
}

#[test]
fn jit_right_expression() {
    let result = run_jit(r#"Right("hello")"#);
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
fn jit_string_interpolation_expression() {
    let result = run_jit(r##""result: #{1 + 2}""##);
    assert_eq!(result, Value::String("result: 3".into()));
}

#[test]
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
