use crate::runtime::{RuntimeContext, value::Value};

use super::helpers::{arg_number_ref, check_arity_ref, type_error};

pub(super) fn base_abs(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_abs_borrowed(_ctx, &borrowed)
}

pub(super) fn base_abs_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "abs", "abs(n)")?;
    match args[0] {
        Value::Integer(v) => Ok(Value::Integer(v.abs())),
        Value::Float(v) => Ok(Value::Float(v.abs())),
        other => Err(type_error(
            "abs",
            "argument",
            "Number",
            other.type_name(),
            "abs(n)",
        )),
    }
}

pub(super) fn base_min(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_min_borrowed(_ctx, &borrowed)
}

pub(super) fn base_min_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "min", "min(a, b)")?;
    let a = arg_number_ref(args, 0, "min", "first argument", "min(a, b)")?;
    let b = arg_number_ref(args, 1, "min", "second argument", "min(a, b)")?;
    let result = a.min(b);
    // Return integer if both inputs were integers and result is whole
    match (args[0], args[1]) {
        (Value::Integer(_), Value::Integer(_)) => Ok(Value::Integer(result as i64)),
        _ => Ok(Value::Float(result)),
    }
}

pub(super) fn base_max(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_max_borrowed(_ctx, &borrowed)
}

pub(super) fn base_max_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "max", "max(a, b)")?;
    let a = arg_number_ref(args, 0, "max", "first argument", "max(a, b)")?;
    let b = arg_number_ref(args, 1, "max", "second argument", "max(a, b)")?;
    let result = a.max(b);
    // Return integer if both inputs were integers and result is whole
    match (args[0], args[1]) {
        (Value::Integer(_), Value::Integer(_)) => Ok(Value::Integer(result as i64)),
        _ => Ok(Value::Float(result)),
    }
}
