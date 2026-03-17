use crate::{
    bytecode::bytecode::Bytecode,
    runtime::{base::get_base_function_index, value::Value, vm::VM},
};

use super::higher_order_ops::{
    base_all, base_any, base_count, base_filter, base_find, base_flatten, base_fold, base_map,
    base_sort_by, base_zip,
};

fn test_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

fn base_fn(name: &str) -> Value {
    Value::BaseFunction(get_base_function_index(name).expect("base function exists") as u8)
}

#[test]
fn map_with_base_callback_and_empty_input() {
    let arr = Value::Array(
        vec![
            Value::String("a".to_string().into()),
            Value::String("hello".to_string().into()),
            Value::String("xyz".to_string().into()),
        ]
        .into(),
    );

    let mapped = base_map(&mut test_vm(), vec![arr, base_fn("len")]).unwrap();
    assert_eq!(
        mapped,
        Value::Array(vec![Value::Integer(1), Value::Integer(5), Value::Integer(3)].into())
    );

    let empty = base_map(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), base_fn("len")],
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

    let filtered = base_filter(&mut test_vm(), vec![arr, base_fn("first")]).unwrap();
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

    let empty = base_filter(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), base_fn("first")],
    )
    .unwrap();
    assert_eq!(empty, Value::Array(vec![].into()));
}

#[test]
fn fold_with_base_callback_and_empty_input() {
    let arr = Value::Array(
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Array(vec![Value::Integer(2), Value::Integer(3)].into()),
            Value::Array(vec![Value::Integer(4)].into()),
        ]
        .into(),
    );

    let folded = base_fold(
        &mut test_vm(),
        vec![arr, Value::Array(vec![].into()), base_fn("concat")],
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

    let init = Value::String("seed".to_string().into());
    let empty_fold = base_fold(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), init.clone(), base_fn("concat")],
    )
    .unwrap();
    assert_eq!(empty_fold, init);
}

#[test]
fn map_filter_fold_reject_non_callable_callback() {
    let map_err = base_map(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Integer(42),
        ],
    )
    .unwrap_err();
    assert!(map_err.contains("wrong runtime type"));
    assert!(map_err.contains("expected type: Function"));

    let filter_err = base_filter(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Boolean(true),
        ],
    )
    .unwrap_err();
    assert!(filter_err.contains("wrong runtime type"));
    assert!(filter_err.contains("expected type: Function"));

    let fold_err = base_fold(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Integer(0),
            Value::String("nope".to_string().into()),
        ],
    )
    .unwrap_err();
    assert!(fold_err.contains("wrong runtime type"));
    assert!(fold_err.contains("expected type: Function"));
}

#[test]
fn any_short_circuits_and_empty() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into());

    // any with is_int — all are ints, so true
    let result = base_any(&mut test_vm(), vec![arr.clone(), base_fn("is_int")]).unwrap();
    assert_eq!(result, Value::Boolean(true));

    // any with is_string — none are strings, so false
    let result = base_any(&mut test_vm(), vec![arr.clone(), base_fn("is_string")]).unwrap();
    assert_eq!(result, Value::Boolean(false));

    // any on empty array returns false
    let result = base_any(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), base_fn("is_int")],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(false));

    // any on None returns false
    let result = base_any(&mut test_vm(), vec![Value::None, base_fn("is_int")]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn all_short_circuits_and_empty() {
    let all_ints =
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into());
    let mixed = Value::Array(
        vec![
            Value::Integer(1),
            Value::String("x".to_string().into()),
            Value::Integer(3),
        ]
        .into(),
    );

    // all with is_int on all-int array — true
    let result = base_all(&mut test_vm(), vec![all_ints.clone(), base_fn("is_int")]).unwrap();
    assert_eq!(result, Value::Boolean(true));

    // all with is_int on mixed array — false
    let result = base_all(&mut test_vm(), vec![mixed.clone(), base_fn("is_int")]).unwrap();
    assert_eq!(result, Value::Boolean(false));

    // all on empty array returns true (vacuous truth)
    let result = base_all(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), base_fn("is_int")],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(true));

    // all on None returns true (vacuous truth)
    let result = base_all(&mut test_vm(), vec![Value::None, base_fn("is_int")]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn find_returns_some_or_none() {
    let arr = Value::Array(
        vec![
            Value::String("hello".to_string().into()),
            Value::Integer(42),
            Value::String("world".to_string().into()),
        ]
        .into(),
    );

    // find first integer — returns Some(42)
    let result = base_find(&mut test_vm(), vec![arr.clone(), base_fn("is_int")]).unwrap();
    assert_eq!(result, Value::Some(std::rc::Rc::new(Value::Integer(42))));

    // find string — returns Some("hello"), the first one
    let result = base_find(&mut test_vm(), vec![arr.clone(), base_fn("is_string")]).unwrap();
    assert_eq!(
        result,
        Value::Some(std::rc::Rc::new(Value::String("hello".to_string().into())))
    );

    // find boolean — nothing matches, returns None
    let result = base_find(&mut test_vm(), vec![arr.clone(), base_fn("is_bool")]).unwrap();
    assert_eq!(result, Value::None);

    // find on empty array returns None
    let result = base_find(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), base_fn("is_int")],
    )
    .unwrap();
    assert_eq!(result, Value::None);

    // find on None returns None
    let result = base_find(&mut test_vm(), vec![Value::None, base_fn("is_int")]).unwrap();
    assert_eq!(result, Value::None);
}

