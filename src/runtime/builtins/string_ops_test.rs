use crate::runtime::value::Value;

use super::string_ops::{
    builtin_chars, builtin_ends_with, builtin_join, builtin_lower, builtin_replace, builtin_split,
    builtin_starts_with, builtin_substring, builtin_to_string, builtin_trim, builtin_upper,
};

#[test]
fn to_string_converts_values() {
    let result = builtin_to_string(&[Value::Integer(42)]).unwrap();
    assert_eq!(result, Value::String("42".to_string().into()));
}

#[test]
fn split_empty_delim_splits_chars() {
    let result = builtin_split(&[
        Value::String("ab".to_string().into()),
        Value::String("".to_string().into()),
    ])
    .unwrap();

    assert_eq!(
        result,
        Value::Array(
            vec![
                Value::String("a".to_string().into()),
                Value::String("b".to_string().into())
            ]
            .into()
        )
    );
}

#[test]
fn join_rejects_non_string_elements() {
    let err = builtin_join(&[
        Value::Array(vec![Value::Integer(1)].into()),
        Value::String(",".to_string().into()),
    ])
    .unwrap_err();
    assert!(err.contains("join expected array elements to be String"));
}

#[test]
fn trim_upper_lower_chars() {
    let trimmed = builtin_trim(&[Value::String("  hi ".to_string().into())]).unwrap();
    assert_eq!(trimmed, Value::String("hi".to_string().into()));

    let upper = builtin_upper(&[Value::String("hi".to_string().into())]).unwrap();
    assert_eq!(upper, Value::String("HI".to_string().into()));

    let lower = builtin_lower(&[Value::String("HI".to_string().into())]).unwrap();
    assert_eq!(lower, Value::String("hi".to_string().into()));

    let chars = builtin_chars(&[Value::String("ab".to_string().into())]).unwrap();
    assert_eq!(
        chars,
        Value::Array(
            vec![
                Value::String("a".to_string().into()),
                Value::String("b".to_string().into())
            ]
            .into()
        )
    );
}

#[test]
fn substring_extracts_range() {
    let result = builtin_substring(&[
        Value::String("hello".to_string().into()),
        Value::Integer(1),
        Value::Integer(4),
    ])
    .unwrap();

    assert_eq!(result, Value::String("ell".to_string().into()));
}

#[test]
fn starts_ends_and_replace_work() {
    let starts = builtin_starts_with(&[
        Value::String("hello".to_string().into()),
        Value::String("he".to_string().into()),
    ])
    .unwrap();
    assert_eq!(starts, Value::Boolean(true));

    let ends = builtin_ends_with(&[
        Value::String("hello".to_string().into()),
        Value::String("lo".to_string().into()),
    ])
    .unwrap();
    assert_eq!(ends, Value::Boolean(true));

    let replaced = builtin_replace(&[
        Value::String("banana".to_string().into()),
        Value::String("na".to_string().into()),
        Value::String("X".to_string().into()),
    ])
    .unwrap();
    assert_eq!(replaced, Value::String("baXX".to_string().into()));
}
