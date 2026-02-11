use std::rc::Rc;

use crate::runtime::{RuntimeContext, value::Value};

use super::helpers::{
    arg_array, arg_int, arg_string, check_arity, check_arity_range, format_hint, type_error,
};

pub(super) fn builtin_len(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "len", "len(value)")?;
    match &args[0] {
        Value::String(s) => Ok(Value::Integer(s.len() as i64)),
        Value::Array(arr) => Ok(Value::Integer(arr.len() as i64)),
        other => Err(type_error(
            "len",
            "argument",
            "String or Array",
            other.type_name(),
            "len(value)",
        )),
    }
}

pub(super) fn builtin_first(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "first", "first(arr)")?;
    let arr = arg_array(&args, 0, "first", "argument", "first(arr)")?;
    if arr.is_empty() {
        Ok(Value::None)
    } else {
        Ok(arr[0].clone())
    }
}

pub(super) fn builtin_last(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "last", "last(arr)")?;
    let arr = arg_array(&args, 0, "last", "argument", "last(arr)")?;
    if arr.is_empty() {
        Ok(Value::None)
    } else {
        Ok(arr[arr.len() - 1].clone())
    }
}

pub(super) fn builtin_rest(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "rest", "rest(arr)")?;
    let arr = arg_array(&args, 0, "rest", "argument", "rest(arr)")?;
    if arr.is_empty() {
        Ok(Value::None)
    } else {
        Ok(Value::Array(arr[1..].to_vec().into()))
    }
}

pub(super) fn builtin_push(
    _ctx: &mut dyn RuntimeContext,
    mut args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "push", "push(arr, elem)")?;
    let elem = args.swap_remove(1);
    let arr_obj = args.swap_remove(0);

    match arr_obj {
        Value::Array(mut arr) => {
            Rc::make_mut(&mut arr).push(elem);
            Ok(Value::Array(arr))
        }
        other => Err(type_error(
            "push",
            "first argument",
            "Array",
            other.type_name(),
            "push(arr, elem)",
        )),
    }
}

pub(super) fn builtin_concat(
    _ctx: &mut dyn RuntimeContext,
    mut args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "concat", "concat(a, b)")?;
    let b_obj = args.swap_remove(1);
    let a_obj = args.swap_remove(0);

    match (a_obj, b_obj) {
        (Value::Array(mut a), Value::Array(b)) => {
            Rc::make_mut(&mut a).extend(b.iter().cloned());
            Ok(Value::Array(a))
        }
        (left, right) => {
            if !matches!(left, Value::Array(_)) {
                return Err(type_error(
                    "concat",
                    "first argument",
                    "Array",
                    left.type_name(),
                    "concat(a, b)",
                ));
            }
            Err(type_error(
                "concat",
                "second argument",
                "Array",
                right.type_name(),
                "concat(a, b)",
            ))
        }
    }
}

pub(super) fn builtin_reverse(
    _ctx: &mut dyn RuntimeContext,
    mut args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "reverse", "reverse(arr)")?;

    match args.swap_remove(0) {
        Value::Array(mut arr) => {
            Rc::make_mut(&mut arr).reverse();
            Ok(Value::Array(arr))
        }
        other => Err(type_error(
            "reverse",
            "argument",
            "Array",
            other.type_name(),
            "reverse(arr)",
        )),
    }
}

pub(super) fn builtin_contains(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
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

pub(super) fn builtin_slice(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
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
pub(super) fn builtin_sort(
    _ctx: &mut dyn RuntimeContext,
    mut args: Vec<Value>,
) -> Result<Value, String> {
    check_arity_range(&args, 1, 2, "sort", "sort(arr, order)")?;

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

    let mut arr = match args.swap_remove(0) {
        Value::Array(arr) => arr,
        other => {
            return Err(type_error(
                "sort",
                "first argument",
                "Array",
                other.type_name(),
                "sort(arr, order)",
            ));
        }
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

    Rc::make_mut(&mut arr).sort_by(|a, b| {
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
    Ok(Value::Array(arr))
}

/// map(arr, fn) - Apply fn to each element, return new array of results
///
/// Callback signature: fn(element) - must accept exactly 1 argument
/// Elements are processed in left-to-right order
pub(super) fn builtin_map(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "map", "map(arr, fun)")?;
    let arr = arg_array(&args, 0, "map", "first argument", "map(arr, fun)")?;
    let func = args[1].clone();

    match &func {
        Value::Closure(_) | Value::Builtin(_) => {}
        other => {
            return Err(type_error(
                "map",
                "second argument",
                "Function",
                other.type_name(),
                "map(arr, fn)",
            ));
        }
    }

    let mut results = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        let result = ctx
            .invoke_value(func.clone(), vec![item.clone()])
            .map_err(|e| format!("map: callback error at index {}: {}", idx, e))?;
        results.push(result);
    }
    Ok(Value::Array(results.into()))
}

/// filter(arr, pred) - Keep elements where pred returns truthy
///
/// Callback signature: pred(element) - must accept exactly 1 argument
/// Truthiness: Only `Boolean(false)` and `None` are falsy; all other values are truthy
/// Elements are processed in left-to-right order
pub(super) fn builtin_filter(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "filter", "filter(arr, pred)")?;
    let arr = arg_array(&args, 0, "filter", "first argument", "filter(arr, pred)")?;
    let func = args[1].clone();

    match &func {
        Value::Closure(_) | Value::Builtin(_) => {}
        other => {
            return Err(type_error(
                "filter",
                "second argument",
                "Function",
                other.type_name(),
                "filter(arr, pred)",
            ));
        }
    }

    let mut results = Vec::new();
    for (idx, item) in arr.iter().enumerate() {
        let result = ctx
            .invoke_value(func.clone(), vec![item.clone()])
            .map_err(|e| format!("filter: callback error at index {}: {}", idx, e))?;
        if result.is_truthy() {
            results.push(item.clone());
        }
    }
    Ok(Value::Array(results.into()))
}

/// fold(arr, initial, fn) - Reduce array to single value
///
/// Callback signature: fn(accumulator, element) - must accept exactly 2 arguments
/// Left fold (foldl) semantics: processes elements in left-to-right order
/// Returns initial value unchanged if array is empty
pub(super) fn builtin_fold(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 3, "fold", "fold(arr, initial, fun)")?;
    let arr = arg_array(
        &args,
        0,
        "fold",
        "first argument",
        "fold(arr, initial, fun)",
    )?;
    let mut acc = args[1].clone();
    let func = args[2].clone();

    match &func {
        Value::Closure(_) | Value::Builtin(_) => {}
        other => {
            return Err(type_error(
                "fold",
                "third argument",
                "Function",
                other.type_name(),
                "fold(arr, initial, fun)",
            ));
        }
    }

    for (idx, item) in arr.iter().enumerate() {
        acc = ctx
            .invoke_value(func.clone(), vec![acc, item.clone()])
            .map_err(|e| format!("fold: callback error at index {}: {}", idx, e))?;
    }
    Ok(acc)
}
