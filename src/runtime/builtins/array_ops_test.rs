use crate::runtime::object::Object;

use super::array_ops::{
    builtin_concat, builtin_contains, builtin_first, builtin_last, builtin_len, builtin_push,
    builtin_rest, builtin_reverse, builtin_slice, builtin_sort,
};

#[test]
fn len_works_for_string_and_array() {
    let result = builtin_len(&[Object::String("abc".to_string())]).unwrap();
    assert_eq!(result, Object::Integer(3));

    let result =
        builtin_len(&[Object::Array(vec![Object::Integer(1), Object::Integer(2)])]).unwrap();
    assert_eq!(result, Object::Integer(2));
}

#[test]
fn first_last_rest_work() {
    let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
    let first = builtin_first(&[arr.clone()]).unwrap();
    assert_eq!(first, Object::Integer(1));

    let last = builtin_last(&[arr.clone()]).unwrap();
    assert_eq!(last, Object::Integer(2));

    let rest = builtin_rest(&[arr]).unwrap();
    assert_eq!(rest, Object::Array(vec![Object::Integer(2)]));
}

#[test]
fn push_concat_reverse_contains_slice() {
    let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);

    let pushed = builtin_push(&[arr.clone(), Object::Integer(3)]).unwrap();
    assert_eq!(
        pushed,
        Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3)
        ])
    );

    let concat = builtin_concat(&[arr.clone(), Object::Array(vec![Object::Integer(3)])]).unwrap();
    assert_eq!(
        concat,
        Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3)
        ])
    );

    let reversed = builtin_reverse(&[arr.clone()]).unwrap();
    assert_eq!(
        reversed,
        Object::Array(vec![Object::Integer(2), Object::Integer(1)])
    );

    let contains = builtin_contains(&[arr.clone(), Object::Integer(2)]).unwrap();
    assert_eq!(contains, Object::Boolean(true));

    let sliced = builtin_slice(&[arr, Object::Integer(0), Object::Integer(1)]).unwrap();
    assert_eq!(sliced, Object::Array(vec![Object::Integer(1)]));
}

#[test]
fn sort_default_and_desc() {
    let arr = Object::Array(vec![
        Object::Integer(3),
        Object::Integer(1),
        Object::Integer(2),
    ]);
    let sorted = builtin_sort(&[arr.clone()]).unwrap();
    assert_eq!(
        sorted,
        Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3)
        ])
    );

    let sorted_desc = builtin_sort(&[arr, Object::String("desc".to_string())]).unwrap();
    assert_eq!(
        sorted_desc,
        Object::Array(vec![
            Object::Integer(3),
            Object::Integer(2),
            Object::Integer(1)
        ])
    );
}

#[test]
fn sort_rejects_bad_order() {
    let arr = Object::Array(vec![Object::Integer(1)]);
    let err = builtin_sort(&[arr, Object::String("down".to_string())]).unwrap_err();
    assert!(err.contains("sort order"));
}
