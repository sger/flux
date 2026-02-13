use std::any::type_name;

use crate::runtime::{
    RuntimeContext,
    builtins::helpers::type_error,
    gc::{
        gc_handle::GcHandle,
        hamt::{hamt_iter, is_hamt},
    },
    hash_key::HashKey,
    value::Value,
};

use super::helpers::{arg_hash, check_arity, format_hint};

fn hash_key_to_object(key: &HashKey) -> Value {
    match key {
        HashKey::Integer(v) => Value::Integer(*v),
        HashKey::Boolean(v) => Value::Boolean(*v),
        HashKey::String(v) => Value::String(v.clone().into()),
    }
}

/// Extracts a GcHandle from an argument, validating that it points to a HAMT node.
fn arg_hamt(
    ctx: &dyn RuntimeContext,
    args: &[Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<GcHandle, String> {
    match &args[index] {
        Value::Gc(h) if is_hamt(ctx.gc_heap(), *h) => Ok(*h),
        other => Err(type_error(
            name,
            label,
            "Hash",
            other.type_name(),
            signature,
        )),
    }
}

pub(super) fn builtin_keys(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "keys", "keys(h)")?;
    let handle = arg_hamt(ctx, &args, 0, "keys", "argument", "keys(h)")?;
    let pairs = hamt_iter(ctx.gc_heap(), handle);
    let keys: Vec<Value> = pairs.iter().map(|(k, _)| hash_key_to_object(k)).collect();
    Ok(Value::Array(keys.into()))
}

pub(super) fn builtin_values(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "values", "values(h)")?;
    let hash = arg_hash(&args, 0, "values", "argument", "values(h)")?;
    let values: Vec<Value> = hash.values().cloned().collect();
    Ok(Value::Array(values.into()))
}

pub(super) fn builtin_has_key(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "has_key", "has_key(h, k)")?;
    let hash = arg_hash(&args, 0, "has_key", "first argument", "has_key(h, k)")?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "has_key key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("has_key(h, k)")
        )
    })?;
    Ok(Value::Boolean(hash.contains_key(&key)))
}

pub(super) fn builtin_merge(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "merge", "merge(h1, h2)")?;
    let h1 = arg_hamt(ctx, &args, 0, "merge", "first argument", "merge(h1, h2)")?;
    let h2 = arg_hamt(ctx, &args, 1, "merge", "second argument", "merge(h1, h2)")?;

    let mut result = h1.clone();
    for (k, v) in h2.iter() {
        result.insert(k.clone(), v.clone());
    }
    Ok(Value::Hash(result.into()))
}

pub(super) fn builtin_delete(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "delete", "delete(h, k)")?;
    let hash = arg_hash(&args, 0, "delete", "first argument", "delete(h, k)")?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "delete key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("delete(h, k)")
        )
    })?;
    let mut result = hash.clone();
    result.remove(&key);
    Ok(Value::Hash(result.into()))
}
