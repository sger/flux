use crate::runtime::value::Value;

use super::helpers::{arg_number, check_arity, type_error};

pub(super) fn builtin_abs(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "abs", "abs(n)")?;
    match &args[0] {
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

pub(super) fn builtin_min(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 2, "min", "min(a, b)")?;
    let a = arg_number(&args, 0, "min", "first argument", "min(a, b)")?;
    let b = arg_number(&args, 1, "min", "second argument", "min(a, b)")?;
    let result = a.min(b);
    // Return integer if both inputs were integers and result is whole
    match (&args[0], &args[1]) {
        (Value::Integer(_), Value::Integer(_)) => Ok(Value::Integer(result as i64)),
        _ => Ok(Value::Float(result)),
    }
}

pub(super) fn builtin_max(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 2, "max", "max(a, b)")?;
    let a = arg_number(&args, 0, "max", "first argument", "max(a, b)")?;
    let b = arg_number(&args, 1, "max", "second argument", "max(a, b)")?;
    let result = a.max(b);
    // Return integer if both inputs were integers and result is whole
    match (&args[0], &args[1]) {
        (Value::Integer(_), Value::Integer(_)) => Ok(Value::Integer(result as i64)),
        _ => Ok(Value::Float(result)),
    }
}
