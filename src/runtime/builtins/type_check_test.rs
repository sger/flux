use crate::runtime::value::Value;

use super::type_check::{
    builtin_is_array, builtin_is_bool, builtin_is_float, builtin_is_hash, builtin_is_int,
    builtin_is_none, builtin_is_some, builtin_is_string, builtin_type_of,
};

#[test]
fn type_of_returns_type_name() {
    let result = builtin_type_of(&[Value::Integer(1)]).unwrap();
    assert_eq!(result, Value::String("Int".to_string()));
}

#[test]
fn is_type_checks_values() {
    assert_eq!(
        builtin_is_int(&[Value::Integer(1)]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        builtin_is_float(&[Value::Float(1.0)]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        builtin_is_string(&[Value::String("s".to_string())]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        builtin_is_bool(&[Value::Boolean(true)]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        builtin_is_array(&[Value::Array(vec![])]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        builtin_is_hash(&[Value::Hash(Default::default())]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        builtin_is_none(&[Value::None]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        builtin_is_some(&[Value::Some(Box::new(Value::Integer(1)))]).unwrap(),
        Value::Boolean(true)
    );
}
