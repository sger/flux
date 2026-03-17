use crate::runtime::{RuntimeContext, value::Value};

use super::helpers::{
    arg_array_ref, arg_int_ref, arg_string_ref, check_arity_range_ref, check_arity_ref, format_hint,
};

pub(super) fn base_to_string(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_to_string_borrowed(_ctx, &borrowed)
}

pub(super) fn base_to_string_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "to_string", "to_string(value)")?;
    Ok(Value::String(
        super::list_ops::format_value(ctx, args[0]).into(),
    ))
}

pub(super) fn base_split(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_split_borrowed(_ctx, &borrowed)
}

pub(super) fn base_split_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "split", "split(s, delim)")?;
    let s = arg_string_ref(args, 0, "split", "first argument", "split(s, delim)")?;
    let delim = arg_string_ref(args, 1, "split", "second argument", "split(s, delim)")?;
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

pub(super) fn base_join(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_join_borrowed(_ctx, &borrowed)
}

pub(super) fn base_join_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "join", "join(arr, delim)")?;
    let arr = arg_array_ref(args, 0, "join", "first argument", "join(arr, delim)")?;
    let delim = arg_string_ref(args, 1, "join", "second argument", "join(arr, delim)")?;
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

pub(super) fn base_trim(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_trim_borrowed(_ctx, &borrowed)
}

pub(super) fn base_trim_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "trim", "trim(s)")?;
    let s = arg_string_ref(args, 0, "trim", "argument", "trim(s)")?;
    Ok(Value::String(s.trim().to_string().into()))
}

pub(super) fn base_upper(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_upper_borrowed(_ctx, &borrowed)
}

pub(super) fn base_upper_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "upper", "upper(s)")?;
    let s = arg_string_ref(args, 0, "upper", "argument", "upper(s)")?;
    Ok(Value::String(s.to_uppercase().into()))
}

pub(super) fn base_starts_with(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_starts_with_borrowed(_ctx, &borrowed)
}

pub(super) fn base_starts_with_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "starts_with", "starts_with(s, prefix)")?;
    let s = arg_string_ref(
        args,
        0,
        "starts_with",
        "first argument",
        "starts_with(s, prefix)",
    )?;
    let prefix = arg_string_ref(
        args,
        1,
        "starts_with",
        "second argument",
        "starts_with(s, prefix)",
    )?;
    Ok(Value::Boolean(s.starts_with(prefix)))
}

pub(super) fn base_ends_with(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_ends_with_borrowed(_ctx, &borrowed)
}

pub(super) fn base_ends_with_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "ends_with", "ends_with(s, suffix)")?;
    let s = arg_string_ref(
        args,
        0,
        "ends_with",
        "first argument",
        "ends_with(s, suffix)",
    )?;
    let suffix = arg_string_ref(
        args,
        1,
        "ends_with",
        "second argument",
        "ends_with(s, suffix)",
    )?;
    Ok(Value::Boolean(s.ends_with(suffix)))
}

pub(super) fn base_replace(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_replace_borrowed(_ctx, &borrowed)
}

pub(super) fn base_replace_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 3, "replace", "replace(s, from, to)")?;
    let s = arg_string_ref(args, 0, "replace", "first argument", "replace(s, from, to)")?;
    let from = arg_string_ref(
        args,
        1,
        "replace",
        "second argument",
        "replace(s, from, to)",
    )?;
    let to = arg_string_ref(args, 2, "replace", "third argument", "replace(s, from, to)")?;
    Ok(Value::String(s.replace(from, to).into()))
}

pub(super) fn base_lower(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_lower_borrowed(_ctx, &borrowed)
}

pub(super) fn base_lower_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "lower", "lower(s)")?;
    let s = arg_string_ref(args, 0, "lower", "argument", "lower(s)")?;
    Ok(Value::String(s.to_lowercase().into()))
}

pub(super) fn base_chars(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_chars_borrowed(_ctx, &borrowed)
}

pub(super) fn base_chars_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "chars", "chars(s)")?;
    let s = arg_string_ref(args, 0, "chars", "argument", "chars(s)")?;
    let chars: Vec<Value> = s
        .chars()
        .map(|c| Value::String(c.to_string().into()))
        .collect();
    Ok(Value::Array(chars.into()))
}

pub(super) fn base_str_contains(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_str_contains_borrowed(_ctx, &borrowed)
}

pub(super) fn base_str_contains_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(
        args,
        2,
        "str_contains",
        "str_contains(haystack, needle)",
    )?;
    let haystack = arg_string_ref(
        args,
        0,
        "str_contains",
        "first argument",
        "str_contains(haystack, needle)",
    )?;
    let needle = arg_string_ref(
        args,
        1,
        "str_contains",
        "second argument",
        "str_contains(haystack, needle)",
    )?;
    Ok(Value::Boolean(haystack.contains(needle)))
}

pub(super) fn base_substring(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_substring_borrowed(_ctx, &borrowed)
}

pub(super) fn base_substring_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_range_ref(args, 2, 3, "substring", "substring(s, start[,end])")?;
    let s = arg_string_ref(
        args,
        0,
        "substring",
        "first argument",
        "substring(s, start[,end])",
    )?;
    let start = arg_int_ref(
        args,
        1,
        "substring",
        "second argument",
        "substring(s, start[,end])",
    )?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let start = if start < 0 { 0 } else { start as usize };
    let end = if args.len() == 3 {
        let e = arg_int_ref(
            args,
            2,
            "substring",
            "third argument",
            "substring(s, start[,end])",
        )?;
        if e < 0 {
            0
        } else if e > len {
            len as usize
        } else {
            e as usize
        }
    } else {
        len as usize
    };
    if start >= end || start >= chars.len() {
        Ok(Value::String(String::new().into()))
    } else {
        let substring: String = chars[start..end].iter().collect();
        Ok(Value::String(substring.into()))
    }
}
