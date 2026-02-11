use crate::{
    bytecode::bytecode::Bytecode,
    runtime::{value::Value, vm::VM},
};

use super::array_ops::{
    builtin_concat, builtin_contains, builtin_first, builtin_last, builtin_len, builtin_push,
    builtin_rest, builtin_reverse, builtin_slice, builtin_sort,
};

fn test_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

#[test]
fn len_works_for_string_and_array() {
    let result = builtin_len(
        &mut test_vm(),
        vec![Value::String("abc".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Value::Integer(3));

    let result = builtin_len(vec![Value::Array(
        vec![Value::Integer(1), Value::Integer(2)].into(),
    )])
    .unwrap();
    assert_eq!(result, Value::Integer(2));
}

#[test]
fn first_last_rest_work() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());
    let first = builtin_first(vec![arr.clone()]).unwrap();
    assert_eq!(first, Value::Integer(1));

    let last = builtin_last(vec![arr.clone()]).unwrap();
    assert_eq!(last, Value::Integer(2));

    let rest = builtin_rest(vec![arr]).unwrap();
    assert_eq!(rest, Value::Array(vec![Value::Integer(2)].into()));
}

#[test]
fn push_concat_reverse_contains_slice() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());

    let pushed = builtin_push(vec![arr.clone(), Value::Integer(3)]).unwrap();
    assert_eq!(
        pushed,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    let concat = builtin_concat(vec![
        arr.clone(),
        Value::Array(vec![Value::Integer(3)].into()),
    ])
    .unwrap();
    assert_eq!(
        concat,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    let reversed = builtin_reverse(vec![arr.clone()]).unwrap();
    assert_eq!(
        reversed,
        Value::Array(vec![Value::Integer(2), Value::Integer(1)].into())
    );

    let contains = builtin_contains(vec![arr.clone(), Value::Integer(2)]).unwrap();
    assert_eq!(contains, Value::Boolean(true));

    let sliced = builtin_slice(vec![arr, Value::Integer(0), Value::Integer(1)]).unwrap();
    assert_eq!(sliced, Value::Array(vec![Value::Integer(1)].into()));
}

#[test]
fn sort_default_and_desc() {
    let arr = Value::Array(vec![Value::Integer(3), Value::Integer(1), Value::Integer(2)].into());
    let sorted = builtin_sort(vec![arr.clone()]).unwrap();
    assert_eq!(
        sorted,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    let sorted_desc = builtin_sort(vec![arr, Value::String("desc".to_string().into())]).unwrap();
    assert_eq!(
        sorted_desc,
        Value::Array(vec![Value::Integer(3), Value::Integer(2), Value::Integer(1)].into())
    );
}

#[test]
fn sort_rejects_bad_order() {
    let arr = Value::Array(vec![Value::Integer(1)].into());
    let err = builtin_sort(vec![arr, Value::String("down".to_string().into())]).unwrap_err();
    assert!(err.contains("sort order"));
}
