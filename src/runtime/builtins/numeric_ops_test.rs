use crate::runtime::value::Value;

use super::numeric_ops::{builtin_abs, builtin_max, builtin_min};

#[test]
fn abs_handles_int_and_float() {
    let result = builtin_abs(vec![Value::Integer(-5)]).unwrap();
    assert_eq!(result, Value::Integer(5));

    let result = builtin_abs(vec![Value::Float(-2.5)]).unwrap();
    assert_eq!(result, Value::Float(2.5));
}

#[test]
fn min_and_max_return_expected_types() {
    let min = builtin_min(vec![Value::Integer(1), Value::Integer(2)]).unwrap();
    assert_eq!(min, Value::Integer(1));

    let max = builtin_max(vec![Value::Integer(1), Value::Float(2.5)]).unwrap();
    assert_eq!(max, Value::Float(2.5));
}

#[test]
fn abs_rejects_non_number() {
    let err = builtin_abs(vec![Value::Boolean(true)]).unwrap_err();
    assert!(err.contains("Number"));
}
