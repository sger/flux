use crate::{
    bytecode::{bytecode::Bytecode, vm::VM},
    runtime::{hamt as rc_hamt, hash_key::HashKey, value::Value},
};

use super::hash_ops::{base_delete, base_has_key, base_keys, base_merge, base_values};

fn test_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

#[test]
fn keys_and_values_return_arrays() {
    let mut vm = test_vm();
    let mut root = rc_hamt::hamt_empty();
    root = rc_hamt::hamt_insert(&root, HashKey::String("a".to_string()), Value::Integer(1));
    root = rc_hamt::hamt_insert(
        &root,
        HashKey::Integer(2),
        Value::String("b".to_string().into()),
    );
    let map_value = Value::HashMap(root);

    let keys = base_keys(&mut vm, vec![map_value.clone()]).unwrap();
    let values = base_values(&mut vm, vec![map_value]).unwrap();

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
    let mut vm = test_vm();
    let mut root = rc_hamt::hamt_empty();
    root = rc_hamt::hamt_insert(&root, HashKey::String("k".to_string()), Value::Integer(1));
    let map_value = Value::HashMap(root);

    let has = base_has_key(
        &mut vm,
        vec![map_value.clone(), Value::String("k".to_string().into())],
    )
    .unwrap();
    assert_eq!(has, Value::Boolean(true));

    let mut root2 = rc_hamt::hamt_empty();
    root2 = rc_hamt::hamt_insert(&root2, HashKey::String("k".to_string()), Value::Integer(2));
    let map_value2 = Value::HashMap(root2);

    let merged = base_merge(&mut vm, vec![map_value, map_value2]).unwrap();
    match merged {
        Value::HashMap(node) => {
            assert_eq!(
                rc_hamt::hamt_lookup(&node, &HashKey::String("k".to_string())),
                Some(Value::Integer(2))
            );
        }
        _ => panic!("expected HashMap"),
    }
}

#[test]
fn delete_removes_existing_key_and_keeps_missing() {
    let mut vm = test_vm();
    let mut root = rc_hamt::hamt_empty();
    root = rc_hamt::hamt_insert(&root, HashKey::String("k".to_string()), Value::Integer(1));
    root = rc_hamt::hamt_insert(&root, HashKey::String("x".to_string()), Value::Integer(2));
    let map_value = Value::HashMap(root);

    let deleted = base_delete(
        &mut vm,
        vec![map_value.clone(), Value::String("k".to_string().into())],
    )
    .unwrap();

    match deleted {
        Value::HashMap(node) => {
            assert_eq!(
                rc_hamt::hamt_lookup(&node, &HashKey::String("k".to_string())),
                None
            );
            assert_eq!(
                rc_hamt::hamt_lookup(&node, &HashKey::String("x".to_string())),
                Some(Value::Integer(2))
            );
        }
        _ => panic!("expected HashMap"),
    }

    let deleted_missing = base_delete(
        &mut vm,
        vec![map_value, Value::String("missing".to_string().into())],
    )
    .unwrap();

    match deleted_missing {
        Value::HashMap(node) => assert_eq!(rc_hamt::hamt_len(&node), 2),
        _ => panic!("expected HashMap"),
    }
}
