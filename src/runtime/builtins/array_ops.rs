use crate::runtime::{object::Object, value::Value};

use super::helpers::{
    arg_array, arg_int, arg_string, check_arity, check_arity_range, format_hint, type_error,
};

pub(super) fn builtin_len(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "len", "len(value)")?;
    match &args[0] {
        Object::String(s) => Ok(Value::Integer(s.len() as i64)),
        Object::Array(arr) => Ok(Value::Integer(arr.len() as i64)),
        other => Err(type_error(
            "len",
            "argument",
            "String or Array",
            other.type_name(),
            "len(value)",
        )),
    }
}

pub(super) fn builtin_first(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "first", "first(arr)")?;
    let arr = arg_array(&args, 0, "first", "argument", "first(arr)")?;
    if arr.is_empty() {
        Ok(Value::None)
    } else {
        Ok(arr[0].clone())
    }
}

pub(super) fn builtin_last(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "last", "last(arr)")?;
    let arr = arg_array(&args, 0, "last", "argument", "last(arr)")?;
    if arr.is_empty() {
        Ok(Value::None)
    } else {
        Ok(arr[arr.len() - 1].clone())
    }
}

pub(super) fn builtin_rest(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "rest", "rest(arr)")?;
    let arr = arg_array(&args, 0, "rest", "argument", "rest(arr)")?;
    if arr.is_empty() {
        Ok(Value::None)
    } else {
        Ok(Value::Array(arr[1..].to_vec().into()))
    }
}

pub(super) fn builtin_push(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 2, "push", "push(arr, elem)")?;
    let arr = arg_array(&args, 0, "push", "first argument", "push(arr, elem)")?;
    let mut new_arr = arr.clone();
    new_arr.push(args[1].clone());
    Ok(Value::Array(new_arr.into()))
}

pub(super) fn builtin_concat(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 2, "concat", "concat(a, b)")?;
    let a = arg_array(&args, 0, "concat", "first argument", "concat(a, b)")?;
    let b = arg_array(&args, 1, "concat", "second argument", "concat(a, b)")?;
    let mut result = a.clone();
    result.extend(b.iter().cloned());
    Ok(Value::Array(result.into()))
}

pub(super) fn builtin_reverse(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "reverse", "reverse(arr)")?;
    let arr = arg_array(&args, 0, "reverse", "argument", "reverse(arr)")?;
    let mut result = arr.clone();
    result.reverse();
    Ok(Value::Array(result.into()))
}

pub(super) fn builtin_contains(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 2, "contains", "contains(arr, elem)")?;
    let arr = arg_array(
        &args,
        0,
        "contains",
        "first argument",
        "contains(arr, elem)",
    )?;
    let elem = &args[1];
    let found = arr.iter().any(|item| item == elem);
    Ok(Value::Boolean(found))
}

pub(super) fn builtin_slice(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 3, "slice", "slice(arr, start, end)")?;
    let arr = arg_array(
        &args,
        0,
        "slice",
        "first argument",
        "slice(arr, start, end)",
    )?;
    let start = arg_int(
        &args,
        1,
        "slice",
        "second argument",
        "slice(arr, start, end)",
    )?;
    let end = arg_int(
        &args,
        2,
        "slice",
        "third argument",
        "slice(arr, start, end)",
    )?;
    let len = arr.len() as i64;
    let start = if start < 0 { 0 } else { start as usize };
    let end = if end > len {
        len as usize
    } else {
        end as usize
    };
    if start >= end || start >= arr.len() {
        Ok(Value::Array(vec![].into()))
    } else {
        Ok(Value::Array(arr[start..end].to_vec().into()))
    }
}

/// sort(arr) or sort(arr, order) - Return a new sorted array
/// order: "asc" (default) or "desc"
/// Only works with integers/floats
pub(super) fn builtin_sort(args: &[Value]) -> Result<Value, String> {
    check_arity_range(&args, 1, 2, "sort", "sort(arr, order)")?;
    let arr = arg_array(&args, 0, "sort", "first argument", "sort(arr, order)")?;
    // Determine sort order (default: ascending)
    let descending = if args.len() == 2 {
        match arg_string(&args, 1, "sort", "second argument", "sort(arr, order)")? {
            "asc" => false,
            "desc" => true,
            other => {
                return Err(format!(
                    "sort order must be \"asc\" or \"desc\", got \"{}\"{}",
                    other,
                    format_hint("sort(arr, order)")
                ));
            }
        }
    } else {
        false
    };

    // Check if all elements are comparable (integers or floats)
    let all_numeric = arr
        .iter()
        .all(|item| matches!(item, Value::Integer(_) | Value::Float(_)));

    if !all_numeric && !arr.is_empty() {
        return Err(format!(
            "sort only supports arrays of Integers or Floats{}",
            format_hint("sort(arr, order)")
        ));
    }

    let mut result = arr.clone();

    result.sort_by(|a, b| {
        use std::cmp::Ordering;
        // Smart comparison: avoid f64 conversion when both are same type
        let cmp = match (a, b) {
            (Value::Integer(i1), Value::Integer(i2)) => i1.cmp(i2),
            (Value::Float(f1), Value::Float(f2)) => f1.partial_cmp(f2).unwrap_or(Ordering::Equal),
            (Value::Integer(i), Value::Float(f)) => {
                (*i as f64).partial_cmp(f).unwrap_or(Ordering::Equal)
            }
            (Value::Float(f), Value::Integer(i)) => {
                f.partial_cmp(&(*i as f64)).unwrap_or(Ordering::Equal)
            }
            _ => Ordering::Equal,
        };
        if descending { cmp.reverse() } else { cmp }
    });
    Ok(Value::Array(result.into()))
}
