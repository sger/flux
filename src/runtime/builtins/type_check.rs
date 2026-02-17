use crate::runtime::{
    RuntimeContext,
    gc::{HeapObject, hamt::is_hamt},
    value::Value,
};

use super::helpers::check_arity;

pub(super) fn builtin_type_of(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "type_of", "type_of(x)")?;
    let name = match &args[0] {
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => "List",
            HeapObject::HamtNode { .. } | HeapObject::HamtCollision { .. } => "Map",
        },
        other => other.type_name(),
    };

    Ok(Value::String(name.to_string().into()))
}

pub(super) fn builtin_is_int(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "is_int", "is_int(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Integer(_))))
}

pub(super) fn builtin_is_float(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "is_float", "is_float(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Float(_))))
}

pub(super) fn builtin_is_string(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "is_string", "is_string(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::String(_))))
}

pub(super) fn builtin_is_bool(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "is_bool", "is_bool(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Boolean(_))))
}

pub(super) fn builtin_is_array(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "is_array", "is_array(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Array(_))))
}

pub(super) fn builtin_is_hash(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "is_hash", "is_hash(x)")?;
    let result = match &args[0] {
        Value::Gc(h) => is_hamt(ctx.gc_heap(), *h),
        _ => false,
    };
    Ok(Value::Boolean(result))
}

pub(super) fn builtin_is_none(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "is_none", "is_none(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::None)))
}

pub(super) fn builtin_is_some(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "is_some", "is_some(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Some(_))))
}
