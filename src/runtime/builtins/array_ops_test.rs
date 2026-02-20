use crate::{
    bytecode::bytecode::Bytecode,
    runtime::{builtins::get_builtin_index, value::Value, vm::VM},
};

use super::array_ops::{
    builtin_all, builtin_any, builtin_concat, builtin_contains, builtin_count, builtin_filter,
    builtin_find, builtin_first, builtin_flatten, builtin_fold, builtin_last, builtin_len,
    builtin_map, builtin_push, builtin_rest, builtin_reverse, builtin_slice, builtin_sort,
    builtin_sort_by, builtin_zip,
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
fn any_short_circuits_and_empty() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into());

    // any with is_int — all are ints, so true
    let result = builtin_any(&mut test_vm(), vec![arr.clone(), builtin("is_int")]).unwrap();
    assert_eq!(result, Value::Boolean(true));

    // any with is_string — none are strings, so false
    let result = builtin_any(&mut test_vm(), vec![arr.clone(), builtin("is_string")]).unwrap();
    assert_eq!(result, Value::Boolean(false));

    // any on empty array returns false
    let result = builtin_any(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), builtin("is_int")],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(false));

    // any on None returns false
    let result = builtin_any(&mut test_vm(), vec![Value::None, builtin("is_int")]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn all_short_circuits_and_empty() {
    let all_ints =
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into());
    let mixed = Value::Array(
        vec![
            Value::Integer(1),
            Value::String("x".into()),
            Value::Integer(3),
        ]
        .into(),
    );

    // all with is_int on all-int array — true
    let result = builtin_all(&mut test_vm(), vec![all_ints.clone(), builtin("is_int")]).unwrap();
    assert_eq!(result, Value::Boolean(true));

    // all with is_int on mixed array — false
    let result = builtin_all(&mut test_vm(), vec![mixed.clone(), builtin("is_int")]).unwrap();
    assert_eq!(result, Value::Boolean(false));

    // all on empty array returns true (vacuous truth)
    let result = builtin_all(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), builtin("is_int")],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(true));

    // all on None returns true (vacuous truth)
    let result = builtin_all(&mut test_vm(), vec![Value::None, builtin("is_int")]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn find_returns_some_or_none() {
    let arr = Value::Array(
        vec![
            Value::String("hello".into()),
            Value::Integer(42),
            Value::String("world".into()),
        ]
        .into(),
    );

    // find first integer — returns Some(42)
    let result = builtin_find(&mut test_vm(), vec![arr.clone(), builtin("is_int")]).unwrap();
    assert_eq!(result, Value::Some(std::rc::Rc::new(Value::Integer(42))));

    // find string — returns Some("hello"), the first one
    let result = builtin_find(&mut test_vm(), vec![arr.clone(), builtin("is_string")]).unwrap();
    assert_eq!(
        result,
        Value::Some(std::rc::Rc::new(Value::String("hello".into())))
    );

    // find boolean — nothing matches, returns None
    let result = builtin_find(&mut test_vm(), vec![arr.clone(), builtin("is_bool")]).unwrap();
    assert_eq!(result, Value::None);

    // find on empty array returns None
    let result = builtin_find(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), builtin("is_int")],
    )
    .unwrap();
    assert_eq!(result, Value::None);

    // find on None returns None
    let result = builtin_find(&mut test_vm(), vec![Value::None, builtin("is_int")]).unwrap();
    assert_eq!(result, Value::None);
}

#[test]
fn sort_by_integers_strings_and_empty() {
    // Sort integers by negative value (descending via key)
    let arr = Value::Array(vec![Value::Integer(3), Value::Integer(1), Value::Integer(2)].into());
    let sorted = builtin_sort_by(&mut test_vm(), vec![arr, builtin("abs")]).unwrap();
    assert_eq!(
        sorted,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    // Sort strings lexicographically using len as key
    let strs = Value::Array(
        vec![
            Value::String("banana".into()),
            Value::String("fig".into()),
            Value::String("apple".into()),
        ]
        .into(),
    );
    let sorted = builtin_sort_by(&mut test_vm(), vec![strs, builtin("len")]).unwrap();
    assert_eq!(
        sorted,
        Value::Array(
            vec![
                Value::String("fig".into()),
                Value::String("apple".into()),
                Value::String("banana".into()),
            ]
            .into()
        )
    );

    // sort_by on empty array returns empty
    let result = builtin_sort_by(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), builtin("len")],
    )
    .unwrap();
    assert_eq!(result, Value::Array(vec![].into()));
}

