use crate::runtime::object::Object;

use super::type_check::{
    builtin_is_array, builtin_is_bool, builtin_is_float, builtin_is_hash, builtin_is_int,
    builtin_is_none, builtin_is_some, builtin_is_string, builtin_type_of,
};

#[test]
fn type_of_returns_type_name() {
    let result = builtin_type_of(vec![Object::Integer(1)]).unwrap();
    assert_eq!(result, Object::String("Int".to_string()));
}

#[test]
fn is_type_checks_values() {
    assert_eq!(
        builtin_is_int(vec![Object::Integer(1)]).unwrap(),
        Object::Boolean(true)
    );
    assert_eq!(
        builtin_is_float(vec![Object::Float(1.0)]).unwrap(),
        Object::Boolean(true)
    );
    assert_eq!(
        builtin_is_string(vec![Object::String("s".to_string())]).unwrap(),
        Object::Boolean(true)
    );
    assert_eq!(
        builtin_is_bool(vec![Object::Boolean(true)]).unwrap(),
        Object::Boolean(true)
    );
    assert_eq!(
        builtin_is_array(vec![Object::Array(vec![])]).unwrap(),
        Object::Boolean(true)
    );
    assert_eq!(
        builtin_is_hash(vec![Object::Hash(Default::default())]).unwrap(),
        Object::Boolean(true)
    );
    assert_eq!(
        builtin_is_none(vec![Object::None]).unwrap(),
        Object::Boolean(true)
    );
    assert_eq!(
        builtin_is_some(vec![Object::Some(Box::new(Object::Integer(1)))]).unwrap(),
        Object::Boolean(true)
    );
}
