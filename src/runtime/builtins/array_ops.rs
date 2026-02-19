use std::rc::Rc;

use crate::runtime::{
    RuntimeContext,
    gc::{HeapObject, hamt::hamt_len},
    value::Value,
};

use super::helpers::{
    arg_array, arg_int, arg_string, check_arity, check_arity_range, format_hint, type_error,
};
use super::list_ops;

pub(super) fn builtin_len(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 1, "len", "len(value)")?;
    match &args[0] {
        Value::String(s) => Ok(Value::Integer(s.len() as i64)),
        Value::Array(arr) => Ok(Value::Integer(arr.len() as i64)),
        Value::Tuple(tuple) => Ok(Value::Integer(tuple.len() as i64)),
        Value::None | Value::EmptyList => Ok(Value::Integer(0)),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => match list_ops::list_len(ctx, &args[0]) {
                Some(len) => Ok(Value::Integer(len as i64)),
                None => Err("len: malformed list".to_string()),
            },
            HeapObject::HamtNode { .. } | HeapObject::HamtCollision { .. } => {
                Ok(Value::Integer(hamt_len(ctx.gc_heap(), *h) as i64))
            }
        },
        other => Err(type_error(
            "len",
            "argument",
            "String, Array, Tuple, List, or Map",
            other.type_name(),
            "len(value)",
        )),
    }
}

pub(super) fn builtin_first(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "first", "first(collection)")?;
    match &args[0] {
        Value::Array(arr) => {
            if arr.is_empty() {
                Ok(Value::None)
            } else {
                Ok(arr[0].clone())
            }
        }
        Value::None | Value::EmptyList => Ok(Value::None),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { head, .. } => Ok(head.clone()),
            _ => Err(type_error(
                "first",
                "argument",
                "Array or List",
                "Map",
                "first(collection)",
            )),
        },
        other => Err(type_error(
            "first",
            "argument",
            "Array or List",
            other.type_name(),
            "first(collection)",
        )),
    }
}

pub(super) fn builtin_last(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "last", "last(collection)")?;
    match &args[0] {
        Value::Array(arr) => {
            if arr.is_empty() {
                Ok(Value::None)
            } else {
                Ok(arr[arr.len() - 1].clone())
            }
        }
        Value::None | Value::EmptyList => Ok(Value::None),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => match list_ops::collect_list(ctx, &args[0]) {
                Some(elems) if elems.is_empty() => Ok(Value::None),
                Some(elems) => Ok(elems.into_iter().last().unwrap()),
                None => Err("last: malformed list".to_string()),
            },
            _ => Err(type_error(
                "last",
                "argument",
                "Array or List",
                "Map",
                "last(collection)",
            )),
        },
        other => Err(type_error(
            "last",
            "argument",
            "Array or List",
            other.type_name(),
            "last(collection)",
        )),
    }
}

pub(super) fn builtin_rest(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "rest", "rest(collection)")?;
    match &args[0] {
        Value::Array(arr) => {
            if arr.is_empty() {
                Ok(Value::None)
            } else {
                Ok(Value::Array(arr[1..].to_vec().into()))
            }
        }
        Value::None | Value::EmptyList => Ok(Value::None),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { tail, .. } => Ok(tail.clone()),
            _ => Err(type_error(
                "rest",
                "argument",
                "Array or List",
                "Map",
                "rest(collection)",
            )),
        },
        other => Err(type_error(
            "rest",
            "argument",
            "Array or List",
            other.type_name(),
            "rest(collection)",
        )),
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
    ctx: &mut dyn RuntimeContext,
    mut args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "reverse", "reverse(collection)")?;

    match &args[0] {
        Value::Array(_) => {
            let arr_val = args.swap_remove(0);
            match arr_val {
                Value::Array(mut arr) => {
                    Rc::make_mut(&mut arr).reverse();
                    Ok(Value::Array(arr))
                }
                _ => unreachable!(),
            }
        }
        Value::None | Value::EmptyList => Ok(Value::None),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("reverse: malformed list")?;
                let mut list = Value::None;
                for elem in elements {
                    let handle = ctx.gc_heap_mut().alloc(HeapObject::Cons {
                        head: elem,
                        tail: list,
                    });
                    list = Value::Gc(handle);
                }
                Ok(list)
            }
            _ => Err(type_error(
                "reverse",
                "argument",
                "Array or List",
                "Map",
                "reverse(collection)",
            )),
        },
        other => Err(type_error(
            "reverse",
            "argument",
            "Array or List",
            other.type_name(),
            "reverse(collection)",
        )),
    }
}

