use crate::runtime::object::Object;

use super::numeric_ops::{builtin_abs, builtin_max, builtin_min};

#[test]
fn abs_handles_int_and_float() {
    let result = builtin_abs(&[Object::Integer(-5)]).unwrap();
    assert_eq!(result, Object::Integer(5));

    let result = builtin_abs(&[Object::Float(-2.5)]).unwrap();
    assert_eq!(result, Object::Float(2.5));
}

#[test]
fn min_and_max_return_expected_types() {
    let min = builtin_min(&[Object::Integer(1), Object::Integer(2)]).unwrap();
    assert_eq!(min, Object::Integer(1));

    let max = builtin_max(&[Object::Integer(1), Object::Float(2.5)]).unwrap();
    assert_eq!(max, Object::Float(2.5));
}

#[test]
fn abs_rejects_non_number() {
    let err = builtin_abs(&[Object::Boolean(true)]).unwrap_err();
    assert!(err.contains("Number"));
}
