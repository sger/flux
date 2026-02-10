use crate::runtime::value::Value;

use super::helpers::{arg_array, arg_int, arg_string, check_arity, format_hint};

pub(super) fn builtin_to_string(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 1, "to_string", "to_string(value)")?;
    Ok(Value::String(args[0].to_string_value().into()))
}

pub(super) fn builtin_split(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "split", "split(s, delim)")?;
    let s = arg_string(&args, 0, "split", "first argument", "split(s, delim)")?;
    let delim = arg_string(&args, 1, "split", "second argument", "split(s, delim)")?;
    let parts: Vec<Value> = if delim.is_empty() {
        // Match test expectation: split into characters without empty ends.
        s.chars()
            .map(|ch| Value::String(ch.to_string().into()))
            .collect()
    } else {
        s.split(delim)
            .map(|part| Value::String(part.to_string().into()))
            .collect()
    };
    Ok(Value::Array(parts.into()))
}

pub(super) fn builtin_join(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "join", "join(arr, delim)")?;
    let arr = arg_array(&args, 0, "join", "first argument", "join(arr, delim)")?;
    let delim = arg_string(&args, 1, "join", "second argument", "join(arr, delim)")?;
    let strings: Result<Vec<String>, String> = arr
        .iter()
        .map(|item| match item {
            Value::String(s) => Ok(s.to_string()),
            other => Err(format!(
                "join expected array elements to be String, got {}{}",
                other.type_name(),
                format_hint("join(arr, delim)")
            )),
        })
        .collect();
    Ok(Value::String(strings?.join(delim).into()))
}

pub(super) fn builtin_trim(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 1, "trim", "trim(s)")?;
    let s = arg_string(&args, 0, "trim", "argument", "trim(s)")?;
    Ok(Value::String(s.trim().to_string().into()))
}

pub(super) fn builtin_upper(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 1, "upper", "upper(s)")?;
    let s = arg_string(&args, 0, "upper", "argument", "upper(s)")?;
    Ok(Value::String(s.to_uppercase().into()))
}

pub(super) fn builtin_starts_with(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "starts_with", "starts_with(s, prefix)")?;
    let s = arg_string(
        &args,
        0,
        "starts_with",
        "first argument",
        "starts_with(s, prefix)",
    )?;
    let prefix = arg_string(
        &args,
        1,
        "starts_with",
        "second argument",
        "starts_with(s, prefix)",
    )?;
    Ok(Value::Boolean(s.starts_with(prefix)))
}

pub(super) fn builtin_ends_with(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "ends_with", "ends_with(s, suffix)")?;
    let s = arg_string(
        &args,
        0,
        "ends_with",
        "first argument",
        "ends_with(s, suffix)",
    )?;
    let suffix = arg_string(
        &args,
        1,
        "ends_with",
        "second argument",
        "ends_with(s, suffix)",
    )?;
    Ok(Value::Boolean(s.ends_with(suffix)))
}

pub(super) fn builtin_replace(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 3, "replace", "replace(s, from, to)")?;
    let s = arg_string(
        &args,
        0,
        "replace",
        "first argument",
        "replace(s, from, to)",
    )?;
    let from = arg_string(
        &args,
        1,
        "replace",
        "second argument",
        "replace(s, from, to)",
    )?;
    let to = arg_string(
        &args,
        2,
        "replace",
        "third argument",
        "replace(s, from, to)",
    )?;
    Ok(Value::String(s.replace(from, to).into()))
}

pub(super) fn builtin_lower(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 1, "lower", "lower(s)")?;
    let s = arg_string(&args, 0, "lower", "argument", "lower(s)")?;
    Ok(Value::String(s.to_lowercase().into()))
}

pub(super) fn builtin_chars(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 1, "chars", "chars(s)")?;
    let s = arg_string(&args, 0, "chars", "argument", "chars(s)")?;
    let chars: Vec<Value> = s
        .chars()
        .map(|c| Value::String(c.to_string().into()))
        .collect();
    Ok(Value::Array(chars.into()))
}

pub(super) fn builtin_substring(args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 3, "substring", "substring(s, start, end)")?;
    let s = arg_string(
        &args,
        0,
        "substring",
        "first argument",
        "substring(s, start, end)",
    )?;
    let start = arg_int(
        &args,
        1,
        "substring",
        "second argument",
        "substring(s, start, end)",
    )?;
    let end = arg_int(
        &args,
        2,
        "substring",
        "third argument",
        "substring(s, start, end)",
    )?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let start = if start < 0 { 0 } else { start as usize };
    let end = if end > len {
        len as usize
    } else {
        end as usize
    };
    if start >= end || start >= chars.len() {
        Ok(Value::String(String::new().into()))
    } else {
        let substring: String = chars[start..end].iter().collect();
        Ok(Value::String(substring.into()))
    }
}