pub(super) fn builtin_contains(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "contains", "contains(collection, elem)")?;
    let elem = &args[1];
    match &args[0] {
        Value::Array(arr) => {
            let found = arr.iter().any(|item| item == elem);
            Ok(Value::Boolean(found))
        }
        Value::None | Value::EmptyList => Ok(Value::Boolean(false)),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("contains: malformed list")?;
                let found = elements.iter().any(|item| item == elem);
                Ok(Value::Boolean(found))
            }
            _ => Err(type_error(
                "contains",
                "first argument",
                "Array or List",
                "Map",
                "contains(collection, elem)",
            )),
        },
        other => Err(type_error(
            "contains",
            "first argument",
            "Array or List",
            other.type_name(),
            "contains(collection, elem)",
        )),
    }
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

/// range(start, end) - Build an integer range as an array.
///
/// End is exclusive. Supports ascending and descending ranges.
pub(super) fn builtin_range(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "range", "range(start, end)")?;
    let start = arg_int(&args, 0, "range", "first argument", "range(start, end)")?;
    let end = arg_int(&args, 1, "range", "second argument", "range(start, end)")?;

    let mut out = Vec::new();
    if start < end {
        let mut i = start;
        while i < end {
            out.push(Value::Integer(i));
            i += 1;
        }
    } else if start > end {
        let mut i = start;
        while i > end {
            out.push(Value::Integer(i));
            i -= 1;
        }
    }
    Ok(Value::Array(out.into()))
}

fn aggregate_numeric(
    values: &[Value],
    name: &str,
    signature: &str,
    product: bool,
) -> Result<Value, String> {
    let mut int_acc: i64 = if product { 1 } else { 0 };
    let mut float_acc: f64 = if product { 1.0 } else { 0.0 };
    let mut has_float = false;

    for value in values {
        match value {
            Value::Integer(v) => {
                if has_float {
                    if product {
                        float_acc *= *v as f64;
                    } else {
                        float_acc += *v as f64;
                    }
                } else if product {
                    int_acc = int_acc.checked_mul(*v).ok_or_else(|| {
                        format!("{}: integer overflow{}", name, format_hint(signature))
                    })?;
                } else {
                    int_acc = int_acc.checked_add(*v).ok_or_else(|| {
                        format!("{}: integer overflow{}", name, format_hint(signature))
                    })?;
                }
            }
            Value::Float(v) => {
                if !has_float {
                    float_acc = int_acc as f64;
                    has_float = true;
                }
                if product {
                    float_acc *= *v;
                } else {
                    float_acc += *v;
                }
            }
            other => {
                return Err(type_error(
                    name,
                    "array elements",
                    "Integer or Float",
                    other.type_name(),
                    signature,
                ));
            }
        }
    }

    if has_float {
        Ok(Value::Float(float_acc))
    } else {
        Ok(Value::Integer(int_acc))
    }
}

/// sum(collection) - Sum numeric elements in an array or list.
pub(super) fn builtin_sum(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 1, "sum", "sum(collection)")?;
    match &args[0] {
        Value::Array(arr) => aggregate_numeric(arr, "sum", "sum(collection)", false),
        Value::None | Value::EmptyList => Ok(Value::Integer(0)),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("sum: malformed list")?;
                aggregate_numeric(&elements, "sum", "sum(collection)", false)
            }
            _ => Err(type_error(
                "sum",
                "argument",
                "Array or List",
                "Map",
                "sum(collection)",
            )),
        },
        other => Err(type_error(
            "sum",
            "argument",
            "Array or List",
            other.type_name(),
            "sum(collection)",
        )),
    }
}

/// product(collection) - Multiply numeric elements in an array or list.
pub(super) fn builtin_product(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "product", "product(collection)")?;
    match &args[0] {
        Value::Array(arr) => aggregate_numeric(arr, "product", "product(collection)", true),
        Value::None | Value::EmptyList => Ok(Value::Integer(1)),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("product: malformed list")?;
                aggregate_numeric(&elements, "product", "product(collection)", true)
            }
            _ => Err(type_error(
                "product",
                "argument",
                "Array or List",
                "Map",
                "product(collection)",
            )),
        },
        other => Err(type_error(
            "product",
            "argument",
            "Array or List",
            other.type_name(),
            "product(collection)",
        )),
    }
}

