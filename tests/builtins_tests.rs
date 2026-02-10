use std::collections::HashMap;

const PI: f64 = std::f64::consts::PI;

use flux::runtime::builtins::get_builtin;
use flux::runtime::hash_key::HashKey;
use flux::runtime::object::Object;

fn call(name: &str, args: Vec<Object>) -> Result<Object, String> {
    let builtin = get_builtin(name).unwrap_or_else(|| panic!("missing builtin: {}", name));
    (builtin.func)(&args)
}

fn make_test_hash() -> Object {
    let mut hash = HashMap::new();
    hash.insert(
        HashKey::String("name".to_string()),
        Object::String("Alice".to_string().into()),
    );
    hash.insert(HashKey::Integer(42), Object::Integer(100));
    hash.insert(
        HashKey::Boolean(true),
        Object::String("yes".to_string().into()),
    );
    Object::Hash(hash.into())
}

#[test]
fn test_builtin_len_string() {
    let result = call("len", vec![Object::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Object::Integer(5));
}

#[test]
fn test_builtin_len_array() {
    let result = call(
        "len",
        vec![Object::Array(
            vec![Object::Integer(1), Object::Integer(2), Object::Integer(3)].into(),
        )],
    )
    .unwrap();
    assert_eq!(result, Object::Integer(3));
}

#[test]
fn test_builtin_first() {
    let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)].into());
    let result = call("first", vec![arr].into()).unwrap();
    assert_eq!(result, Object::Integer(1));
}

#[test]
fn test_builtin_last() {
    let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)].into());
    let result = call("last", vec![arr].into()).unwrap();
    assert_eq!(result, Object::Integer(2));
}

#[test]
fn test_builtin_rest() {
    let arr =
        Object::Array(vec![Object::Integer(1), Object::Integer(2), Object::Integer(3)].into());
    let result = call("rest", vec![arr].into()).unwrap();
    assert_eq!(
        result,
        Object::Array(vec![Object::Integer(2), Object::Integer(3)].into())
    );
}

