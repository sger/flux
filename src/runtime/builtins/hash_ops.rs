use crate::runtime::{hash_key::HashKey, object::Object};

use super::helpers::{arg_hash, check_arity, format_hint};

fn hash_key_to_object(key: &HashKey) -> Object {
    match key {
        HashKey::Integer(v) => Object::Integer(*v),
        HashKey::Boolean(v) => Object::Boolean(*v),
        HashKey::String(v) => Object::String(v.clone()),
    }
}

pub(super) fn builtin_keys(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "keys", "keys(h)")?;
    let hash = arg_hash(&args, 0, "keys", "argument", "keys(h)")?;
    let keys: Vec<Object> = hash.keys().map(hash_key_to_object).collect();
    Ok(Object::Array(keys))
}

pub(super) fn builtin_values(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "values", "values(h)")?;
    let hash = arg_hash(&args, 0, "values", "argument", "values(h)")?;
    let values: Vec<Object> = hash.values().cloned().collect();
    Ok(Object::Array(values))
}

pub(super) fn builtin_has_key(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "has_key", "has_key(h, k)")?;
    let hash = arg_hash(&args, 0, "has_key", "first argument", "has_key(h, k)")?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "has_key key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("has_key(h, k)")
        )
    })?;
    Ok(Object::Boolean(hash.contains_key(&key)))
}

pub(super) fn builtin_merge(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "merge", "merge(h1, h2)")?;
    let h1 = arg_hash(&args, 0, "merge", "first argument", "merge(h1, h2)")?;
    let h2 = arg_hash(&args, 1, "merge", "second argument", "merge(h1, h2)")?;
    let mut result = h1.clone();
    for (k, v) in h2.iter() {
        result.insert(k.clone(), v.clone());
    }
    Ok(Object::Hash(result))
}
