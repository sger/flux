use crate::{
    bytecode::bytecode::Bytecode,
    runtime::{value::Value, vm::VM},
};

use super::array_ops::base_sort;
use super::collection_ops::{
    base_concat, base_contains, base_first, base_last, base_len, base_push, base_rest,
    base_reverse, base_slice,
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
    let result = base_len(
        &mut test_vm(),
        vec![Value::String("abc".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Value::Integer(3));

    let result = base_len(
        &mut test_vm(),
        vec![Value::Array(
            vec![Value::Integer(1), Value::Integer(2)].into(),
        )],
    )
    .unwrap();
    assert_eq!(result, Value::Integer(2));
}

#[test]
fn first_last_rest_work() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());
    let first = base_first(&mut test_vm(), vec![arr.clone()]).unwrap();
    assert_eq!(first, Value::Integer(1));

    let last = base_last(&mut test_vm(), vec![arr.clone()]).unwrap();
    assert_eq!(last, Value::Integer(2));

    let rest = base_rest(&mut test_vm(), vec![arr]).unwrap();
    assert_eq!(rest, Value::Array(vec![Value::Integer(2)].into()));
}

#[test]
fn push_concat_reverse_contains_slice() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());

    let pushed = base_push(&mut test_vm(), vec![arr.clone(), Value::Integer(3)]).unwrap();
    assert_eq!(
        pushed,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    let concat = base_concat(
        &mut test_vm(),
        vec![arr.clone(), Value::Array(vec![Value::Integer(3)].into())],
    )
    .unwrap();
    assert_eq!(
        concat,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    let reversed = base_reverse(&mut test_vm(), vec![arr.clone()]).unwrap();
    assert_eq!(
        reversed,
        Value::Array(vec![Value::Integer(2), Value::Integer(1)].into())
    );

    let contains = base_contains(&mut test_vm(), vec![arr.clone(), Value::Integer(2)]).unwrap();
    assert_eq!(contains, Value::Boolean(true));

    let sliced = base_slice(
        &mut test_vm(),
        vec![arr, Value::Integer(0), Value::Integer(1)],
    )
    .unwrap();
    assert_eq!(sliced, Value::Array(vec![Value::Integer(1)].into()));
}

#[test]
fn sort_default_and_desc() {
    let arr = Value::Array(vec![Value::Integer(3), Value::Integer(1), Value::Integer(2)].into());
    let sorted = base_sort(&mut test_vm(), vec![arr.clone()]).unwrap();
    assert_eq!(
        sorted,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    let sorted_desc = base_sort(
        &mut test_vm(),
        vec![arr, Value::String("desc".to_string().into())],
    )
    .unwrap();
    assert_eq!(
        sorted_desc,
        Value::Array(vec![Value::Integer(3), Value::Integer(2), Value::Integer(1)].into())
    );
}

#[test]
fn sort_rejects_bad_order() {
    let arr = Value::Array(vec![Value::Integer(1)].into());
    let err = base_sort(
        &mut test_vm(),
        vec![arr, Value::String("down".to_string().into())],
    )
    .unwrap_err();
    assert!(err.contains("sort order"));
}
