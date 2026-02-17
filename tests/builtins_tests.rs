const PI: f64 = std::f64::consts::PI;

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use flux::bytecode::bytecode::Bytecode;
use flux::runtime::RuntimeContext;
use flux::runtime::builtins::{get_builtin, get_builtin_index};
use flux::runtime::gc::GcHeap;
use flux::runtime::gc::hamt::{hamt_empty, hamt_insert, hamt_len, hamt_lookup};
use flux::runtime::hash_key::HashKey;
use flux::runtime::value::Value;
use flux::runtime::vm::VM;

fn test_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

fn call(name: &str, args: Vec<Value>) -> Result<Value, String> {
    let builtin = get_builtin(name).unwrap_or_else(|| panic!("missing builtin: {}", name));
    (builtin.func)(&mut test_vm(), args)
}

fn call_vm(vm: &mut VM, name: &str, args: Vec<Value>) -> Result<Value, String> {
    let builtin = get_builtin(name).unwrap_or_else(|| panic!("missing builtin: {}", name));
    (builtin.func)(vm, args)
}

fn make_test_hash(heap: &mut GcHeap) -> Value {
    let mut root = hamt_empty(heap);
    root = hamt_insert(
        heap,
        root,
        HashKey::String("name".to_string()),
        Value::String("Alice".to_string().into()),
    );
    root = hamt_insert(heap, root, HashKey::Integer(42), Value::Integer(100));
    root = hamt_insert(
        heap,
        root,
        HashKey::Boolean(true),
        Value::String("yes".to_string().into()),
    );
    Value::Gc(root)
}

fn temp_file_path(label: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("flux_builtin_{}_{}.txt", label, nanos));
    path
}

#[test]
fn test_builtin_len_string() {
    let result = call("len", vec![Value::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Value::Integer(5));
}

#[test]
fn test_builtin_len_array() {
    let result = call(
        "len",
        vec![Value::Array(
            vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into(),
        )],
    )
    .unwrap();
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn test_builtin_first() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());
    let result = call("first", vec![arr]).unwrap();
    assert_eq!(result, Value::Integer(1));
}

#[test]
fn test_builtin_last() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());
    let result = call("last", vec![arr]).unwrap();
    assert_eq!(result, Value::Integer(2));
}

#[test]
fn test_builtin_rest() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into());
    let result = call("rest", vec![arr]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Integer(2), Value::Integer(3)].into())
    );
}

#[test]
fn test_builtin_push() {
    let arr = Value::Array(vec![Value::Integer(1)].into());
    let result = call("push", vec![arr, Value::Integer(2)]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Integer(1), Value::Integer(2)].into())
    );
}

#[test]
fn test_get_builtin() {
    assert!(get_builtin("print").is_some());
    assert!(get_builtin("len").is_some());
    assert!(get_builtin("nonexistent").is_none());
}

#[test]
fn test_builtin_concat() {
    let a = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());
    let b = Value::Array(vec![Value::Integer(3), Value::Integer(4)].into());
    let result = call("concat", vec![a, b]).unwrap();
    assert_eq!(
        result,
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
}

#[test]
fn test_builtin_concat_empty() {
    let a = Value::Array(vec![Value::Integer(1)].into());
    let b = Value::Array(vec![].into());
    let result = call("concat", vec![a, b]).unwrap();
    assert_eq!(result, Value::Array(vec![Value::Integer(1)].into()));
}

#[test]
fn test_builtin_reverse() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into());
    let result = call("reverse", vec![arr]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Integer(3), Value::Integer(2), Value::Integer(1)].into())
    );
}

#[test]
fn test_builtin_reverse_empty() {
    let arr = Value::Array(vec![].into());
    let result = call("reverse", vec![arr]).unwrap();
    assert_eq!(result, Value::Array(vec![].into()));
}

#[test]
fn test_builtin_contains_found() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into());
    let result = call("contains", vec![arr, Value::Integer(2)]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_contains_not_found() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into());
    let result = call("contains", vec![arr, Value::Integer(5)]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn test_builtin_slice() {
    let arr = Value::Array(
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
            Value::Integer(5),
        ]
        .into(),
    );
    let result = call("slice", vec![arr, Value::Integer(1), Value::Integer(4)]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Integer(2), Value::Integer(3), Value::Integer(4)].into())
    );
}

#[test]
fn test_builtin_slice_out_of_bounds() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());
    let result = call("slice", vec![arr, Value::Integer(0), Value::Integer(10)]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Integer(1), Value::Integer(2)].into())
    );
}

