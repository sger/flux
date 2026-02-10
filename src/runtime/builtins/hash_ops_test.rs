use std::collections::HashMap;

use crate::runtime::{hash_key::HashKey, value::Value};

use super::hash_ops::{
    builtin_delete, builtin_has_key, builtin_keys, builtin_merge, builtin_values,
};

#[test]
fn keys_and_values_return_arrays() {
    let mut map = HashMap::new();
    map.insert(HashKey::String("a".to_string()), Value::Integer(1));
    map.insert(HashKey::Integer(2), Value::String("b".to_string().into()));

    let keys = builtin_keys(&[Value::Hash(map.clone().into())]).unwrap();
    let values = builtin_values(&[Value::Hash(map.into())]).unwrap();

    match keys {
        Value::Array(items) => {
            assert!(items.contains(&Value::String("a".to_string().into())));
            assert!(items.contains(&Value::Integer(2)));
        }
        _ => panic!("expected array of keys"),
    }

    match values {
        Value::Array(items) => {
            assert!(items.contains(&Value::Integer(1)));
            assert!(items.contains(&Value::String("b".to_string().into())));
        }
        _ => panic!("expected array of values"),
    }
}

#[test]
fn has_key_and_merge_work() {
    let mut map = HashMap::new();
    map.insert(HashKey::String("k".to_string()), Value::Integer(1));

    let has = builtin_has_key(&[
        Value::Hash(map.clone().into()),
        Value::String("k".to_string().into()),
    ])
    .unwrap();
    assert_eq!(has, Value::Boolean(true));

    let mut map2 = HashMap::new();
    map2.insert(HashKey::String("k".to_string()), Value::Integer(2));

    let merged = builtin_merge(&[Value::Hash(map.into()), Value::Hash(map2.into())]).unwrap();
    match merged {
        Value::Hash(map) => {
            assert_eq!(
                map.get(&HashKey::String("k".to_string())),
                Some(&Value::Integer(2))
            );
        }
        _ => panic!("expected hash"),
    }
}

#[test]
fn delete_removes_existing_key_and_keeps_missing() {
    let mut map = HashMap::new();
    map.insert(HashKey::String("k".to_string()), Value::Integer(1));
    map.insert(HashKey::String("x".to_string()), Value::Integer(2));

    let deleted = builtin_delete(&[
        Value::Hash(map.clone().into()),
        Value::String("k".to_string().into()),
    ])
    .unwrap();

    match deleted {
        Value::Hash(result) => {
            assert_eq!(result.get(&HashKey::String("k".to_string())), None);
            assert_eq!(
                result.get(&HashKey::String("x".to_string())),
                Some(&Value::Integer(2))
            );
        }
        _ => panic!("expected hash"),
    }

    let deleted_missing = builtin_delete(&[
        Value::Hash(map.into()),
        Value::String("missing".to_string().into()),
    ])
    .unwrap();

    match deleted_missing {
        Value::Hash(result) => assert_eq!(result.len(), 2),
        _ => panic!("expected hash"),
    }
}
