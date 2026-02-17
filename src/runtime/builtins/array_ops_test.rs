use crate::{
    bytecode::bytecode::Bytecode,
    runtime::{builtins::get_builtin_index, value::Value, vm::VM},
};

use super::array_ops::{
    builtin_concat, builtin_contains, builtin_filter, builtin_first, builtin_fold, builtin_last,
    builtin_len, builtin_map, builtin_push, builtin_rest, builtin_reverse, builtin_slice,
    builtin_sort,
};

fn test_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

fn builtin(name: &str) -> Value {
    Value::Builtin(get_builtin_index(name).expect("builtin exists") as u8)
}

#[test]
fn len_works_for_string_and_array() {
    let result = builtin_len(
        &mut test_vm(),
        vec![Value::String("abc".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Value::Integer(3));

    let result = builtin_len(
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
    let first = builtin_first(&mut test_vm(), vec![arr.clone()]).unwrap();
    assert_eq!(first, Value::Integer(1));

    let last = builtin_last(&mut test_vm(), vec![arr.clone()]).unwrap();
    assert_eq!(last, Value::Integer(2));

    let rest = builtin_rest(&mut test_vm(), vec![arr]).unwrap();
    assert_eq!(rest, Value::Array(vec![Value::Integer(2)].into()));
}

#[test]
fn push_concat_reverse_contains_slice() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());

    let pushed = builtin_push(&mut test_vm(), vec![arr.clone(), Value::Integer(3)]).unwrap();
    assert_eq!(
        pushed,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    let concat = builtin_concat(
        &mut test_vm(),
        vec![arr.clone(), Value::Array(vec![Value::Integer(3)].into())],
    )
    .unwrap();
    assert_eq!(
        concat,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    let reversed = builtin_reverse(&mut test_vm(), vec![arr.clone()]).unwrap();
    assert_eq!(
        reversed,
        Value::Array(vec![Value::Integer(2), Value::Integer(1)].into())
    );

    let contains = builtin_contains(&mut test_vm(), vec![arr.clone(), Value::Integer(2)]).unwrap();
    assert_eq!(contains, Value::Boolean(true));

    let sliced = builtin_slice(
        &mut test_vm(),
        vec![arr, Value::Integer(0), Value::Integer(1)],
    )
    .unwrap();
    assert_eq!(sliced, Value::Array(vec![Value::Integer(1)].into()));
}

#[test]
fn sort_default_and_desc() {
    let arr = Value::Array(vec![Value::Integer(3), Value::Integer(1), Value::Integer(2)].into());
    let sorted = builtin_sort(&mut test_vm(), vec![arr.clone()]).unwrap();
    assert_eq!(
        sorted,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    let sorted_desc = builtin_sort(
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
    let err = builtin_sort(
        &mut test_vm(),
        vec![arr, Value::String("down".to_string().into())],
    )
    .unwrap_err();
    assert!(err.contains("sort order"));
}

#[test]
fn map_with_builtin_callback_and_empty_input() {
    let arr = Value::Array(
        vec![
            Value::String("a".into()),
            Value::String("hello".into()),
            Value::String("xyz".into()),
        ]
        .into(),
    );

    let mapped = builtin_map(&mut test_vm(), vec![arr, builtin("len")]).unwrap();
    assert_eq!(
        mapped,
        Value::Array(vec![Value::Integer(1), Value::Integer(5), Value::Integer(3)].into())
    );

    let empty = builtin_map(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), builtin("len")],
    )
    .unwrap();
    assert_eq!(empty, Value::Array(vec![].into()));
}

#[test]
fn filter_truthiness_and_empty_input() {
    let arr = Value::Array(
        vec![
            Value::Array(vec![].into()),
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Array(vec![].into()),
            Value::Array(vec![Value::Integer(2), Value::Integer(3)].into()),
        ]
        .into(),
    );

    let filtered = builtin_filter(&mut test_vm(), vec![arr, builtin("first")]).unwrap();
    assert_eq!(
        filtered,
        Value::Array(
            vec![
                Value::Array(vec![Value::Integer(1)].into()),
                Value::Array(vec![Value::Integer(2), Value::Integer(3)].into())
            ]
            .into()
        )
    );

    let empty = builtin_filter(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), builtin("first")],
    )
    .unwrap();
    assert_eq!(empty, Value::Array(vec![].into()));
}

#[test]
fn fold_with_builtin_callback_and_empty_input() {
    let arr = Value::Array(
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Array(vec![Value::Integer(2), Value::Integer(3)].into()),
            Value::Array(vec![Value::Integer(4)].into()),
        ]
        .into(),
    );

    let folded = builtin_fold(
        &mut test_vm(),
        vec![arr, Value::Array(vec![].into()), builtin("concat")],
    )
    .unwrap();
    assert_eq!(
        folded,
        Value::Array(
            vec![
                Value::Integer(1),
                Value::Integer(2),
                Value::Integer(3),
                Value::Integer(4)
            ]
            .into()
        )
    );

    let init = Value::String("seed".into());
    let empty_fold = builtin_fold(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), init.clone(), builtin("concat")],
    )
    .unwrap();
    assert_eq!(empty_fold, init);
}

#[test]
fn map_filter_fold_reject_non_callable_callback() {
    let map_err = builtin_map(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Integer(42),
        ],
    )
    .unwrap_err();
    assert!(map_err.contains("to be Function"));

    let filter_err = builtin_filter(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Boolean(true),
        ],
    )
    .unwrap_err();
    assert!(filter_err.contains("to be Function"));

    let fold_err = builtin_fold(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Integer(0),
            Value::String("nope".into()),
        ],
    )
    .unwrap_err();
    assert!(fold_err.contains("to be Function"));
}

#[test]
fn map_filter_fold_propagate_callback_arity_errors() {
    let map_err = builtin_map(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            builtin("concat"),
        ],
    )
    .unwrap_err();
    assert!(map_err.contains("wrong number of arguments"));

    let filter_err = builtin_filter(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            builtin("concat"),
        ],
    )
    .unwrap_err();
    assert!(filter_err.contains("wrong number of arguments"));

    let fold_err = builtin_fold(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Integer(0),
            builtin("len"),
        ],
    )
    .unwrap_err();
    assert!(fold_err.contains("wrong number of arguments"));
}
