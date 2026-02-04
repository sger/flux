use crate::runtime::object::Object;

use super::string_ops::{
    builtin_chars, builtin_join, builtin_lower, builtin_split, builtin_substring, builtin_to_string,
    builtin_trim, builtin_upper,
};

#[test]
fn to_string_converts_values() {
    let result = builtin_to_string(vec![Object::Integer(42)]).unwrap();
    assert_eq!(result, Object::String("42".to_string()));
}

#[test]
fn split_empty_delim_splits_chars() {
    let result = builtin_split(vec![
        Object::String("ab".to_string()),
        Object::String("".to_string()),
    ])
    .unwrap();

    assert_eq!(
        result,
        Object::Array(vec![Object::String("a".to_string()), Object::String("b".to_string())])
    );
}

#[test]
fn join_rejects_non_string_elements() {
    let err = builtin_join(vec![
        Object::Array(vec![Object::Integer(1)]),
        Object::String(",".to_string()),
    ])
    .unwrap_err();
    assert!(err.contains("join expected array elements to be String"));
}

#[test]
fn trim_upper_lower_chars() {
    let trimmed = builtin_trim(vec![Object::String("  hi ".to_string())]).unwrap();
    assert_eq!(trimmed, Object::String("hi".to_string()));

    let upper = builtin_upper(vec![Object::String("hi".to_string())]).unwrap();
    assert_eq!(upper, Object::String("HI".to_string()));

    let lower = builtin_lower(vec![Object::String("HI".to_string())]).unwrap();
    assert_eq!(lower, Object::String("hi".to_string()));

    let chars = builtin_chars(vec![Object::String("ab".to_string())]).unwrap();
    assert_eq!(
        chars,
        Object::Array(vec![Object::String("a".to_string()), Object::String("b".to_string())])
    );
}

#[test]
fn substring_extracts_range() {
    let result = builtin_substring(vec![
        Object::String("hello".to_string()),
        Object::Integer(1),
        Object::Integer(4),
    ])
    .unwrap();

    assert_eq!(result, Object::String("ell".to_string()));
}
