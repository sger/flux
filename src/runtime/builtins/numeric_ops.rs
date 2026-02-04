use crate::runtime::object::Object;

use super::helpers::{arg_number, check_arity, type_error};

pub(super) fn builtin_abs(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "abs", "abs(n)")?;
    match &args[0] {
        Object::Integer(v) => Ok(Object::Integer(v.abs())),
        Object::Float(v) => Ok(Object::Float(v.abs())),
        other => Err(type_error(
            "abs",
            "argument",
            "Number",
            other.type_name(),
            "abs(n)",
        )),
    }
}

pub(super) fn builtin_min(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "min", "min(a, b)")?;
    let a = arg_number(&args, 0, "min", "first argument", "min(a, b)")?;
    let b = arg_number(&args, 1, "min", "second argument", "min(a, b)")?;
    let result = a.min(b);
    // Return integer if both inputs were integers and result is whole
    match (&args[0], &args[1]) {
        (Object::Integer(_), Object::Integer(_)) => Ok(Object::Integer(result as i64)),
        _ => Ok(Object::Float(result)),
    }
}

pub(super) fn builtin_max(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "max", "max(a, b)")?;
    let a = arg_number(&args, 0, "max", "first argument", "max(a, b)")?;
    let b = arg_number(&args, 1, "max", "second argument", "max(a, b)")?;
    let result = a.max(b);
    // Return integer if both inputs were integers and result is whole
    match (&args[0], &args[1]) {
        (Object::Integer(_), Object::Integer(_)) => Ok(Object::Integer(result as i64)),
        _ => Ok(Object::Float(result)),
    }
}
