use crate::runtime::value::Value;

use super::string_ops::{
    builtin_chars, builtin_join, builtin_lower, builtin_split, builtin_substring,
    builtin_to_string, builtin_trim, builtin_upper,
};

#[test]
fn to_string_converts_values() {
    let result = builtin_to_string(&[Value::Integer(42)]).unwrap();
    assert_eq!(result, Value::String("42".to_string()));
}

#[test]
fn split_empty_delim_splits_chars() {
    let result = builtin_split(&[
        Value::String("ab".to_string()),
        Value::String("".to_string()),
    ])
    .unwrap();

    assert_eq!(
        result,
        Value::Array(vec![
            Value::String("a".to_string()),
            Value::String("b".to_string())
        ])
    );
}

#[test]
fn join_rejects_non_string_elements() {
    let err = builtin_join(&[
        Value::Array(vec![Value::Integer(1)]),
        Value::String(",".to_string()),
    ])
    .unwrap_err();
    assert!(err.contains("join expected array elements to be String"));
}

#[test]
fn trim_upper_lower_chars() {
    let trimmed = builtin_trim(&[Value::String("  hi ".to_string())]).unwrap();
    assert_eq!(trimmed, Value::String("hi".to_string()));

    let upper = builtin_upper(&[Value::String("hi".to_string())]).unwrap();
    assert_eq!(upper, Value::String("HI".to_string()));

    let lower = builtin_lower(&[Value::String("HI".to_string())]).unwrap();
    assert_eq!(lower, Value::String("hi".to_string()));

    let chars = builtin_chars(&[Value::String("ab".to_string())]).unwrap();
    assert_eq!(
        chars,
        Value::Array(vec![
            Value::String("a".to_string()),
            Value::String("b".to_string())
        ])
    );
}

#[test]
fn substring_extracts_range() {
    let result = builtin_substring(&[
        Value::String("hello".to_string()),
        Value::Integer(1),
        Value::Integer(4),
    ])
    .unwrap();

    assert_eq!(result, Value::String("ell".to_string()));
}
