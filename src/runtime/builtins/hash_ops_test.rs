use crate::{
    bytecode::bytecode::Bytecode,
    runtime::{
        gc::hamt::{hamt_empty, hamt_insert, hamt_len, hamt_lookup},
        hash_key::HashKey,
        value::Value,
        vm::VM,
    },
};

use super::hash_ops::{
    builtin_delete, builtin_has_key, builtin_keys, builtin_merge, builtin_values,
};

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
    let mut root = hamt_empty(&mut vm.gc_heap);
    root = hamt_insert(
        &mut vm.gc_heap,
        root,
        HashKey::String("a".to_string()),
        Value::Integer(1),
    );
    root = hamt_insert(
        &mut vm.gc_heap,
        root,
        HashKey::Integer(2),
        Value::String("b".to_string().into()),
    );
    let map_value = Value::Gc(root);

    let keys = builtin_keys(&mut vm, vec![map_value.clone()]).unwrap();
    let values = builtin_values(&mut vm, vec![map_value]).unwrap();

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
    let mut root = hamt_empty(&mut vm.gc_heap);
    root = hamt_insert(
        &mut vm.gc_heap,
        root,
        HashKey::String("k".to_string()),
        Value::Integer(1),
    );
    let map_value = Value::Gc(root);

    let has = builtin_has_key(
        &mut vm,
        vec![map_value.clone(), Value::String("k".to_string().into())],
    )
    .unwrap();
    assert_eq!(has, Value::Boolean(true));

    let mut root2 = hamt_empty(&mut vm.gc_heap);
    root2 = hamt_insert(
        &mut vm.gc_heap,
        root2,
        HashKey::String("k".to_string()),
        Value::Integer(2),
    );
    let map_value2 = Value::Gc(root2);

    let merged = builtin_merge(&mut vm, vec![map_value, map_value2]).unwrap();
    match merged {
        Value::Gc(handle) => {
            assert_eq!(
                hamt_lookup(&vm.gc_heap, handle, &HashKey::String("k".to_string())),
                Some(Value::Integer(2))
            );
        }
        _ => panic!("expected Gc hash"),
    }
}

#[test]
fn delete_removes_existing_key_and_keeps_missing() {
    let mut vm = test_vm();
    let mut root = hamt_empty(&mut vm.gc_heap);
    root = hamt_insert(
        &mut vm.gc_heap,
        root,
        HashKey::String("k".to_string()),
        Value::Integer(1),
    );
    root = hamt_insert(
        &mut vm.gc_heap,
        root,
        HashKey::String("x".to_string()),
        Value::Integer(2),
    );
    let map_value = Value::Gc(root);

    let deleted = builtin_delete(
        &mut vm,
        vec![map_value.clone(), Value::String("k".to_string().into())],
    )
    .unwrap();

    match deleted {
        Value::Gc(handle) => {
            assert_eq!(
                hamt_lookup(&vm.gc_heap, handle, &HashKey::String("k".to_string())),
                None
            );
            assert_eq!(
                hamt_lookup(&vm.gc_heap, handle, &HashKey::String("x".to_string())),
                Some(Value::Integer(2))
            );
        }
        _ => panic!("expected Gc hash"),
    }

    let deleted_missing = builtin_delete(
        &mut vm,
        vec![map_value, Value::String("missing".to_string().into())],
    )
    .unwrap();

    match deleted_missing {
        Value::Gc(handle) => assert_eq!(hamt_len(&vm.gc_heap, handle), 2),
        _ => panic!("expected Gc hash"),
    }
}
