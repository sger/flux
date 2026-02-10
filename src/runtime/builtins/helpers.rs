use crate::runtime::{hash_key::HashKey, value::Value};
use std::collections::HashMap;

pub(super) fn format_hint(signature: &str) -> String {
    format!("\n\nHint:\n  {}", signature)
}

pub(super) fn arity_error(name: &str, expected: &str, got: usize, signature: &str) -> String {
    format!(
        "wrong number of arguments\n\n  function: {}/{}\n  expected: {}\n  got: {}{}",
        name,
        expected,
        expected,
        got,
        format_hint(signature)
    )
}

pub(super) fn type_error(
    name: &str,
    label: &str,
    expected: &str,
    got: &str,
    signature: &str,
) -> String {
    format!(
        "{} expected {} to be {}, got {}{}",
        name,
        label,
        expected,
        got,
        format_hint(signature)
    )
}

pub(super) fn check_arity(
    args: &[Value],
    expected: usize,
    name: &str,
    signature: &str,
) -> Result<(), String> {
    if args.len() != expected {
        return Err(arity_error(
            name,
            &expected.to_string(),
            args.len(),
            signature,
        ));
    }
    Ok(())
}

pub(super) fn check_arity_range(
    args: &[Value],
    min: usize,
    max: usize,
    name: &str,
    signature: &str,
) -> Result<(), String> {
    if args.len() < min || args.len() > max {
        return Err(arity_error(
            name,
            &format!("{}..{}", min, max),
            args.len(),
            signature,
        ));
    }
    Ok(())
}

pub(super) fn arg_string<'a>(
    args: &'a [Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a str, String> {
    match &args[index] {
        Value::String(s) => Ok(s.as_str()),
        other => Err(type_error(
            name,
            label,
            "String",
            other.type_name(),
            signature,
        )),
    }
}

pub(super) fn arg_array<'a>(
    args: &'a [Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a Vec<Value>, String> {
    match &args[index] {
        Value::Array(arr) => Ok(arr),
        other => Err(type_error(
            name,
            label,
            "Array",
            other.type_name(),
            signature,
        )),
    }
}

pub(super) fn arg_int(
    args: &[Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<i64, String> {
    match &args[index] {
        Value::Integer(value) => Ok(*value),
        other => Err(type_error(
            name,
            label,
            "Integer",
            other.type_name(),
            signature,
        )),
    }
}

pub(super) fn arg_hash<'a>(
    args: &'a [Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a HashMap<HashKey, Value>, String> {
    match &args[index] {
        Value::Hash(h) => Ok(h),
        other => Err(type_error(
            name,
            label,
            "Hash",
            other.type_name(),
            signature,
        )),
    }
}

pub(super) fn arg_number(
    args: &[Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<f64, String> {
    match &args[index] {
        Value::Integer(v) => Ok(*v as f64),
        Value::Float(v) => Ok(*v),
        other => Err(type_error(
            name,
            label,
            "Number",
            other.type_name(),
            signature,
        )),
    }
}