fn invoke_unary_callback(
    ctx: &mut dyn RuntimeContext,
    func: &Value,
    arg: Value,
) -> Result<Value, String> {
    ctx.invoke_unary_value(func, arg)
}

fn invoke_binary_callback(
    ctx: &mut dyn RuntimeContext,
    func: &Value,
    left: Value,
    right: Value,
) -> Result<Value, String> {
    ctx.invoke_binary_value(func, left, right)
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

/// map(collection, fn) - Apply fn to each element, return new collection of results
///
/// Callback signature: fn(element) - must accept exactly 1 argument
/// Elements are processed in left-to-right order
/// Works on Arrays (returns Array) and Lists (returns List)
pub(super) fn builtin_map(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "map", "map(collection, fn)")?;
    let func = &args[1];

    match func {
        Value::Closure(_) | Value::Builtin(_) | Value::JitClosure(_) => {}
        other => {
            return Err(type_error(
                "map",
                "second argument",
                "Function",
                other.type_name(),
                "map(collection, fn)",
            ));
        }
    }

    match &args[0] {
        Value::Array(arr) => {
            let mut results = Vec::with_capacity(arr.len());
            for (idx, item) in arr.iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item.clone())
                    .map_err(|e| format!("map: callback error at index {}: {}", idx, e))?;
                results.push(result);
            }
            Ok(Value::Array(results.into()))
        }
        Value::None | Value::EmptyList => Ok(Value::None),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("map: malformed list")?;
                let mut results = Vec::with_capacity(elements.len());
                for (idx, item) in elements.into_iter().enumerate() {
                    let result = invoke_unary_callback(ctx, func, item)
                        .map_err(|e| format!("map: callback error at index {}: {}", idx, e))?;
                    results.push(result);
                }
                // Build a cons list from results
                let mut list = Value::None;
                for elem in results.into_iter().rev() {
                    let handle = ctx.gc_heap_mut().alloc(HeapObject::Cons {
                        head: elem,
                        tail: list,
                    });
                    list = Value::Gc(handle);
                }
                Ok(list)
            }
            _ => Err(type_error(
                "map",
                "first argument",
                "Array or List",
                "Map",
                "map(collection, fn)",
            )),
        },
        other => Err(type_error(
            "map",
            "first argument",
            "Array or List",
            other.type_name(),
            "map(collection, fn)",
        )),
    }
}

/// filter(collection, pred) - Keep elements where pred returns truthy
///
/// Callback signature: pred(element) - must accept exactly 1 argument
/// Truthiness: Only `Boolean(false)` and `None` are falsy; all other values are truthy
/// Elements are processed in left-to-right order
/// Works on Arrays (returns Array) and Lists (returns List)
pub(super) fn builtin_filter(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "filter", "filter(collection, pred)")?;
    let func = &args[1];

    match func {
        Value::Closure(_) | Value::Builtin(_) | Value::JitClosure(_) => {}
        other => {
            return Err(type_error(
                "filter",
                "second argument",
                "Function",
                other.type_name(),
                "filter(collection, pred)",
            ));
        }
    }

    match &args[0] {
        Value::Array(arr) => {
            let mut results = Vec::new();
            for (idx, item) in arr.iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item.clone())
                    .map_err(|e| format!("filter: callback error at index {}: {}", idx, e))?;
                if result.is_truthy() {
                    results.push(item.clone());
                }
            }
            Ok(Value::Array(results.into()))
        }
        Value::None | Value::EmptyList => Ok(Value::None),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("filter: malformed list")?;
                let mut results = Vec::new();
                for (idx, item) in elements.into_iter().enumerate() {
                    let result = invoke_unary_callback(ctx, func, item.clone())
                        .map_err(|e| format!("filter: callback error at index {}: {}", idx, e))?;
                    if result.is_truthy() {
                        results.push(item);
                    }
                }
                // Build a cons list from results
                let mut list = Value::None;
                for elem in results.into_iter().rev() {
                    let handle = ctx.gc_heap_mut().alloc(HeapObject::Cons {
                        head: elem,
                        tail: list,
                    });
                    list = Value::Gc(handle);
                }
                Ok(list)
            }
            _ => Err(type_error(
                "filter",
                "first argument",
                "Array or List",
                "Map",
                "filter(collection, pred)",
            )),
        },
        other => Err(type_error(
            "filter",
            "first argument",
            "Array or List",
            other.type_name(),
            "filter(collection, pred)",
        )),
    }
}