#[test]
fn test_builtin_push() {
    let arr = Object::Array(vec![Object::Integer(1)].into());
    let result = call("push", vec![arr, Object::Integer(2)].into()).unwrap();
    assert_eq!(
        result,
        Object::Array(vec![Object::Integer(1), Object::Integer(2)].into())
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
    let a = Object::Array(vec![Object::Integer(1), Object::Integer(2)].into());
    let b = Object::Array(vec![Object::Integer(3), Object::Integer(4)].into());
    let result = call("concat", vec![a, b].into()).unwrap();
    assert_eq!(
        result,
        Object::Array(
            vec![
                Object::Integer(1),
                Object::Integer(2),
                Object::Integer(3),
                Object::Integer(4)
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_concat_empty() {
    let a = Object::Array(vec![Object::Integer(1)].into());
    let b = Object::Array(vec![].into());
    let result = call("concat", vec![a, b].into()).unwrap();
    assert_eq!(result, Object::Array(vec![Object::Integer(1)].into()));
}

#[test]
fn test_builtin_reverse() {
    let arr =
        Object::Array(vec![Object::Integer(1), Object::Integer(2), Object::Integer(3)].into());
    let result = call("reverse", vec![arr].into()).unwrap();
    assert_eq!(
        result,
        Object::Array(vec![Object::Integer(3), Object::Integer(2), Object::Integer(1)].into())
    );
}

#[test]
fn test_builtin_reverse_empty() {
    let arr = Object::Array(vec![].into());
    let result = call("reverse", vec![arr].into()).unwrap();
    assert_eq!(result, Object::Array(vec![].into()));
}

#[test]
fn test_builtin_contains_found() {
    let arr =
        Object::Array(vec![Object::Integer(1), Object::Integer(2), Object::Integer(3)].into());
    let result = call("contains", vec![arr, Object::Integer(2)].into()).unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_contains_not_found() {
    let arr =
        Object::Array(vec![Object::Integer(1), Object::Integer(2), Object::Integer(3)].into());
    let result = call("contains", vec![arr, Object::Integer(5)].into()).unwrap();
    assert_eq!(result, Object::Boolean(false));
}

#[test]
fn test_builtin_slice() {
    let arr = Object::Array(
        vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
            Object::Integer(4),
            Object::Integer(5),
        ]
        .into(),
    );
    let result = call(
        "slice",
        vec![arr, Object::Integer(1), Object::Integer(4)].into(),
    )
    .unwrap();
    assert_eq!(
        result,
        Object::Array(vec![Object::Integer(2), Object::Integer(3), Object::Integer(4)].into())
    );
}

#[test]
fn test_builtin_slice_out_of_bounds() {
    let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)].into());
    let result = call(
        "slice",
        vec![arr, Object::Integer(0), Object::Integer(10)].into(),
    )
    .unwrap();
    assert_eq!(
        result,
        Object::Array(vec![Object::Integer(1), Object::Integer(2)].into())
    );
}

#[test]
fn test_builtin_sort() {
    let arr = Object::Array(
        vec![
            Object::Integer(3),
            Object::Integer(1),
            Object::Integer(4),
            Object::Integer(1),
            Object::Integer(5),
        ]
        .into(),
    );
    let result = call("sort", vec![arr].into()).unwrap();
    assert_eq!(
        result,
        Object::Array(
            vec![
                Object::Integer(1),
                Object::Integer(1),
                Object::Integer(3),
                Object::Integer(4),
                Object::Integer(5)
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_sort_floats() {
    let arr =
        Object::Array(vec![Object::Float(PI), Object::Float(1.0), Object::Float(2.71)].into());
    let result = call("sort", vec![arr].into()).unwrap();
    assert_eq!(
        result,
        Object::Array(vec![Object::Float(1.0), Object::Float(2.71), Object::Float(PI)].into())
    );
}

#[test]
fn test_builtin_sort_mixed_numeric() {
    let arr =
        Object::Array(vec![Object::Integer(3), Object::Float(1.5), Object::Integer(2)].into());
    let result = call("sort", vec![arr].into()).unwrap();
    assert_eq!(
        result,
        Object::Array(vec![Object::Float(1.5), Object::Integer(2), Object::Integer(3)].into())
    );
}

#[test]
fn test_builtin_sort_asc_explicit() {
    let arr =
        Object::Array(vec![Object::Integer(3), Object::Integer(1), Object::Integer(2)].into());
    let result = call(
        "sort",
        vec![arr, Object::String("asc".to_string().into())].into(),
    )
    .unwrap();
    assert_eq!(
        result,
        Object::Array(vec![Object::Integer(1), Object::Integer(2), Object::Integer(3)].into())
    );
}

#[test]
fn test_builtin_sort_desc() {
    let arr = Object::Array(
        vec![
            Object::Integer(3),
            Object::Integer(1),
            Object::Integer(5),
            Object::Integer(2),
        ]
        .into(),
    );
    let result = call(
        "sort",
        vec![arr, Object::String("desc".to_string().into())].into(),
    )
    .unwrap();
    assert_eq!(
        result,
        Object::Array(
            vec![
                Object::Integer(5),
                Object::Integer(3),
                Object::Integer(2),
                Object::Integer(1)
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_sort_desc_floats() {
    let arr =
        Object::Array(vec![Object::Float(1.0), Object::Float(PI), Object::Float(2.71)].into());
    let result = call(
        "sort",
        vec![arr, Object::String("desc".to_string().into())].into(),
    )
    .unwrap();
    assert_eq!(
        result,
        Object::Array(vec![Object::Float(PI), Object::Float(2.71), Object::Float(1.0)].into())
    );
}

#[test]
fn test_builtin_sort_invalid_order() {
    let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)].into());
    let result = call(
        "sort",
        vec![arr, Object::String("invalid".to_string().into())].into(),
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be \"asc\" or \"desc\""));
}

#[test]
fn test_builtin_split() {
    let result = call(
        "split",
        vec![
            Object::String("a,b,c".to_string().into()),
            Object::String(",".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(
        result,
        Object::Array(
            vec![
                Object::String("a".to_string().into()),
                Object::String("b".to_string().into()),
                Object::String("c".to_string().into())
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
            Object::String("hello".to_string().into()),
            Object::String("".to_string().into()),
        ],
    )
    .unwrap();
    // Split by empty string gives each character
    assert_eq!(
        result,
        Object::Array(
            vec![
                Object::String("h".to_string().into()),
                Object::String("e".to_string().into()),
                Object::String("l".to_string().into()),
                Object::String("l".to_string().into()),
                Object::String("o".to_string().into())
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_join() {
    let arr = Object::Array(
        vec![
            Object::String("a".to_string().into()),
            Object::String("b".to_string().into()),
            Object::String("c".to_string().into()),
        ]
        .into(),
    );
    let result = call(
        "join",
        vec![arr, Object::String(",".to_string().into())].into(),
    )
    .unwrap();
    assert_eq!(result, Object::String("a,b,c".to_string().into()));
}

#[test]
fn test_builtin_join_empty_delim() {
    let arr = Object::Array(
        vec![
            Object::String("a".to_string().into()),
            Object::String("b".to_string().into()),
        ]
        .into(),
    );
    let result = call(
        "join",
        vec![arr, Object::String("".to_string().into())].into(),
    )
    .unwrap();
    assert_eq!(result, Object::String("ab".to_string().into()));
}

#[test]
fn test_builtin_trim() {
    let result = call(
        "trim",
        vec![Object::String("  hello world  ".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Object::String("hello world".to_string().into()));
}

#[test]
fn test_builtin_trim_no_whitespace() {
    let result = call("trim", vec![Object::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Object::String("hello".to_string().into()));
}

#[test]
fn test_builtin_upper() {
    let result = call("upper", vec![Object::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Object::String("HELLO".to_string().into()));
}

#[test]
fn test_builtin_lower() {
    let result = call("lower", vec![Object::String("HELLO".to_string().into())]).unwrap();
    assert_eq!(result, Object::String("hello".to_string().into()));
}

#[test]
fn test_builtin_chars() {
    let result = call("chars", vec![Object::String("abc".to_string().into())]).unwrap();
    assert_eq!(
        result,
        Object::Array(
            vec![
                Object::String("a".to_string().into()),
                Object::String("b".to_string().into()),
                Object::String("c".to_string().into())
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_chars_empty() {
    let result = call("chars", vec![Object::String("".to_string().into())].into()).unwrap();
    assert_eq!(result, Object::Array(vec![].into()));
}

#[test]
fn test_builtin_substring() {
    let result = call(
        "substring",
        vec![
            Object::String("hello world".to_string().into()),
            Object::Integer(0),
            Object::Integer(5),
        ],
    )
    .unwrap();
    assert_eq!(result, Object::String("hello".to_string().into()));
}

#[test]
fn test_builtin_substring_middle() {
    let result = call(
        "substring",
        vec![
            Object::String("hello world".to_string().into()),
            Object::Integer(6),
            Object::Integer(11),
        ],
    )
    .unwrap();
    assert_eq!(result, Object::String("world".to_string().into()));
}

#[test]
fn test_builtin_substring_out_of_bounds() {
    let result = call(
        "substring",
        vec![
            Object::String("hello".to_string().into()),
            Object::Integer(0),
            Object::Integer(100),
        ],
    )
    .unwrap();
    assert_eq!(result, Object::String("hello".to_string().into()));
}

#[test]
fn test_builtin_starts_with() {
    let result = call(
        "starts_with",
        vec![
            Object::String("hello".to_string().into()),
            Object::String("he".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_ends_with() {
    let result = call(
        "ends_with",
        vec![
            Object::String("hello".to_string().into()),
            Object::String("lo".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_replace() {
    let result = call(
        "replace",
        vec![
            Object::String("banana".to_string().into()),
            Object::String("na".to_string().into()),
            Object::String("X".to_string().into()),
        ],
    )
    .unwrap();
    assert_eq!(result, Object::String("baXX".to_string().into()));
}

#[test]
fn test_builtin_keys() {
    let hash = make_test_hash();
    let result = call("keys", vec![hash].into()).unwrap();
    match result {
        Object::Array(keys) => {
            assert_eq!(keys.len(), 3);
            // Check that all expected keys are present (order is not guaranteed)
            let has_name = keys.contains(&Object::String("name".to_string().into()));
            let has_42 = keys.contains(&Object::Integer(42));
            let has_true = keys.contains(&Object::Boolean(true));
            assert!(has_name, "missing 'name' key");
            assert!(has_42, "missing 42 key");
            assert!(has_true, "missing true key");
        }
        _ => panic!("expected Array"),
    }
}

#[test]
fn test_builtin_keys_empty() {
    let hash = Object::Hash(HashMap::new().into());
    let result = call("keys", vec![hash]).unwrap();
    assert_eq!(result, Object::Array(vec![].into()));
}

#[test]
fn test_builtin_values() {
    let hash = make_test_hash();
    let result = call("values", vec![hash].into()).unwrap();
    match result {
        Object::Array(values) => {
            assert_eq!(values.len(), 3);
            // Check that all expected values are present (order is not guaranteed)
            let has_alice = values.contains(&Object::String("Alice".to_string().into()));
            let has_100 = values.contains(&Object::Integer(100));
            let has_yes = values.contains(&Object::String("yes".to_string().into()));
            assert!(has_alice, "missing 'Alice' value");
            assert!(has_100, "missing 100 value");
            assert!(has_yes, "missing 'yes' value");
        }
        _ => panic!("expected Array"),
    }
}

#[test]
fn test_builtin_values_empty() {
    let hash = Object::Hash(HashMap::new().into());
    let result = call("values", vec![hash]).unwrap();
    assert_eq!(result, Object::Array(vec![].into()));
}

#[test]
fn test_builtin_has_key_found() {
    let hash = make_test_hash();
    let result = call(
        "has_key",
        vec![hash, Object::String("name".to_string().into())].into(),
    )
    .unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_has_key_not_found() {
    let hash = make_test_hash();
    let result = call(
        "has_key",
        vec![hash, Object::String("email".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Object::Boolean(false));
}

#[test]
fn test_builtin_has_key_integer_key() {
    let hash = make_test_hash();
    let result = call("has_key", vec![hash, Object::Integer(42)]).unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_has_key_boolean_key() {
    let hash = make_test_hash();
    let result = call("has_key", vec![hash, Object::Boolean(true)]).unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_has_key_unhashable() {
    let hash = make_test_hash();
    let result = call("has_key", vec![hash, Object::Array(vec![].into())].into());
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be hashable"));
}

#[test]
fn test_builtin_merge() {
    let mut h1 = HashMap::new();
    h1.insert(HashKey::String("a".to_string()), Object::Integer(1));
    h1.insert(HashKey::String("b".to_string()), Object::Integer(2));

    let mut h2 = HashMap::new();
    h2.insert(HashKey::String("b".to_string()), Object::Integer(20)); // overwrites
    h2.insert(HashKey::String("c".to_string()), Object::Integer(3));

    let result = call(
        "merge",
        vec![Object::Hash(h1.into()), Object::Hash(h2.into())],
    )
    .unwrap();
    match result {
        Object::Hash(merged) => {
            assert_eq!(merged.len(), 3);
            assert_eq!(
                merged.get(&HashKey::String("a".to_string())),
                Some(&Object::Integer(1))
            );
            assert_eq!(
                merged.get(&HashKey::String("b".to_string())),
                Some(&Object::Integer(20))
            ); // overwritten
            assert_eq!(
                merged.get(&HashKey::String("c".to_string())),
                Some(&Object::Integer(3))
            );
        }
        _ => panic!("expected Hash"),
    }
}

#[test]
fn test_builtin_delete() {
    let mut h = HashMap::new();
    h.insert(HashKey::String("a".to_string()), Object::Integer(1));
    h.insert(HashKey::String("b".to_string()), Object::Integer(2));

    let result = call(
        "delete",
        vec![
            Object::Hash(h.into()),
            Object::String("a".to_string().into()),
        ],
    )
    .unwrap();

    match result {
        Object::Hash(map) => {
            assert_eq!(map.get(&HashKey::String("a".to_string())), None);
            assert_eq!(
                map.get(&HashKey::String("b".to_string())),
                Some(&Object::Integer(2))
            );
        }
        _ => panic!("expected Hash"),
    }
}

#[test]
fn test_builtin_merge_empty() {
    let mut h1 = HashMap::new();
    h1.insert(HashKey::String("a".to_string()), Object::Integer(1));

    let h2 = HashMap::new();

    let result = call(
        "merge",
        vec![Object::Hash(h1.clone().into()), Object::Hash(h2.into())],
    )
    .unwrap();
    match result {
        Object::Hash(merged) => {
            assert_eq!(merged.len(), 1);
            assert_eq!(
                merged.get(&HashKey::String("a".to_string())),
                Some(&Object::Integer(1))
            );
        }
        _ => panic!("expected Hash"),
    }
}

#[test]
fn test_builtin_merge_into_empty() {
    let h1 = HashMap::new();

    let mut h2 = HashMap::new();
    h2.insert(HashKey::String("a".to_string()), Object::Integer(1));

    let result = call(
        "merge",
        vec![Object::Hash(h1.into()), Object::Hash(h2.into())],
    )
    .unwrap();
    match result {
        Object::Hash(merged) => {
            assert_eq!(merged.len(), 1);
            assert_eq!(
                merged.get(&HashKey::String("a".to_string())),
                Some(&Object::Integer(1))
            );
        }
        _ => panic!("expected Hash"),
    }
}

#[test]
fn test_builtin_abs_integer_positive() {
    let result = call("abs", vec![Object::Integer(5)]).unwrap();
    assert_eq!(result, Object::Integer(5));
}

#[test]
fn test_builtin_abs_integer_negative() {
    let result = call("abs", vec![Object::Integer(-5)]).unwrap();
    assert_eq!(result, Object::Integer(5));
}

#[test]
fn test_builtin_abs_integer_zero() {
    let result = call("abs", vec![Object::Integer(0)]).unwrap();
    assert_eq!(result, Object::Integer(0));
}

#[test]
fn test_builtin_abs_float_positive() {
    let result = call("abs", vec![Object::Float(PI)]).unwrap();
    assert_eq!(result, Object::Float(PI));
}

#[test]
fn test_builtin_abs_float_negative() {
    let result = call("abs", vec![Object::Float(-PI)]).unwrap();
    assert_eq!(result, Object::Float(PI));
}

#[test]
fn test_builtin_abs_type_error() {
    let result = call("abs", vec![Object::String("hello".to_string().into())]);
    assert!(result.is_err());
}

#[test]
fn test_builtin_min_integers() {
    let result = call("min", vec![Object::Integer(3), Object::Integer(7)]).unwrap();
    assert_eq!(result, Object::Integer(3));
}

#[test]
fn test_builtin_min_integers_reversed() {
    let result = call("min", vec![Object::Integer(10), Object::Integer(2)]).unwrap();
    assert_eq!(result, Object::Integer(2));
}

#[test]
fn test_builtin_min_floats() {
    let result = call("min", vec![Object::Float(3.5), Object::Float(2.1)]).unwrap();
    assert_eq!(result, Object::Float(2.1));
}

#[test]
fn test_builtin_min_mixed() {
    let result = call("min", vec![Object::Integer(3), Object::Float(2.5)]).unwrap();
    assert_eq!(result, Object::Float(2.5));
}

#[test]
fn test_builtin_min_negative() {
    let result = call("min", vec![Object::Integer(-5), Object::Integer(-10)]).unwrap();
    assert_eq!(result, Object::Integer(-10));
}

#[test]
fn test_builtin_max_integers() {
    let result = call("max", vec![Object::Integer(3), Object::Integer(7)]).unwrap();
    assert_eq!(result, Object::Integer(7));
}

#[test]
fn test_builtin_max_integers_reversed() {
    let result = call("max", vec![Object::Integer(10), Object::Integer(2)]).unwrap();
    assert_eq!(result, Object::Integer(10));
}

#[test]
fn test_builtin_max_floats() {
    let result = call("max", vec![Object::Float(3.5), Object::Float(2.1)]).unwrap();
    assert_eq!(result, Object::Float(3.5));
}

#[test]
fn test_builtin_max_mixed() {
    let result = call("max", vec![Object::Integer(3), Object::Float(3.5)]).unwrap();
    assert_eq!(result, Object::Float(3.5));
}

#[test]
fn test_builtin_max_negative() {
    let result = call("max", vec![Object::Integer(-5), Object::Integer(-10)]).unwrap();
    assert_eq!(result, Object::Integer(-5));
}

#[test]
fn test_builtin_min_type_error() {
    let result = call(
        "min",
        vec![Object::String("a".to_string().into()), Object::Integer(1)],
    );
    assert!(result.is_err());
}

#[test]
fn test_builtin_max_type_error() {
    let result = call(
        "max",
        vec![Object::Integer(1), Object::String("a".to_string().into())],
    );
    assert!(result.is_err());
}

#[test]
fn test_builtin_type_of_int() {
    let result = call("type_of", vec![Object::Integer(42)]).unwrap();
    assert_eq!(result, Object::String("Int".to_string().into()));
}

#[test]
fn test_builtin_type_of_float() {
    let result = call("type_of", vec![Object::Float(PI)]).unwrap();
    assert_eq!(result, Object::String("Float".to_string().into()));
}

#[test]
fn test_builtin_type_of_string() {
    let result = call("type_of", vec![Object::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Object::String("String".to_string().into()));
}

#[test]
fn test_builtin_type_of_bool() {
    let result = call("type_of", vec![Object::Boolean(true)]).unwrap();
    assert_eq!(result, Object::String("Bool".to_string().into()));
}

#[test]
fn test_builtin_type_of_array() {
    let result = call(
        "type_of",
        vec![Object::Array(vec![Object::Integer(1)].into())].into(),
    )
    .unwrap();
    assert_eq!(result, Object::String("Array".to_string().into()));
}

#[test]
fn test_builtin_type_of_hash() {
    let result = call("type_of", vec![Object::Hash(HashMap::new().into())]).unwrap();
    assert_eq!(result, Object::String("Hash".to_string().into()));
}

#[test]
fn test_builtin_type_of_none() {
    let result = call("type_of", vec![Object::None]).unwrap();
    assert_eq!(result, Object::String("None".to_string().into()));
}

#[test]
fn test_builtin_type_of_some() {
    let result = call(
        "type_of",
        vec![Object::Some(std::rc::Rc::new(Object::Integer(42)))],
    )
    .unwrap();
    assert_eq!(result, Object::String("Some".to_string().into()));
}

#[test]
fn test_builtin_is_int_true() {
    let result = call("is_int", vec![Object::Integer(42)]).unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_is_int_false() {
    let result = call("is_int", vec![Object::Float(PI)]).unwrap();
    assert_eq!(result, Object::Boolean(false));
}

#[test]
fn test_builtin_is_float_true() {
    let result = call("is_float", vec![Object::Float(PI)]).unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_is_float_false() {
    let result = call("is_float", vec![Object::Integer(42)]).unwrap();
    assert_eq!(result, Object::Boolean(false));
}

#[test]
fn test_builtin_is_string_true() {
    let result = call(
        "is_string",
        vec![Object::String("hello".to_string().into())],
    )
    .unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_is_string_false() {
    let result = call("is_string", vec![Object::Integer(42)]).unwrap();
    assert_eq!(result, Object::Boolean(false));
}

#[test]
fn test_builtin_is_bool_true() {
    let result = call("is_bool", vec![Object::Boolean(true)]).unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_is_bool_false() {
    let result = call("is_bool", vec![Object::Integer(0)]).unwrap();
    assert_eq!(result, Object::Boolean(false));
}

#[test]
fn test_builtin_is_array_true() {
    let result = call("is_array", vec![Object::Array(vec![].into())].into()).unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_is_array_false() {
    let result = call("is_array", vec![Object::String("hello".to_string().into())]).unwrap();
    assert_eq!(result, Object::Boolean(false));
}

#[test]
fn test_builtin_is_hash_true() {
    let result = call("is_hash", vec![Object::Hash(HashMap::new().into())]).unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_is_hash_false() {
    let result = call("is_hash", vec![Object::Array(vec![].into())].into()).unwrap();
    assert_eq!(result, Object::Boolean(false));
}

#[test]
fn test_builtin_is_none_true() {
    let result = call("is_none", vec![Object::None]).unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_is_none_false() {
    let result = call("is_none", vec![Object::Integer(42)]).unwrap();
    assert_eq!(result, Object::Boolean(false));
}

#[test]
fn test_builtin_is_some_true() {
    let result = call(
        "is_some",
        vec![Object::Some(std::rc::Rc::new(Object::Integer(42)))],
    )
    .unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn test_builtin_is_some_false() {
    let result = call("is_some", vec![Object::None]).unwrap();
    assert_eq!(result, Object::Boolean(false));
}
