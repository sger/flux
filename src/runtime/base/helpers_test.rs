use crate::runtime::value::Value;

use super::helpers::{arg_string, check_arity, check_arity_range, format_hint, type_error};

#[test]
fn format_hint_includes_label() {
    let hint = format_hint("len(value)");
    assert!(hint.contains("Hint:"));
    assert!(hint.contains("len(value)"));
}

#[test]
fn check_arity_rejects_wrong_count() {
    let args = vec![Value::Integer(1)];
    let err = check_arity(&args, 2, "len", "len(value)").unwrap_err();
    assert!(err.contains("wrong number of arguments"));
}

#[test]
fn check_arity_range_rejects_out_of_range() {
    let args = vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)];
    let err = check_arity_range(&args, 1, 2, "sort", "sort(arr, order)").unwrap_err();
    assert!(err.contains("wrong number of arguments"));
}

#[test]
fn arg_string_returns_type_error() {
    let args = vec![Value::Integer(1)];
    let err = arg_string(&args, 0, "join", "argument", "join(arr, delim)").unwrap_err();
    assert!(err.contains("expected"));
    assert!(err.contains("String"));
}

#[test]
fn type_error_formats_message() {
    let msg = type_error("len", "argument", "String", "Int", "len(value)");
    assert!(msg.contains("len expected argument to be String"));
    assert!(msg.contains("got Int"));
}