#[test]
fn test_builtin_sort() {
    let arr = Value::Array(
        vec![
            Value::Integer(3),
            Value::Integer(1),
            Value::Integer(4),
            Value::Integer(1),
            Value::Integer(5),
        ]
        .into(),
    );
    let result = call("sort", vec![arr]).unwrap();
    assert_eq!(
        result,
        Value::Array(
            vec![
                Value::Integer(1),
                Value::Integer(1),
                Value::Integer(3),
                Value::Integer(4),
                Value::Integer(5)
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_sort_floats() {
    let arr = Value::Array(vec![Value::Float(PI), Value::Float(1.0), Value::Float(2.71)].into());
    let result = call("sort", vec![arr]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Float(1.0), Value::Float(2.71), Value::Float(PI)].into())
    );
}

#[test]
fn test_builtin_sort_mixed_numeric() {
    let arr = Value::Array(vec![Value::Integer(3), Value::Float(1.5), Value::Integer(2)].into());
    let result = call("sort", vec![arr]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Float(1.5), Value::Integer(2), Value::Integer(3)].into())
    );
}

#[test]
fn test_builtin_sort_asc_explicit() {
    let arr = Value::Array(vec![Value::Integer(3), Value::Integer(1), Value::Integer(2)].into());
    let result = call("sort", vec![arr, Value::String("asc".to_string().into())]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );
}

