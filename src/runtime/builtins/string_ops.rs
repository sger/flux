use crate::runtime::object::Object;

use super::helpers::{arg_array, arg_int, arg_string, check_arity, format_hint};

pub(super) fn builtin_to_string(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "to_string", "to_string(value)")?;
    Ok(Object::String(args[0].to_string_value()))
}

pub(super) fn builtin_split(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "split", "split(s, delim)")?;
    let s = arg_string(&args, 0, "split", "first argument", "split(s, delim)")?;
    let delim = arg_string(&args, 1, "split", "second argument", "split(s, delim)")?;
    let parts: Vec<Object> = if delim.is_empty() {
        // Match test expectation: split into characters without empty ends.
        s.chars().map(|ch| Object::String(ch.to_string())).collect()
    } else {
        s.split(delim)
            .map(|part| Object::String(part.to_string()))
            .collect()
    };
    Ok(Object::Array(parts))
}

pub(super) fn builtin_join(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "join", "join(arr, delim)")?;
    let arr = arg_array(&args, 0, "join", "first argument", "join(arr, delim)")?;
    let delim = arg_string(&args, 1, "join", "second argument", "join(arr, delim)")?;
    let strings: Result<Vec<String>, String> = arr
        .iter()
        .map(|item| match item {
            Object::String(s) => Ok(s.clone()),
            other => Err(format!(
                "join expected array elements to be String, got {}{}",
                other.type_name(),
                format_hint("join(arr, delim)")
            )),
        })
        .collect();
    Ok(Object::String(strings?.join(delim)))
}

pub(super) fn builtin_trim(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "trim", "trim(s)")?;
    let s = arg_string(&args, 0, "trim", "argument", "trim(s)")?;
    Ok(Object::String(s.trim().to_string()))
}

pub(super) fn builtin_upper(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "upper", "upper(s)")?;
    let s = arg_string(&args, 0, "upper", "argument", "upper(s)")?;
    Ok(Object::String(s.to_uppercase()))
}

pub(super) fn builtin_lower(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "lower", "lower(s)")?;
    let s = arg_string(&args, 0, "lower", "argument", "lower(s)")?;
    Ok(Object::String(s.to_lowercase()))
}

pub(super) fn builtin_chars(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "chars", "chars(s)")?;
    let s = arg_string(&args, 0, "chars", "argument", "chars(s)")?;
    let chars: Vec<Object> = s.chars().map(|c| Object::String(c.to_string())).collect();
    Ok(Object::Array(chars))
}

pub(super) fn builtin_substring(args: Vec<Object>) -> Result<Object, String> {
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
        Ok(Object::String(String::new()))
    } else {
        Ok(Object::String(chars[start..end].iter().collect()))
    }
}