/// flat_map(collection, fn) - Map each element to a collection, then flatten
///
/// Callback signature: fn(element) -> collection — must accept exactly 1 argument
/// The callback must return the same collection type as the input (Array → Array, List → List)
/// Elements are processed in left-to-right order
/// Works on Arrays (returns Array) and Lists (returns List)
pub(super) fn builtin_flat_map(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "flat_map", "flat_map(collection, fn)")?;
    let func = &args[1];

    match func {
        Value::Closure(_) | Value::Builtin(_) | Value::JitClosure(_) => {}
        other => {
            return Err(type_error(
                "flat_map",
                "second argument",
                "Function",
                other.type_name(),
                "flat_map(collection, fn)",
            ));
        }
    }

    match &args[0] {
        Value::Array(arr) => {
            let mut results = Vec::new();
            for (idx, item) in arr.iter().enumerate() {
                let inner = invoke_unary_callback(ctx, func, item.clone())
                    .map_err(|e| format!("flat_map: callback error at index {}: {}", idx, e))?;
                match inner {
                    Value::Array(inner_arr) => results.extend(inner_arr.iter().cloned()),
                    Value::None | Value::EmptyList => {}
                    other => {
                        return Err(format!(
                            "flat_map: callback must return an Array when input is Array, got {}",
                            other.type_name()
                        ));
                    }
                }
            }
            Ok(Value::Array(results.into()))
        }
        Value::None | Value::EmptyList => Ok(Value::None),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("flat_map: malformed list")?;
                let mut results: Vec<Value> = Vec::new();
                for (idx, item) in elements.into_iter().enumerate() {
                    let inner = invoke_unary_callback(ctx, func, item)
                        .map_err(|e| format!("flat_map: callback error at index {}: {}", idx, e))?;
                    match inner {
                        Value::None | Value::EmptyList => {}
                        Value::Gc(_) => {
                            let inner_elems = list_ops::collect_list(ctx, &inner)
                                .ok_or("flat_map: callback returned malformed list")?;
                            results.extend(inner_elems);
                        }
                        other => {
                            return Err(format!(
                                "flat_map: callback must return a List when input is List, got {}",
                                other.type_name()
                            ));
                        }
                    }
                }
                // Build cons list from results
                let mut list = Value::None;
                for elem in results.into_iter().rev() {
                    let handle = ctx.gc_heap_mut().alloc(HeapObject::Cons {
                        head: elem,
                        tail: list,
                    });
                    list = Value::Gc(handle);
                }
                Ok(list)
            }
            _ => Err(type_error(
                "flat_map",
                "first argument",
                "Array or List",
                "Map",
                "flat_map(collection, fn)",
            )),
        },
        other => Err(type_error(
            "flat_map",
            "first argument",
            "Array or List",
            other.type_name(),
            "flat_map(collection, fn)",
        )),
    }
}

/// fold(collection, initial, fn) - Reduce collection to single value
///
/// Callback signature: fn(accumulator, element) - must accept exactly 2 arguments
/// Left fold (foldl) semantics: processes elements in left-to-right order
/// Returns initial value unchanged if collection is empty
/// Works on Arrays and Lists
pub(super) fn builtin_fold(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 3, "fold", "fold(collection, initial, fn)")?;
    let mut acc = args[1].clone();
    let func = &args[2];

    match func {
        Value::Closure(_) | Value::Builtin(_) | Value::JitClosure(_) => {}
        other => {
            return Err(type_error(
                "fold",
                "third argument",
                "Function",
                other.type_name(),
                "fold(collection, initial, fn)",
            ));
        }
    }

    match &args[0] {
        Value::Array(arr) => {
            for (idx, item) in arr.iter().enumerate() {
                acc = invoke_binary_callback(ctx, func, acc, item.clone())
                    .map_err(|e| format!("fold: callback error at index {}: {}", idx, e))?;
            }
            Ok(acc)
        }
        Value::None | Value::EmptyList => Ok(acc),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("fold: malformed list")?;
                for (idx, item) in elements.into_iter().enumerate() {
                    acc = invoke_binary_callback(ctx, func, acc, item)
                        .map_err(|e| format!("fold: callback error at index {}: {}", idx, e))?;
                }
                Ok(acc)
            }
            _ => Err(type_error(
                "fold",
                "first argument",
                "Array or List",
                "Map",
                "fold(collection, initial, fn)",
            )),
        },
        other => Err(type_error(
            "fold",
            "first argument",
            "Array or List",
            other.type_name(),
            "fold(collection, initial, fn)",
        )),
    }
}