#[test]
fn test_builtin_sort_desc() {
    let arr = Value::Array(
        vec![
            Value::Integer(3),
            Value::Integer(1),
            Value::Integer(5),
            Value::Integer(2),
        ]
        .into(),
    );
    let result = call("sort", vec![arr, Value::String("desc".to_string().into())]).unwrap();
    assert_eq!(
        result,
        Value::Array(
            vec![
                Value::Integer(5),
                Value::Integer(3),
                Value::Integer(2),
                Value::Integer(1)
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_sort_desc_floats() {
    let arr = Value::Array(vec![Value::Float(1.0), Value::Float(PI), Value::Float(2.71)].into());
    let result = call("sort", vec![arr, Value::String("desc".to_string().into())]).unwrap();
    assert_eq!(
        result,
        Value::Array(vec![Value::Float(PI), Value::Float(2.71), Value::Float(1.0)].into())
    );
}

#[test]
fn test_builtin_sort_invalid_order() {
    let arr = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());
    let result = call(
        "sort",
        vec![arr, Value::String("invalid".to_string().into())],
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be \"asc\" or \"desc\""));
}

#[test]
fn test_builtin_split() {
    let result = call(
        "split",
        vec![
            Value::String("a,b,c".to_string().into()),
            Value::String(",".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(
        result,
        Value::Array(
            vec![
                Value::String("a".to_string().into()),
                Value::String("b".to_string().into()),
                Value::String("c".to_string().into())
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_split_empty() {
    let result = call(
        "split",
        vec![
            Value::String("hello".to_string().into()),
            Value::String("".to_string().into()),
        ],
    )
    .unwrap();
    // Split by empty string gives each character
    assert_eq!(
        result,
        Value::Array(
            vec![
                Value::String("h".to_string().into()),
                Value::String("e".to_string().into()),
                Value::String("l".to_string().into()),
                Value::String("l".to_string().into()),
                Value::String("o".to_string().into())
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_join() {
    let arr = Value::Array(
        vec![
            Value::String("a".to_string().into()),
            Value::String("b".to_string().into()),
            Value::String("c".to_string().into()),
        ]
        .into(),
    );
    let result = call("join", vec![arr, Value::String(",".to_string().into())]).unwrap();
    assert_eq!(result, Value::String("a,b,c".to_string().into()));
}

#[test]
fn test_builtin_join_empty_delim() {
    let arr = Value::Array(
        vec![
            Value::String("a".to_string().into()),
            Value::String("b".to_string().into()),
        ]
        .into(),
    );
    let result = call("join", vec![arr, Value::String("".to_string().into())]).unwrap();
    assert_eq!(result, Value::String("ab".to_string().into()));
}

#[test]
fn test_builtin_trim() {
    let result = call(
        "trim",
        vec![Value::String("  hello world  ".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Value::String("hello world".to_string().into()));
}

#[test]
fn test_builtin_trim_no_whitespace() {
    let result = call("trim", vec![Value::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Value::String("hello".to_string().into()));
}

#[test]
fn test_builtin_upper() {
    let result = call("upper", vec![Value::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Value::String("HELLO".to_string().into()));
}

#[test]
fn test_builtin_lower() {
    let result = call("lower", vec![Value::String("HELLO".to_string().into())]).unwrap();
    assert_eq!(result, Value::String("hello".to_string().into()));
}

#[test]
fn test_builtin_chars() {
    let result = call("chars", vec![Value::String("abc".to_string().into())]).unwrap();
    assert_eq!(
        result,
        Value::Array(
            vec![
                Value::String("a".to_string().into()),
                Value::String("b".to_string().into()),
                Value::String("c".to_string().into())
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_chars_empty() {
    let result = call("chars", vec![Value::String("".to_string().into())]).unwrap();
    assert_eq!(result, Value::Array(vec![].into()));
}

#[test]
fn test_builtin_substring() {
    let result = call(
        "substring",
        vec![
            Value::String("hello world".to_string().into()),
            Value::Integer(0),
            Value::Integer(5),
        ],
    )
    .unwrap();
    assert_eq!(result, Value::String("hello".to_string().into()));
}

#[test]
fn test_builtin_substring_middle() {
    let result = call(
        "substring",
        vec![
            Value::String("hello world".to_string().into()),
            Value::Integer(6),
            Value::Integer(11),
        ],
    )
    .unwrap();
    assert_eq!(result, Value::String("world".to_string().into()));
}

#[test]
fn test_builtin_substring_out_of_bounds() {
    let result = call(
        "substring",
        vec![
            Value::String("hello".to_string().into()),
            Value::Integer(0),
            Value::Integer(100),
        ],
    )
    .unwrap();
    assert_eq!(result, Value::String("hello".to_string().into()));
}

#[test]
fn test_builtin_starts_with() {
    let result = call(
        "starts_with",
        vec![
            Value::String("hello".to_string().into()),
            Value::String("he".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_ends_with() {
    let result = call(
        "ends_with",
        vec![
            Value::String("hello".to_string().into()),
            Value::String("lo".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_replace() {
    let result = call(
        "replace",
        vec![
            Value::String("banana".to_string().into()),
            Value::String("na".to_string().into()),
            Value::String("X".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(result, Value::String("baXX".to_string().into()));
}

#[test]
fn test_builtin_keys() {
    let mut vm = test_vm();
    let hash = make_test_hash(vm.gc_heap_mut());
    let result = call_vm(&mut vm, "keys", vec![hash]).unwrap();
    match result {
        Value::Array(keys) => {
            assert_eq!(keys.len(), 3);
            // Check that all expected keys are present (order is not guaranteed)
            let has_name = keys.contains(&Value::String("name".to_string().into()));
            let has_42 = keys.contains(&Value::Integer(42));
            let has_true = keys.contains(&Value::Boolean(true));
            assert!(has_name, "missing 'name' key");
            assert!(has_42, "missing 42 key");
            assert!(has_true, "missing true key");
        }
        _ => panic!("expected Array"),
    }
}

#[test]
fn test_builtin_keys_empty() {
    let mut vm = test_vm();
    let root = hamt_empty(vm.gc_heap_mut());
    let hash = Value::Gc(root);
    let result = call_vm(&mut vm, "keys", vec![hash]).unwrap();
    assert_eq!(result, Value::Array(vec![].into()));
}

#[test]
fn test_builtin_values() {
    let mut vm = test_vm();
    let hash = make_test_hash(vm.gc_heap_mut());
    let result = call_vm(&mut vm, "values", vec![hash]).unwrap();
    match result {
        Value::Array(values) => {
            assert_eq!(values.len(), 3);
            // Check that all expected values are present (order is not guaranteed)
            let has_alice = values.contains(&Value::String("Alice".to_string().into()));
            let has_100 = values.contains(&Value::Integer(100));
            let has_yes = values.contains(&Value::String("yes".to_string().into()));
            assert!(has_alice, "missing 'Alice' value");
            assert!(has_100, "missing 100 value");
            assert!(has_yes, "missing 'yes' value");
        }
        _ => panic!("expected Array"),
    }
}

#[test]
fn test_builtin_values_empty() {
    let mut vm = test_vm();
    let root = hamt_empty(vm.gc_heap_mut());
    let hash = Value::Gc(root);
    let result = call_vm(&mut vm, "values", vec![hash]).unwrap();
    assert_eq!(result, Value::Array(vec![].into()));
}

#[test]
fn test_builtin_has_key_found() {
    let mut vm = test_vm();
    let hash = make_test_hash(vm.gc_heap_mut());
    let result = call_vm(
        &mut vm,
        "has_key",
        vec![hash, Value::String("name".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_has_key_not_found() {
    let mut vm = test_vm();
    let hash = make_test_hash(vm.gc_heap_mut());
    let result = call_vm(
        &mut vm,
        "has_key",
        vec![hash, Value::String("email".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn test_builtin_has_key_integer_key() {
    let mut vm = test_vm();
    let hash = make_test_hash(vm.gc_heap_mut());
    let result = call_vm(&mut vm, "has_key", vec![hash, Value::Integer(42)]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_has_key_boolean_key() {
    let mut vm = test_vm();
    let hash = make_test_hash(vm.gc_heap_mut());
    let result = call_vm(&mut vm, "has_key", vec![hash, Value::Boolean(true)]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_has_key_unhashable() {
    let mut vm = test_vm();
    let hash = make_test_hash(vm.gc_heap_mut());
    let result = call_vm(&mut vm, "has_key", vec![hash, Value::Array(vec![].into())]);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be hashable"));
}

#[test]
fn test_builtin_merge() {
    let mut vm = test_vm();
    let heap = vm.gc_heap_mut();

    let mut h1 = hamt_empty(heap);
    h1 = hamt_insert(
        heap,
        h1,
        HashKey::String("a".to_string()),
        Value::Integer(1),
    );
    h1 = hamt_insert(
        heap,
        h1,
        HashKey::String("b".to_string()),
        Value::Integer(2),
    );

    let mut h2 = hamt_empty(heap);
    h2 = hamt_insert(
        heap,
        h2,
        HashKey::String("b".to_string()),
        Value::Integer(20),
    ); // overwrites
    h2 = hamt_insert(
        heap,
        h2,
        HashKey::String("c".to_string()),
        Value::Integer(3),
    );

    let result = call_vm(&mut vm, "merge", vec![Value::Gc(h1), Value::Gc(h2)]).unwrap();
    match result {
        Value::Gc(handle) => {
            let heap = vm.gc_heap_mut();
            assert_eq!(hamt_len(heap, handle), 3);
            assert_eq!(
                hamt_lookup(heap, handle, &HashKey::String("a".to_string())),
                Some(Value::Integer(1))
            );
            assert_eq!(
                hamt_lookup(heap, handle, &HashKey::String("b".to_string())),
                Some(Value::Integer(20))
            ); // overwritten
            assert_eq!(
                hamt_lookup(heap, handle, &HashKey::String("c".to_string())),
                Some(Value::Integer(3))
            );
        }
        _ => panic!("expected Gc hash"),
    }
}

#[test]
fn test_builtin_delete() {
    let mut vm = test_vm();
    let heap = vm.gc_heap_mut();

    let mut h = hamt_empty(heap);
    h = hamt_insert(heap, h, HashKey::String("a".to_string()), Value::Integer(1));
    h = hamt_insert(heap, h, HashKey::String("b".to_string()), Value::Integer(2));

    let result = call_vm(
        &mut vm,
        "delete",
        vec![Value::Gc(h), Value::String("a".to_string().into())],
    )
    .unwrap();

    match result {
        Value::Gc(handle) => {
            let heap = vm.gc_heap_mut();
            assert_eq!(
                hamt_lookup(heap, handle, &HashKey::String("a".to_string())),
                None
            );
            assert_eq!(
                hamt_lookup(heap, handle, &HashKey::String("b".to_string())),
                Some(Value::Integer(2))
            );
        }
        _ => panic!("expected Gc hash"),
    }
}

#[test]
fn test_builtin_merge_empty() {
    let mut vm = test_vm();
    let heap = vm.gc_heap_mut();

    let mut h1 = hamt_empty(heap);
    h1 = hamt_insert(
        heap,
        h1,
        HashKey::String("a".to_string()),
        Value::Integer(1),
    );

    let h2 = hamt_empty(heap);

    let result = call_vm(&mut vm, "merge", vec![Value::Gc(h1), Value::Gc(h2)]).unwrap();
    match result {
        Value::Gc(handle) => {
            let heap = vm.gc_heap_mut();
            assert_eq!(hamt_len(heap, handle), 1);
            assert_eq!(
                hamt_lookup(heap, handle, &HashKey::String("a".to_string())),
                Some(Value::Integer(1))
            );
        }
        _ => panic!("expected Gc hash"),
    }
}

#[test]
fn test_builtin_merge_into_empty() {
    let mut vm = test_vm();
    let heap = vm.gc_heap_mut();

    let h1 = hamt_empty(heap);

    let mut h2 = hamt_empty(heap);
    h2 = hamt_insert(
        heap,
        h2,
        HashKey::String("a".to_string()),
        Value::Integer(1),
    );

    let result = call_vm(&mut vm, "merge", vec![Value::Gc(h1), Value::Gc(h2)]).unwrap();
    match result {
        Value::Gc(handle) => {
            let heap = vm.gc_heap_mut();
            assert_eq!(hamt_len(heap, handle), 1);
            assert_eq!(
                hamt_lookup(heap, handle, &HashKey::String("a".to_string())),
                Some(Value::Integer(1))
            );
        }
        _ => panic!("expected Gc hash"),
    }
}

#[test]
fn test_builtin_abs_integer_positive() {
    let result = call("abs", vec![Value::Integer(5)]).unwrap();
    assert_eq!(result, Value::Integer(5));
}

#[test]
fn test_builtin_abs_integer_negative() {
    let result = call("abs", vec![Value::Integer(-5)]).unwrap();
    assert_eq!(result, Value::Integer(5));
}

#[test]
fn test_builtin_abs_integer_zero() {
    let result = call("abs", vec![Value::Integer(0)]).unwrap();
    assert_eq!(result, Value::Integer(0));
}

#[test]
fn test_builtin_abs_float_positive() {
    let result = call("abs", vec![Value::Float(PI)]).unwrap();
    assert_eq!(result, Value::Float(PI));
}

#[test]
fn test_builtin_abs_float_negative() {
    let result = call("abs", vec![Value::Float(-PI)]).unwrap();
    assert_eq!(result, Value::Float(PI));
}

#[test]
fn test_builtin_abs_type_error() {
    let result = call("abs", vec![Value::String("hello".to_string().into())]);
    assert!(result.is_err());
}

#[test]
fn test_builtin_min_integers() {
    let result = call("min", vec![Value::Integer(3), Value::Integer(7)]).unwrap();
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn test_builtin_min_integers_reversed() {
    let result = call("min", vec![Value::Integer(10), Value::Integer(2)]).unwrap();
    assert_eq!(result, Value::Integer(2));
}

#[test]
fn test_builtin_min_floats() {
    let result = call("min", vec![Value::Float(3.5), Value::Float(2.1)]).unwrap();
    assert_eq!(result, Value::Float(2.1));
}

#[test]
fn test_builtin_min_mixed() {
    let result = call("min", vec![Value::Integer(3), Value::Float(2.5)]).unwrap();
    assert_eq!(result, Value::Float(2.5));
}

#[test]
fn test_builtin_min_negative() {
    let result = call("min", vec![Value::Integer(-5), Value::Integer(-10)]).unwrap();
    assert_eq!(result, Value::Integer(-10));
}

#[test]
fn test_builtin_max_integers() {
    let result = call("max", vec![Value::Integer(3), Value::Integer(7)]).unwrap();
    assert_eq!(result, Value::Integer(7));
}

#[test]
fn test_builtin_max_integers_reversed() {
    let result = call("max", vec![Value::Integer(10), Value::Integer(2)]).unwrap();
    assert_eq!(result, Value::Integer(10));
}

#[test]
fn test_builtin_max_floats() {
    let result = call("max", vec![Value::Float(3.5), Value::Float(2.1)]).unwrap();
    assert_eq!(result, Value::Float(3.5));
}

#[test]
fn test_builtin_max_mixed() {
    let result = call("max", vec![Value::Integer(3), Value::Float(3.5)]).unwrap();
    assert_eq!(result, Value::Float(3.5));
}

#[test]
fn test_builtin_max_negative() {
    let result = call("max", vec![Value::Integer(-5), Value::Integer(-10)]).unwrap();
    assert_eq!(result, Value::Integer(-5));
}

#[test]
fn test_builtin_min_type_error() {
    let result = call(
        "min",
        vec![Value::String("a".to_string().into()), Value::Integer(1)],
    );
    assert!(result.is_err());
}

#[test]
fn test_builtin_max_type_error() {
    let result = call(
        "max",
        vec![Value::Integer(1), Value::String("a".to_string().into())],
    );
    assert!(result.is_err());
}

#[test]
fn test_builtin_type_of_int() {
    let result = call("type_of", vec![Value::Integer(42)]).unwrap();
    assert_eq!(result, Value::String("Int".to_string().into()));
}

#[test]
fn test_builtin_type_of_float() {
    let result = call("type_of", vec![Value::Float(PI)]).unwrap();
    assert_eq!(result, Value::String("Float".to_string().into()));
}

#[test]
fn test_builtin_type_of_string() {
    let result = call("type_of", vec![Value::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Value::String("String".to_string().into()));
}

#[test]
fn test_builtin_type_of_bool() {
    let result = call("type_of", vec![Value::Boolean(true)]).unwrap();
    assert_eq!(result, Value::String("Bool".to_string().into()));
}

#[test]
fn test_builtin_type_of_array() {
    let result = call(
        "type_of",
        vec![Value::Array(vec![Value::Integer(1)].into())],
    )
    .unwrap();
    assert_eq!(result, Value::String("Array".to_string().into()));
}

#[test]
fn test_builtin_type_of_hash() {
    let mut vm = test_vm();
    let root = hamt_empty(vm.gc_heap_mut());
    let result = call_vm(&mut vm, "type_of", vec![Value::Gc(root)]).unwrap();
    assert_eq!(result, Value::String("Map".to_string().into()));
}

#[test]
fn test_builtin_type_of_none() {
    let result = call("type_of", vec![Value::None]).unwrap();
    assert_eq!(result, Value::String("None".to_string().into()));
}

#[test]
fn test_builtin_type_of_some() {
    let result = call(
        "type_of",
        vec![Value::Some(std::rc::Rc::new(Value::Integer(42)))],
    )
    .unwrap();
    assert_eq!(result, Value::String("Some".to_string().into()));
}

#[test]
fn test_builtin_is_int_true() {
    let result = call("is_int", vec![Value::Integer(42)]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_is_int_false() {
    let result = call("is_int", vec![Value::Float(PI)]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn test_builtin_is_float_true() {
    let result = call("is_float", vec![Value::Float(PI)]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_is_float_false() {
    let result = call("is_float", vec![Value::Integer(42)]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn test_builtin_is_string_true() {
    let result = call("is_string", vec![Value::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_is_string_false() {
    let result = call("is_string", vec![Value::Integer(42)]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn test_builtin_is_bool_true() {
    let result = call("is_bool", vec![Value::Boolean(true)]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_is_bool_false() {
    let result = call("is_bool", vec![Value::Integer(0)]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn test_builtin_is_array_true() {
    let result = call("is_array", vec![Value::Array(vec![].into())]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_is_array_false() {
    let result = call("is_array", vec![Value::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn test_builtin_is_hash_true() {
    let mut vm = test_vm();
    let root = hamt_empty(vm.gc_heap_mut());
    let result = call_vm(&mut vm, "is_hash", vec![Value::Gc(root)]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_is_hash_false() {
    let result = call("is_hash", vec![Value::Array(vec![].into())]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn test_builtin_is_none_true() {
    let result = call("is_none", vec![Value::None]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_is_none_false() {
    let result = call("is_none", vec![Value::Integer(42)]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn test_builtin_is_some_true() {
    let result = call(
        "is_some",
        vec![Value::Some(std::rc::Rc::new(Value::Integer(42)))],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_is_some_false() {
    let result = call("is_some", vec![Value::None]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

// ── List builtin tests ──────────────────────────────────────────────────

fn make_test_list(vm: &mut VM, elems: &[Value]) -> Value {
    let mut list = Value::None;
    for elem in elems.iter().rev() {
        let handle = vm.gc_heap_mut().alloc(flux::runtime::gc::HeapObject::Cons {
            head: elem.clone(),
            tail: list,
        });
        list = Value::Gc(handle);
    }
    list
}

#[test]
fn test_builtin_list_constructor() {
    let mut vm = test_vm();
    let result = call_vm(
        &mut vm,
        "list",
        vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)],
    )
    .unwrap();
    // Verify via to_array round-trip
    let arr = call_vm(&mut vm, "to_array", vec![result]).unwrap();
    assert_eq!(
        arr,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );
}

#[test]
fn test_builtin_list_empty() {
    let mut vm = test_vm();
    let result = call_vm(&mut vm, "list", vec![]).unwrap();
    assert_eq!(result, Value::EmptyList);
}

#[test]
fn test_builtin_len_list() {
    let mut vm = test_vm();
    let list = make_test_list(
        &mut vm,
        &[Value::Integer(10), Value::Integer(20), Value::Integer(30)],
    );
    let result = call_vm(&mut vm, "len", vec![list]).unwrap();
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn test_builtin_len_empty_list() {
    let result = call("len", vec![Value::None]).unwrap();
    assert_eq!(result, Value::Integer(0));
}

#[test]
fn test_builtin_len_map() {
    let mut vm = test_vm();
    let map = make_test_hash(vm.gc_heap_mut());
    let result = call_vm(&mut vm, "len", vec![map]).unwrap();
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn test_builtin_first_list() {
    let mut vm = test_vm();
    let list = make_test_list(&mut vm, &[Value::Integer(10), Value::Integer(20)]);
    let result = call_vm(&mut vm, "first", vec![list]).unwrap();
    assert_eq!(result, Value::Integer(10));
}

#[test]
fn test_builtin_first_empty_list() {
    let result = call("first", vec![Value::None]).unwrap();
    assert_eq!(result, Value::None);
}

#[test]
fn test_builtin_last_list() {
    let mut vm = test_vm();
    let list = make_test_list(
        &mut vm,
        &[Value::Integer(10), Value::Integer(20), Value::Integer(30)],
    );
    let result = call_vm(&mut vm, "last", vec![list]).unwrap();
    assert_eq!(result, Value::Integer(30));
}

#[test]
fn test_builtin_last_empty_list() {
    let result = call("last", vec![Value::None]).unwrap();
    assert_eq!(result, Value::None);
}

#[test]
fn test_builtin_rest_list() {
    let mut vm = test_vm();
    let list = make_test_list(
        &mut vm,
        &[Value::Integer(1), Value::Integer(2), Value::Integer(3)],
    );
    let rest = call_vm(&mut vm, "rest", vec![list]).unwrap();
    let arr = call_vm(&mut vm, "to_array", vec![rest]).unwrap();
    assert_eq!(
        arr,
        Value::Array(vec![Value::Integer(2), Value::Integer(3)].into())
    );
}

#[test]
fn test_builtin_rest_empty_list() {
    let result = call("rest", vec![Value::None]).unwrap();
    assert_eq!(result, Value::None);
}

#[test]
fn test_builtin_reverse_list() {
    let mut vm = test_vm();
    let list = make_test_list(
        &mut vm,
        &[Value::Integer(1), Value::Integer(2), Value::Integer(3)],
    );
    let reversed = call_vm(&mut vm, "reverse", vec![list]).unwrap();
    let arr = call_vm(&mut vm, "to_array", vec![reversed]).unwrap();
    assert_eq!(
        arr,
        Value::Array(vec![Value::Integer(3), Value::Integer(2), Value::Integer(1)].into())
    );
}

#[test]
fn test_builtin_reverse_empty_list() {
    let result = call("reverse", vec![Value::None]).unwrap();
    assert_eq!(result, Value::None);
}

#[test]
fn test_builtin_contains_list() {
    let mut vm = test_vm();
    let list = make_test_list(
        &mut vm,
        &[Value::Integer(1), Value::Integer(2), Value::Integer(3)],
    );
    let found = call_vm(&mut vm, "contains", vec![list.clone(), Value::Integer(2)]).unwrap();
    assert_eq!(found, Value::Boolean(true));
    let not_found = call_vm(&mut vm, "contains", vec![list, Value::Integer(99)]).unwrap();
    assert_eq!(not_found, Value::Boolean(false));
}

#[test]
fn test_builtin_contains_empty_list() {
    let result = call("contains", vec![Value::None, Value::Integer(1)]).unwrap();
    assert_eq!(result, Value::Boolean(false));
}

// ── HAMT builtin tests ──────────────────────────────────────────────────

#[test]
fn test_builtin_put() {
    let mut vm = test_vm();
    let map = make_test_hash(vm.gc_heap_mut());
    let updated = call_vm(
        &mut vm,
        "put",
        vec![
            map.clone(),
            Value::String("email".into()),
            Value::String("a@b.com".into()),
        ],
    )
    .unwrap();
    // Original unchanged
    let orig_len = call_vm(&mut vm, "len", vec![map.clone()]).unwrap();
    assert_eq!(orig_len, Value::Integer(3));
    // New map has 4 entries
    let new_len = call_vm(&mut vm, "len", vec![updated]).unwrap();
    assert_eq!(new_len, Value::Integer(4));
}

#[test]
fn test_builtin_get() {
    let mut vm = test_vm();
    let map = make_test_hash(vm.gc_heap_mut());
    let found = call_vm(
        &mut vm,
        "get",
        vec![map.clone(), Value::String("name".into())],
    )
    .unwrap();
    assert_eq!(
        found,
        Value::Some(std::rc::Rc::new(Value::String("Alice".into())))
    );
    let not_found = call_vm(&mut vm, "get", vec![map, Value::String("missing".into())]).unwrap();
    assert_eq!(not_found, Value::None);
}

#[test]
fn test_builtin_is_map() {
    let mut vm = test_vm();
    let map = make_test_hash(vm.gc_heap_mut());
    let result = call_vm(&mut vm, "is_map", vec![map]).unwrap();
    assert_eq!(result, Value::Boolean(true));
    let result2 = call_vm(&mut vm, "is_map", vec![Value::Integer(1)]).unwrap();
    assert_eq!(result2, Value::Boolean(false));
}

#[test]
fn test_hamt_structural_sharing() {
    let mut vm = test_vm();
    let heap = vm.gc_heap_mut();
    let root1 = hamt_empty(heap);
    let root2 = hamt_insert(heap, root1, HashKey::String("a".into()), Value::Integer(1));
    let root3 = hamt_insert(heap, root2, HashKey::String("b".into()), Value::Integer(2));
    // root2 still has only "a"
    assert_eq!(
        hamt_lookup(vm.gc_heap(), root2, &HashKey::String("a".into())),
        Some(Value::Integer(1))
    );
    assert_eq!(
        hamt_lookup(vm.gc_heap(), root2, &HashKey::String("b".into())),
        None
    );
    // root3 has both
    assert_eq!(
        hamt_lookup(vm.gc_heap(), root3, &HashKey::String("a".into())),
        Some(Value::Integer(1))
    );
    assert_eq!(
        hamt_lookup(vm.gc_heap(), root3, &HashKey::String("b".into())),
        Some(Value::Integer(2))
    );
}

#[test]
fn test_hamt_10k_sequential_inserts() {
    let mut vm = test_vm();
    let heap = vm.gc_heap_mut();
    let mut root = hamt_empty(heap);
    for i in 0..10_000i64 {
        root = hamt_insert(heap, root, HashKey::Integer(i), Value::Integer(i * 2));
    }
    assert_eq!(hamt_len(vm.gc_heap(), root), 10_000);
    // Spot check a few entries
    assert_eq!(
        hamt_lookup(vm.gc_heap(), root, &HashKey::Integer(0)),
        Some(Value::Integer(0))
    );
    assert_eq!(
        hamt_lookup(vm.gc_heap(), root, &HashKey::Integer(5000)),
        Some(Value::Integer(10_000))
    );
    assert_eq!(
        hamt_lookup(vm.gc_heap(), root, &HashKey::Integer(9999)),
        Some(Value::Integer(19_998))
    );
}

#[test]
fn test_builtin_read_file() {
    let path = temp_file_path("read_file");
    fs::write(&path, "line1\nline2\n").expect("write temp file");

    let result = call(
        "read_file",
        vec![Value::String(path.to_string_lossy().to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Value::String("line1\nline2\n".into()));

    fs::remove_file(path).ok();
}

#[test]
fn test_builtin_read_lines() {
    let path = temp_file_path("read_lines");
    fs::write(&path, "10\n20\n30\n").expect("write temp file");

    let result = call(
        "read_lines",
        vec![Value::String(path.to_string_lossy().to_string().into())],
    )
    .unwrap();
    assert_eq!(
        result,
        Value::Array(
            vec![
                Value::String("10".into()),
                Value::String("20".into()),
                Value::String("30".into())
            ]
            .into()
        )
    );

    fs::remove_file(path).ok();
}

#[test]
fn test_builtin_parse_int() {
    let result = call("parse_int", vec![Value::String("  12345  ".into())]).unwrap();
    assert_eq!(result, Value::Integer(12345));
}

#[test]
fn test_builtin_parse_int_invalid() {
    let err = call("parse_int", vec![Value::String("12x".into())]).unwrap_err();
    assert!(err.contains("parse_int"));
    assert!(err.contains("could not parse"));
}

#[test]
fn test_builtin_now_ms() {
    let result = call("now_ms", vec![]).unwrap();
    match result {
        Value::Integer(ms) => assert!(ms > 0),
        _ => panic!("expected Integer"),
    }
}

#[test]
fn test_builtin_time() {
    let print_builtin_idx = get_builtin_index("print").expect("print builtin exists");
    let result = call("time", vec![Value::Builtin(print_builtin_idx as u8)]).unwrap();
    match result {
        Value::Integer(ms) => assert!(ms >= 0),
        _ => panic!("expected Integer"),
    }
}

#[test]
fn test_builtin_range() {
    let asc = call("range", vec![Value::Integer(2), Value::Integer(6)]).unwrap();
    assert_eq!(
        asc,
        Value::Array(
            vec![
                Value::Integer(2),
                Value::Integer(3),
                Value::Integer(4),
                Value::Integer(5)
            ]
            .into()
        )
    );

    let desc = call("range", vec![Value::Integer(3), Value::Integer(0)]).unwrap();
    assert_eq!(
        desc,
        Value::Array(vec![Value::Integer(3), Value::Integer(2), Value::Integer(1)].into())
    );
}

#[test]
fn test_builtin_sum_and_product() {
    let sum_i = call(
        "sum",
        vec![Value::Array(
            vec![Value::Integer(2), Value::Integer(3), Value::Integer(5)].into(),
        )],
    )
    .unwrap();
    assert_eq!(sum_i, Value::Integer(10));

    let product_i = call(
        "product",
        vec![Value::Array(
            vec![Value::Integer(2), Value::Integer(3), Value::Integer(5)].into(),
        )],
    )
    .unwrap();
    assert_eq!(product_i, Value::Integer(30));
}

#[test]
fn test_builtin_parse_ints_and_split_ints() {
    let parsed = call(
        "parse_ints",
        vec![Value::Array(
            vec![
                Value::String("10".into()),
                Value::String(" 20 ".into()),
                Value::String("-3".into()),
            ]
            .into(),
        )],
    )
    .unwrap();
    assert_eq!(
        parsed,
        Value::Array(vec![Value::Integer(10), Value::Integer(20), Value::Integer(-3)].into())
    );

    let split = call(
        "split_ints",
        vec![Value::String("1,2,-5".into()), Value::String(",".into())],
    )
    .unwrap();
    assert_eq!(
        split,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(-5)].into())
    );
}