#[test]
fn any_all_find_sort_by_reject_non_callable() {
    let arr = Value::Array(vec![Value::Integer(1)].into());

    let err = builtin_any(&mut test_vm(), vec![arr.clone(), Value::Integer(1)]).unwrap_err();
    assert!(err.contains("to be Function"));

    let err = builtin_all(&mut test_vm(), vec![arr.clone(), Value::Integer(1)]).unwrap_err();
    assert!(err.contains("to be Function"));

    let err = builtin_find(&mut test_vm(), vec![arr.clone(), Value::Integer(1)]).unwrap_err();
    assert!(err.contains("to be Function"));

    let err = builtin_sort_by(&mut test_vm(), vec![arr.clone(), Value::Integer(1)]).unwrap_err();
    assert!(err.contains("to be Function"));
}

#[test]
fn zip_pairs_and_stops_at_shorter() {
    let xs = Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into());
    let ys = Value::Array(
        vec![
            Value::String("a".into()),
            Value::String("b".into()),
            Value::String("c".into()),
        ]
        .into(),
    );

    let result = builtin_zip(&mut test_vm(), vec![xs.clone(), ys.clone()]).unwrap();
    assert_eq!(
        result,
        Value::Array(
            vec![
                Value::Tuple(std::rc::Rc::new(vec![
                    Value::Integer(1),
                    Value::String("a".into())
                ])),
                Value::Tuple(std::rc::Rc::new(vec![
                    Value::Integer(2),
                    Value::String("b".into())
                ])),
                Value::Tuple(std::rc::Rc::new(vec![
                    Value::Integer(3),
                    Value::String("c".into())
                ])),
            ]
            .into()
        )
    );

    // stops at shorter — xs has 3 elements, ys has 2
    let ys_short = Value::Array(vec![Value::String("a".into()), Value::String("b".into())].into());
    let result = builtin_zip(&mut test_vm(), vec![xs.clone(), ys_short]).unwrap();
    match result {
        Value::Array(arr) => assert_eq!(arr.len(), 2),
        other => panic!("expected Array, got {:?}", other),
    };

    // empty input returns empty
    let result = builtin_zip(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), xs.clone()],
    )
    .unwrap();
    assert_eq!(result, Value::Array(vec![].into()));
}

#[test]
fn flatten_one_level() {
    let nested = Value::Array(
        vec![
            Value::Array(vec![Value::Integer(1), Value::Integer(2)].into()),
            Value::Array(vec![Value::Integer(3)].into()),
            Value::Array(vec![Value::Integer(4), Value::Integer(5)].into()),
        ]
        .into(),
    );

    let result = builtin_flatten(&mut test_vm(), vec![nested]).unwrap();
    assert_eq!(
        result,
        Value::Array(
            vec![
                Value::Integer(1),
                Value::Integer(2),
                Value::Integer(3),
                Value::Integer(4),
                Value::Integer(5),
            ]
            .into()
        )
    );

    // empty inner arrays are skipped
    let with_empty = Value::Array(
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Array(vec![].into()),
            Value::Array(vec![Value::Integer(2)].into()),
        ]
        .into(),
    );
    let result = builtin_flatten(&mut test_vm(), vec![with_empty]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Integer(1), Value::Integer(2)].into())
    );

    // flatten empty outer returns empty
    let result = builtin_flatten(&mut test_vm(), vec![Value::Array(vec![].into())]).unwrap();
    assert_eq!(result, Value::Array(vec![].into()));
}

#[test]
fn count_matches_and_empty() {
    let arr = Value::Array(
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
        ]
        .into(),
    );

    // count integers — all 4 match
    let result = builtin_count(&mut test_vm(), vec![arr.clone(), builtin("is_int")]).unwrap();
    assert_eq!(result, Value::Integer(4));

    // count strings — none match
    let result = builtin_count(&mut test_vm(), vec![arr.clone(), builtin("is_string")]).unwrap();
    assert_eq!(result, Value::Integer(0));

    // count on empty returns 0
    let result = builtin_count(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), builtin("is_int")],
    )
    .unwrap();
    assert_eq!(result, Value::Integer(0));

    // count on None returns 0
    let result = builtin_count(&mut test_vm(), vec![Value::None, builtin("is_int")]).unwrap();
    assert_eq!(result, Value::Integer(0));

    // count rejects non-callable
    let err = builtin_count(&mut test_vm(), vec![arr.clone(), Value::Integer(1)]).unwrap_err();
    assert!(err.contains("to be Function"));
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