#[test]
fn sort_by_integers_strings_and_empty() {
    // Sort integers by negative value (descending via key)
    let arr = Value::Array(vec![Value::Integer(3), Value::Integer(1), Value::Integer(2)].into());
    let sorted = base_sort_by(&mut test_vm(), vec![arr, base_fn("abs")]).unwrap();
    assert_eq!(
        sorted,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );

    // Sort strings lexicographically using len as key
    let strs = Value::Array(
        vec![
            Value::String("banana".to_string().into()),
            Value::String("fig".to_string().into()),
            Value::String("apple".to_string().into()),
        ]
        .into(),
    );
    let sorted = base_sort_by(&mut test_vm(), vec![strs, base_fn("len")]).unwrap();
    assert_eq!(
        sorted,
        Value::Array(
            vec![
                Value::String("fig".to_string().into()),
                Value::String("apple".to_string().into()),
                Value::String("banana".to_string().into()),
            ]
            .into()
        )
    );

    // sort_by on empty array returns empty
    let result = base_sort_by(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), base_fn("len")],
    )
    .unwrap();
    assert_eq!(result, Value::Array(vec![].into()));
}

#[test]
fn any_all_find_sort_by_reject_non_callable() {
    let arr = Value::Array(vec![Value::Integer(1)].into());

    let err = base_any(&mut test_vm(), vec![arr.clone(), Value::Integer(1)]).unwrap_err();
    assert!(err.contains("wrong runtime type"));
    assert!(err.contains("expected type: Function"));

    let err = base_all(&mut test_vm(), vec![arr.clone(), Value::Integer(1)]).unwrap_err();
    assert!(err.contains("wrong runtime type"));
    assert!(err.contains("expected type: Function"));

    let err = base_find(&mut test_vm(), vec![arr.clone(), Value::Integer(1)]).unwrap_err();
    assert!(err.contains("wrong runtime type"));
    assert!(err.contains("expected type: Function"));

    let err = base_sort_by(&mut test_vm(), vec![arr.clone(), Value::Integer(1)]).unwrap_err();
    assert!(err.contains("wrong runtime type"));
    assert!(err.contains("expected type: Function"));
}

#[test]
fn zip_pairs_and_stops_at_shorter() {
    let xs = Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into());
    let ys = Value::Array(
        vec![
            Value::String("a".to_string().into()),
            Value::String("b".to_string().into()),
            Value::String("c".to_string().into()),
        ]
        .into(),
    );

    let result = base_zip(&mut test_vm(), vec![xs.clone(), ys.clone()]).unwrap();
    assert_eq!(
        result,
        Value::Array(
            vec![
                Value::Tuple(std::rc::Rc::new(vec![
                    Value::Integer(1),
                    Value::String("a".to_string().into())
                ])),
                Value::Tuple(std::rc::Rc::new(vec![
                    Value::Integer(2),
                    Value::String("b".to_string().into())
                ])),
                Value::Tuple(std::rc::Rc::new(vec![
                    Value::Integer(3),
                    Value::String("c".to_string().into())
                ])),
            ]
            .into()
        )
    );

    // stops at shorter — xs has 3 elements, ys has 2
    let ys_short = Value::Array(
        vec![
            Value::String("a".to_string().into()),
            Value::String("b".to_string().into()),
        ]
        .into(),
    );
    let result = base_zip(&mut test_vm(), vec![xs.clone(), ys_short]).unwrap();
    match result {
        Value::Array(arr) => assert_eq!(arr.len(), 2),
        other => panic!("expected Array, got {:?}", other),
    };

    // empty input returns empty
    let result = base_zip(
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

    let result = base_flatten(&mut test_vm(), vec![nested]).unwrap();
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
    let result = base_flatten(&mut test_vm(), vec![with_empty]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Integer(1), Value::Integer(2)].into())
    );

    // flatten empty outer returns empty
    let result = base_flatten(&mut test_vm(), vec![Value::Array(vec![].into())]).unwrap();
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
    let result = base_count(&mut test_vm(), vec![arr.clone(), base_fn("is_int")]).unwrap();
    assert_eq!(result, Value::Integer(4));

    // count strings — none match
    let result = base_count(&mut test_vm(), vec![arr.clone(), base_fn("is_string")]).unwrap();
    assert_eq!(result, Value::Integer(0));

    // count on empty returns 0
    let result = base_count(
        &mut test_vm(),
        vec![Value::Array(vec![].into()), base_fn("is_int")],
    )
    .unwrap();
    assert_eq!(result, Value::Integer(0));

    // count on None returns 0
    let result = base_count(&mut test_vm(), vec![Value::None, base_fn("is_int")]).unwrap();
    assert_eq!(result, Value::Integer(0));

    // count rejects non-callable
    let err = base_count(&mut test_vm(), vec![arr.clone(), Value::Integer(1)]).unwrap_err();
    assert!(err.contains("wrong runtime type"));
    assert!(err.contains("expected type: Function"));
}

#[test]
fn map_filter_fold_propagate_callback_arity_errors() {
    let map_err = base_map(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            base_fn("concat"),
        ],
    )
    .unwrap_err();
    assert!(map_err.contains("wrong number of arguments"));

    let filter_err = base_filter(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            base_fn("concat"),
        ],
    )
    .unwrap_err();
    assert!(filter_err.contains("wrong number of arguments"));

    let fold_err = base_fold(
        &mut test_vm(),
        vec![
            Value::Array(vec![Value::Integer(1)].into()),
            Value::Integer(0),
            base_fn("len"),
        ],
    )
    .unwrap_err();
    assert!(fold_err.contains("wrong number of arguments"));
}
