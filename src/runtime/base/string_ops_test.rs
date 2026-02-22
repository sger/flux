use crate::{
    bytecode::bytecode::Bytecode,
    runtime::{value::Value, vm::VM},
};

use super::string_ops::{
    base_chars, base_ends_with, base_join, base_lower, base_replace, base_split, base_starts_with,
    base_substring, base_to_string, base_trim, base_upper,
};

fn test_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

#[test]
fn to_string_converts_values() {
    let result = base_to_string(&mut test_vm(), vec![Value::Integer(42)]).unwrap();
    assert_eq!(result, Value::String("42".to_string().into()));
}

#[test]
fn split_empty_delim_splits_chars() {
    let result = base_split(
        &mut test_vm(),
        vec![
            Value::String("ab".to_string().into()),
            Value::String("".to_string().into()),
        ],
    )
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
    let err = base_join(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::String(",".to_string().into()),
        ],
    )
    .unwrap_err();
    assert!(err.contains("join expected array elements to be String"));
}

#[test]
fn trim_upper_lower_chars() {
    let trimmed = base_trim(
        &mut test_vm(),
        vec![Value::String("  hi ".to_string().into())],
    )
    .unwrap();
    assert_eq!(trimmed, Value::String("hi".to_string().into()));

    let upper = base_upper(&mut test_vm(), vec![Value::String("hi".to_string().into())]).unwrap();
    assert_eq!(upper, Value::String("HI".to_string().into()));

    let lower = base_lower(&mut test_vm(), vec![Value::String("HI".to_string().into())]).unwrap();
    assert_eq!(lower, Value::String("hi".to_string().into()));

    let chars = base_chars(&mut test_vm(), vec![Value::String("ab".to_string().into())]).unwrap();
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
    let result = base_substring(
        &mut test_vm(),
        vec![
            Value::String("hello".to_string().into()),
            Value::Integer(1),
            Value::Integer(4),
        ],
    )
    .unwrap();

    assert_eq!(result, Value::String("ell".to_string().into()));
}

#[test]
fn starts_ends_and_replace_work() {
    let starts = base_starts_with(
        &mut test_vm(),
        vec![
            Value::String("hello".to_string().into()),
            Value::String("he".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(starts, Value::Boolean(true));

    let ends = base_ends_with(
        &mut test_vm(),
        vec![
            Value::String("hello".to_string().into()),
            Value::String("lo".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(ends, Value::Boolean(true));

    let replaced = base_replace(
        &mut test_vm(),
        vec![
            Value::String("banana".to_string().into()),
            Value::String("na".to_string().into()),
            Value::String("X".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(replaced, Value::String("baXX".to_string().into()));
}
