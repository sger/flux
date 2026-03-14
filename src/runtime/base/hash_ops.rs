use crate::runtime::{
    RuntimeContext,
    gc::{
        gc_handle::GcHandle,
        hamt::{hamt_delete, hamt_insert, hamt_iter, hamt_lookup, is_hamt},
    },
    hash_key::HashKey,
    value::Value,
};

use super::helpers::{check_arity_ref, format_hint, type_error};

fn hash_key_to_object(key: &HashKey) -> Value {
    match key {
        HashKey::Integer(v) => Value::Integer(*v),
        HashKey::Boolean(v) => Value::Boolean(*v),
        HashKey::String(v) => Value::String(v.clone().into()),
    }
}

fn arg_hamt_ref(
    ctx: &dyn RuntimeContext,
    args: &[&Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<GcHandle, String> {
    match args[index] {
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

pub(super) fn base_keys(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_keys_borrowed(ctx, &borrowed)
}

pub(super) fn base_keys_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "keys", "keys(h)")?;
    let handle = arg_hamt_ref(ctx, args, 0, "keys", "argument", "keys(h)")?;
    let pairs = hamt_iter(ctx.gc_heap(), handle);
    let keys: Vec<Value> = pairs.iter().map(|(k, _)| hash_key_to_object(k)).collect();
    Ok(Value::Array(keys.into()))
}

pub(super) fn base_values(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_values_borrowed(ctx, &borrowed)
}

pub(super) fn base_values_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "values", "values(h)")?;
    let handle = arg_hamt_ref(ctx, args, 0, "values", "argument", "values(h)")?;
    let pairs = hamt_iter(ctx.gc_heap(), handle);
    let values: Vec<Value> = pairs.into_iter().map(|(_, v)| v).collect();
    Ok(Value::Array(values.into()))
}

pub(super) fn base_has_key(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_has_key_borrowed(ctx, &borrowed)
}

pub(super) fn base_has_key_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "has_key", "has_key(h, k)")?;
    let handle = arg_hamt_ref(ctx, args, 0, "has_key", "first argument", "has_key(h, k)")?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "has_key key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("has_key(h, k)")
        )
    })?;
    let found = hamt_lookup(ctx.gc_heap(), handle, &key).is_some();
    Ok(Value::Boolean(found))
}

pub(super) fn base_merge(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_merge_borrowed(ctx, &borrowed)
}

pub(super) fn base_merge_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "merge", "merge(h1, h2)")?;
    let h1 = arg_hamt_ref(ctx, args, 0, "merge", "first argument", "merge(h1, h2)")?;
    let h2 = arg_hamt_ref(ctx, args, 1, "merge", "second argument", "merge(h1, h2)")?;
    // Iterate h2's pairs and insert them into h1
    let pairs = hamt_iter(ctx.gc_heap(), h2);
    let mut result = h1;
    for (k, v) in pairs {
        result = hamt_insert(ctx.gc_heap_mut(), result, k, v);
    }
    Ok(Value::Gc(result))
}

pub(super) fn base_delete(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_delete_borrowed(ctx, &borrowed)
}

pub(super) fn base_delete_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "delete", "delete(h, k)")?;
    let handle = arg_hamt_ref(ctx, args, 0, "delete", "first argument", "delete(h, k)")?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "delete key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("delete(h, k)")
        )
    })?;
    let result = hamt_delete(ctx.gc_heap_mut(), handle, &key);
    Ok(Value::Gc(result))
}

/// put(map, key, value) - Returns a new map with the key-value pair added/updated.
pub(super) fn base_put(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_put_borrowed(ctx, &borrowed)
}

pub(super) fn base_put_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 3, "put", "put(map, key, value)")?;
    let handle = arg_hamt_ref(
        ctx,
        args,
        0,
        "put",
        "first argument",
        "put(map, key, value)",
    )?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "put key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("put(map, key, value)")
        )
    })?;
    let result = hamt_insert(ctx.gc_heap_mut(), handle, key, args[2].clone());
    Ok(Value::Gc(result))
}

/// get(map, key) - Returns Some(value) if key exists, None otherwise.
pub(super) fn base_get(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_get_borrowed(ctx, &borrowed)
}

pub(super) fn base_get_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "get", "get(map, key)")?;
    let handle = arg_hamt_ref(ctx, args, 0, "get", "first argument", "get(map, key)")?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "get key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("get(map, key)")
        )
    })?;

    match hamt_lookup(ctx.gc_heap(), handle, &key) {
        Some(value) => Ok(Value::Some(std::rc::Rc::new(value))),
        None => Ok(Value::None),
    }
}

/// is_map(x) - Returns true if x is a HAMT map.
pub(super) fn base_is_map(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_is_map_borrowed(ctx, &borrowed)
}

pub(super) fn base_is_map_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "is_map", "is_map(x)")?;
    let result = match args[0] {
        Value::Gc(h) => is_hamt(ctx.gc_heap(), *h),
        _ => false,
    };
    Ok(Value::Boolean(result))
}
