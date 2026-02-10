use std::collections::HashMap;

const PI: f64 = std::f64::consts::PI;

use flux::runtime::builtins::get_builtin;
use flux::runtime::hash_key::HashKey;
use flux::runtime::value::Value;

fn call(name: &str, args: Vec<Value>) -> Result<Value, String> {
    let builtin = get_builtin(name).unwrap_or_else(|| panic!("missing builtin: {}", name));
    (builtin.func)(&args)
}

fn make_test_hash() -> Value {
    let mut hash = HashMap::new();
    hash.insert(
        HashKey::String("name".to_string()),
        Value::String("Alice".to_string().into()),
    );
    hash.insert(HashKey::Integer(42), Value::Integer(100));
    hash.insert(
        HashKey::Boolean(true),
        Value::String("yes".to_string().into()),
    );
    Value::Hash(hash.into())
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
    let hash = make_test_hash();
    let result = call("keys", vec![hash]).unwrap();
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
    let hash = Value::Hash(HashMap::new().into());
    let result = call("keys", vec![hash]).unwrap();
    assert_eq!(result, Value::Array(vec![].into()));
}

#[test]
fn test_builtin_values() {
    let hash = make_test_hash();
    let result = call("values", vec![hash]).unwrap();
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
    let hash = Value::Hash(HashMap::new().into());
    let result = call("values", vec![hash]).unwrap();
    assert_eq!(result, Value::Array(vec![].into()));
}

#[test]
fn test_builtin_has_key_found() {
    let hash = make_test_hash();
    let result = call(
        "has_key",
        vec![hash, Value::String("name".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_has_key_not_found() {
    let hash = make_test_hash();
    let result = call(
        "has_key",
        vec![hash, Value::String("email".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn test_builtin_has_key_integer_key() {
    let hash = make_test_hash();
    let result = call("has_key", vec![hash, Value::Integer(42)]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_has_key_boolean_key() {
    let hash = make_test_hash();
    let result = call("has_key", vec![hash, Value::Boolean(true)]).unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_builtin_has_key_unhashable() {
    let hash = make_test_hash();
    let result = call("has_key", vec![hash, Value::Array(vec![].into())]);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be hashable"));
}

#[test]
fn test_builtin_merge() {
    let mut h1 = HashMap::new();
    h1.insert(HashKey::String("a".to_string()), Value::Integer(1));
    h1.insert(HashKey::String("b".to_string()), Value::Integer(2));

    let mut h2 = HashMap::new();
    h2.insert(HashKey::String("b".to_string()), Value::Integer(20)); // overwrites
    h2.insert(HashKey::String("c".to_string()), Value::Integer(3));

    let result = call(
        "merge",
        vec![Value::Hash(h1.into()), Value::Hash(h2.into())],
    )
    .unwrap();
    match result {
        Value::Hash(merged) => {
            assert_eq!(merged.len(), 3);
            assert_eq!(
                merged.get(&HashKey::String("a".to_string())),
                Some(&Value::Integer(1))
            );
            assert_eq!(
                merged.get(&HashKey::String("b".to_string())),
                Some(&Value::Integer(20))
            ); // overwritten
            assert_eq!(
                merged.get(&HashKey::String("c".to_string())),
                Some(&Value::Integer(3))
            );
        }
        _ => panic!("expected Hash"),
    }
}

#[test]
fn test_builtin_delete() {
    let mut h = HashMap::new();
    h.insert(HashKey::String("a".to_string()), Value::Integer(1));
    h.insert(HashKey::String("b".to_string()), Value::Integer(2));

    let result = call(
        "delete",
        vec![Value::Hash(h.into()), Value::String("a".to_string().into())],
    )
    .unwrap();

    match result {
        Value::Hash(map) => {
            assert_eq!(map.get(&HashKey::String("a".to_string())), None);
            assert_eq!(
                map.get(&HashKey::String("b".to_string())),
                Some(&Value::Integer(2))
            );
        }
        _ => panic!("expected Hash"),
    }
}

#[test]
fn test_builtin_merge_empty() {
    let mut h1 = HashMap::new();
    h1.insert(HashKey::String("a".to_string()), Value::Integer(1));

    let h2 = HashMap::new();

    let result = call(
        "merge",
        vec![Value::Hash(h1.clone().into()), Value::Hash(h2.into())],
    )
    .unwrap();
    match result {
        Value::Hash(merged) => {
            assert_eq!(merged.len(), 1);
            assert_eq!(
                merged.get(&HashKey::String("a".to_string())),
                Some(&Value::Integer(1))
            );
        }
        _ => panic!("expected Hash"),
    }
}

#[test]
fn test_builtin_merge_into_empty() {
    let h1 = HashMap::new();

    let mut h2 = HashMap::new();
    h2.insert(HashKey::String("a".to_string()), Value::Integer(1));

    let result = call(
        "merge",
        vec![Value::Hash(h1.into()), Value::Hash(h2.into())],
    )
    .unwrap();
    match result {
        Value::Hash(merged) => {
            assert_eq!(merged.len(), 1);
            assert_eq!(
                merged.get(&HashKey::String("a".to_string())),
                Some(&Value::Integer(1))
            );
        }
        _ => panic!("expected Hash"),
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
    let result = call("type_of", vec![Value::Hash(HashMap::new().into())]).unwrap();
    assert_eq!(result, Value::String("Hash".to_string().into()));
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
    let result = call("is_hash", vec![Value::Hash(HashMap::new().into())]).unwrap();
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
