use std::collections::HashMap;

use crate::runtime::{hash_key::HashKey, object::Object};

use super::hash_ops::{builtin_has_key, builtin_keys, builtin_merge, builtin_values};

#[test]
fn keys_and_values_return_arrays() {
    let mut map = HashMap::new();
    map.insert(HashKey::String("a".to_string()), Object::Integer(1));
    map.insert(HashKey::Integer(2), Object::String("b".to_string()));

    let keys = builtin_keys(&[Object::Hash(map.clone())]).unwrap();
    let values = builtin_values(&[Object::Hash(map)]).unwrap();

    match keys {
        Object::Array(items) => {
            assert!(items.contains(&Object::String("a".to_string())));
            assert!(items.contains(&Object::Integer(2)));
        }
        _ => panic!("expected array of keys"),
    }

    match values {
        Object::Array(items) => {
            assert!(items.contains(&Object::Integer(1)));
            assert!(items.contains(&Object::String("b".to_string())));
        }
        _ => panic!("expected array of values"),
    }
}

#[test]
fn has_key_and_merge_work() {
    let mut map = HashMap::new();
    map.insert(HashKey::String("k".to_string()), Object::Integer(1));

    let has =
        builtin_has_key(&[Object::Hash(map.clone()), Object::String("k".to_string())]).unwrap();
    assert_eq!(has, Object::Boolean(true));

    let mut map2 = HashMap::new();
    map2.insert(HashKey::String("k".to_string()), Object::Integer(2));

    let merged = builtin_merge(&[Object::Hash(map), Object::Hash(map2)]).unwrap();
    match merged {
        Object::Hash(map) => {
            assert_eq!(
                map.get(&HashKey::String("k".to_string())),
                Some(&Object::Integer(2))
            );
        }
        _ => panic!("expected hash"),
    }
}
