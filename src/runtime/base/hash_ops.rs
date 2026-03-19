use crate::runtime::{
    RuntimeContext,
    gc::{
        gc_handle::GcHandle,
        hamt::{hamt_delete, hamt_insert, hamt_iter, hamt_lookup, is_hamt},
    },
    hamt as rc_hamt,
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

/// Identifies which HAMT representation a map value uses.
enum HamtRef<'a> {
    /// New Rc-based HAMT (Aether Phase 3).
    Rc(&'a std::rc::Rc<rc_hamt::HamtNode>),
    /// Legacy GC-based HAMT.
    Gc(GcHandle),
}

fn arg_hamt_any<'a>(
    ctx: &dyn RuntimeContext,
    args: &'a [&'a Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<HamtRef<'a>, String> {
    match args[index] {
        Value::HashMap(node) => Ok(HamtRef::Rc(node)),
        Value::Gc(h) if is_hamt(ctx.gc_heap(), *h) => Ok(HamtRef::Gc(*h)),
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
    let href = arg_hamt_any(ctx, args, 0, "keys", "argument", "keys(h)")?;
    let pairs = match href {
        HamtRef::Rc(node) => rc_hamt::hamt_iter(node),
        HamtRef::Gc(handle) => hamt_iter(ctx.gc_heap(), handle),
    };
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
    let href = arg_hamt_any(ctx, args, 0, "values", "argument", "values(h)")?;
    let pairs = match href {
        HamtRef::Rc(node) => rc_hamt::hamt_iter(node),
        HamtRef::Gc(handle) => hamt_iter(ctx.gc_heap(), handle),
    };
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
    let href = arg_hamt_any(ctx, args, 0, "has_key", "first argument", "has_key(h, k)")?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "has_key key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("has_key(h, k)")
        )
    })?;
    let found = match href {
        HamtRef::Rc(node) => rc_hamt::hamt_lookup(node, &key).is_some(),
        HamtRef::Gc(handle) => hamt_lookup(ctx.gc_heap(), handle, &key).is_some(),
    };
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
    let href1 = arg_hamt_any(ctx, args, 0, "merge", "first argument", "merge(h1, h2)")?;
    let href2 = arg_hamt_any(ctx, args, 1, "merge", "second argument", "merge(h1, h2)")?;

    // Get pairs from h2
    let pairs = match href2 {
        HamtRef::Rc(node) => rc_hamt::hamt_iter(node),
        HamtRef::Gc(handle) => hamt_iter(ctx.gc_heap(), handle),
    };

    // Insert into h1 -- always produce Rc-based result
    match href1 {
        HamtRef::Rc(node) => {
            let mut result = std::rc::Rc::clone(node);
            for (k, v) in pairs {
                result = rc_hamt::hamt_insert(&result, k, v);
            }
            Ok(Value::HashMap(result))
        }
        HamtRef::Gc(handle) => {
            let mut result = handle;
            for (k, v) in pairs {
                result = hamt_insert(ctx.gc_heap_mut(), result, k, v);
            }
            Ok(Value::Gc(result))
        }
    }
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
    let href = arg_hamt_any(ctx, args, 0, "delete", "first argument", "delete(h, k)")?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "delete key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("delete(h, k)")
        )
    })?;
    match href {
        HamtRef::Rc(node) => Ok(Value::HashMap(rc_hamt::hamt_delete(node, &key))),
        HamtRef::Gc(handle) => Ok(Value::Gc(hamt_delete(ctx.gc_heap_mut(), handle, &key))),
    }
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
    let href = arg_hamt_any(
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
    match href {
        HamtRef::Rc(node) => Ok(Value::HashMap(rc_hamt::hamt_insert(
            node,
            key,
            args[2].clone(),
        ))),
        HamtRef::Gc(handle) => {
            let result = hamt_insert(ctx.gc_heap_mut(), handle, key, args[2].clone());
            Ok(Value::Gc(result))
        }
    }
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
    let href = arg_hamt_any(ctx, args, 0, "get", "first argument", "get(map, key)")?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "get key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("get(map, key)")
        )
    })?;

    let result = match href {
        HamtRef::Rc(node) => rc_hamt::hamt_lookup(node, &key),
        HamtRef::Gc(handle) => hamt_lookup(ctx.gc_heap(), handle, &key),
    };
    match result {
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
        Value::HashMap(_) => true,
        Value::Gc(h) => is_hamt(ctx.gc_heap(), *h),
        _ => false,
    };
    Ok(Value::Boolean(result))
}
