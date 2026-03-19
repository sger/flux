use std::rc::Rc;

use crate::runtime::{RuntimeContext, cons_cell::ConsCell, gc::HeapObject, value::Value};

use super::helpers::{check_arity, type_error};
use super::list_ops;

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

/// map(collection, fn) - Apply fn to each element, return new collection of results
///
/// Callback signature: fn(element) - must accept exactly 1 argument
/// Elements are processed in left-to-right order
/// Works on Arrays (returns Array) and Lists (returns List)
pub(super) fn base_map(
    ctx: &mut dyn RuntimeContext,
    mut args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "map", "map(collection, fn)")?;

    match &args[1] {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
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

    // Aether fast path: if array is uniquely owned, mutate in-place (no new allocation).
    if matches!(&args[0], Value::Array(arr) if Rc::strong_count(arr) == 1) {
        let func = args.swap_remove(1);
        let arr_rc = match args.swap_remove(0) {
            Value::Array(a) => a,
            _ => unreachable!(),
        };
        if let Ok(mut vec) = Rc::try_unwrap(arr_rc) {
            for (idx, elem) in vec.iter_mut().enumerate() {
                let old = std::mem::replace(elem, Value::None);
                *elem = invoke_unary_callback(ctx, &func, old)
                    .map_err(|e| format!("map: callback error at index {}: {}", idx, e))?;
            }
            return Ok(Value::Array(Rc::new(vec)));
        }
    }

    let func = &args[1];
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
        Value::Cons(_) => {
            let elements = list_ops::collect_list(ctx, &args[0]).ok_or("map: malformed list")?;
            let mut results = Vec::with_capacity(elements.len());
            for (idx, item) in elements.into_iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item)
                    .map_err(|e| format!("map: callback error at index {}: {}", idx, e))?;
                results.push(result);
            }
            // Build a cons list from results
            let mut list = Value::EmptyList;
            for elem in results.into_iter().rev() {
                list = ConsCell::cons(elem, list);
            }
            Ok(list)
        }
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
                let mut list = Value::EmptyList;
                for elem in results.into_iter().rev() {
                    list = ConsCell::cons(elem, list);
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
pub(super) fn base_filter(
    ctx: &mut dyn RuntimeContext,
    mut args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "filter", "filter(collection, pred)")?;

    match &args[1] {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
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

    // Aether fast path: if array is uniquely owned, filter in-place (no new allocation).
    if matches!(&args[0], Value::Array(arr) if Rc::strong_count(arr) == 1) {
        let func = args.swap_remove(1);
        let arr_rc = match args.swap_remove(0) {
            Value::Array(a) => a,
            _ => unreachable!(),
        };
        if let Ok(mut vec) = Rc::try_unwrap(arr_rc) {
            let mut write = 0;
            for read in 0..vec.len() {
                let keep = invoke_unary_callback(ctx, &func, vec[read].clone())
                    .map_err(|e| format!("filter: callback error at index {}: {}", read, e))?
                    .is_truthy();
                if keep {
                    if write != read {
                        vec.swap(write, read);
                    }
                    write += 1;
                }
            }
            vec.truncate(write);
            return Ok(Value::Array(Rc::new(vec)));
        }
    }

    let func = &args[1];
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
        Value::Cons(_) => {
            let elements = list_ops::collect_list(ctx, &args[0]).ok_or("filter: malformed list")?;
            let mut results = Vec::new();
            for (idx, item) in elements.into_iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item.clone())
                    .map_err(|e| format!("filter: callback error at index {}: {}", idx, e))?;
                if result.is_truthy() {
                    results.push(item);
                }
            }
            // Build a cons list from results
            let mut list = Value::EmptyList;
            for elem in results.into_iter().rev() {
                list = ConsCell::cons(elem, list);
            }
            Ok(list)
        }
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
                let mut list = Value::EmptyList;
                for elem in results.into_iter().rev() {
                    list = ConsCell::cons(elem, list);
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
pub(super) fn base_flat_map(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "flat_map", "flat_map(collection, fn)")?;
    let func = &args[1];

    match func {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
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
        Value::Cons(_) => {
            let elements =
                list_ops::collect_list(ctx, &args[0]).ok_or("flat_map: malformed list")?;
            let mut results: Vec<Value> = Vec::new();
            for (idx, item) in elements.into_iter().enumerate() {
                let inner = invoke_unary_callback(ctx, func, item)
                    .map_err(|e| format!("flat_map: callback error at index {}: {}", idx, e))?;
                match inner {
                    Value::None | Value::EmptyList => {}
                    Value::Cons(_) | Value::Gc(_) => {
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
            let mut list = Value::EmptyList;
            for elem in results.into_iter().rev() {
                list = ConsCell::cons(elem, list);
            }
            Ok(list)
        }
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
                        Value::Cons(_) | Value::Gc(_) => {
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
                let mut list = Value::EmptyList;
                for elem in results.into_iter().rev() {
                    list = ConsCell::cons(elem, list);
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

/// any(collection, pred) - Return true if pred returns truthy for any element (short-circuits)
///
/// Callback signature: pred(element) - must accept exactly 1 argument
/// Returns Boolean(false) for empty collections
/// Works on Arrays and Lists
pub(super) fn base_any(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "any", "any(collection, pred)")?;
    let func = &args[1];

    match func {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
        other => {
            return Err(type_error(
                "any",
                "second argument",
                "Function",
                other.type_name(),
                "any(collection, pred)",
            ));
        }
    }

    match &args[0] {
        Value::Array(arr) => {
            for (idx, item) in arr.iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item.clone())
                    .map_err(|e| format!("any: callback error at index {}: {}", idx, e))?;
                if result.is_truthy() {
                    return Ok(Value::Boolean(true));
                }
            }
            Ok(Value::Boolean(false))
        }
        Value::None | Value::EmptyList => Ok(Value::Boolean(false)),
        Value::Cons(_) => {
            let elements = list_ops::collect_list(ctx, &args[0]).ok_or("any: malformed list")?;
            for (idx, item) in elements.into_iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item)
                    .map_err(|e| format!("any: callback error at index {}: {}", idx, e))?;
                if result.is_truthy() {
                    return Ok(Value::Boolean(true));
                }
            }
            Ok(Value::Boolean(false))
        }
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("any: malformed list")?;
                for (idx, item) in elements.into_iter().enumerate() {
                    let result = invoke_unary_callback(ctx, func, item)
                        .map_err(|e| format!("any: callback error at index {}: {}", idx, e))?;
                    if result.is_truthy() {
                        return Ok(Value::Boolean(true));
                    }
                }
                Ok(Value::Boolean(false))
            }
            _ => Err(type_error(
                "any",
                "first argument",
                "Array or List",
                "Map",
                "any(collection, pred)",
            )),
        },
        other => Err(type_error(
            "any",
            "first argument",
            "Array or List",
            other.type_name(),
            "any(collection, pred)",
        )),
    }
}

/// all(collection, pred) - Return true if pred returns truthy for every element (short-circuits)
///
/// Callback signature: pred(element) - must accept exactly 1 argument
/// Returns Boolean(true) for empty collections (vacuous truth)
/// Works on Arrays and Lists
pub(super) fn base_all(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "all", "all(collection, pred)")?;
    let func = &args[1];

    match func {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
        other => {
            return Err(type_error(
                "all",
                "second argument",
                "Function",
                other.type_name(),
                "all(collection, pred)",
            ));
        }
    }

    match &args[0] {
        Value::Array(arr) => {
            for (idx, item) in arr.iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item.clone())
                    .map_err(|e| format!("all: callback error at index {}: {}", idx, e))?;
                if !result.is_truthy() {
                    return Ok(Value::Boolean(false));
                }
            }
            Ok(Value::Boolean(true))
        }
        Value::None | Value::EmptyList => Ok(Value::Boolean(true)),
        Value::Cons(_) => {
            let elements = list_ops::collect_list(ctx, &args[0]).ok_or("all: malformed list")?;
            for (idx, item) in elements.into_iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item)
                    .map_err(|e| format!("all: callback error at index {}: {}", idx, e))?;
                if !result.is_truthy() {
                    return Ok(Value::Boolean(false));
                }
            }
            Ok(Value::Boolean(true))
        }
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("all: malformed list")?;
                for (idx, item) in elements.into_iter().enumerate() {
                    let result = invoke_unary_callback(ctx, func, item)
                        .map_err(|e| format!("all: callback error at index {}: {}", idx, e))?;
                    if !result.is_truthy() {
                        return Ok(Value::Boolean(false));
                    }
                }
                Ok(Value::Boolean(true))
            }
            _ => Err(type_error(
                "all",
                "first argument",
                "Array or List",
                "Map",
                "all(collection, pred)",
            )),
        },
        other => Err(type_error(
            "all",
            "first argument",
            "Array or List",
            other.type_name(),
            "all(collection, pred)",
        )),
    }
}

/// find(collection, pred) - Return Some(first element where pred is truthy), or None
///
/// Callback signature: pred(element) - must accept exactly 1 argument
/// Returns None for empty collections or when no element matches
/// Works on Arrays and Lists
pub(super) fn base_find(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "find", "find(collection, pred)")?;
    let func = &args[1];

    match func {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
        other => {
            return Err(type_error(
                "find",
                "second argument",
                "Function",
                other.type_name(),
                "find(collection, pred)",
            ));
        }
    }

    match &args[0] {
        Value::Array(arr) => {
            for (idx, item) in arr.iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item.clone())
                    .map_err(|e| format!("find: callback error at index {}: {}", idx, e))?;
                if result.is_truthy() {
                    return Ok(Value::Some(Rc::new(item.clone())));
                }
            }
            Ok(Value::None)
        }
        Value::None | Value::EmptyList => Ok(Value::None),
        Value::Cons(_) => {
            let elements = list_ops::collect_list(ctx, &args[0]).ok_or("find: malformed list")?;
            for (idx, item) in elements.into_iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item.clone())
                    .map_err(|e| format!("find: callback error at index {}: {}", idx, e))?;
                if result.is_truthy() {
                    return Ok(Value::Some(Rc::new(item)));
                }
            }
            Ok(Value::None)
        }
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("find: malformed list")?;
                for (idx, item) in elements.into_iter().enumerate() {
                    let result = invoke_unary_callback(ctx, func, item.clone())
                        .map_err(|e| format!("find: callback error at index {}: {}", idx, e))?;
                    if result.is_truthy() {
                        return Ok(Value::Some(Rc::new(item)));
                    }
                }
                Ok(Value::None)
            }
            _ => Err(type_error(
                "find",
                "first argument",
                "Array or List",
                "Map",
                "find(collection, pred)",
            )),
        },
        other => Err(type_error(
            "find",
            "first argument",
            "Array or List",
            other.type_name(),
            "find(collection, pred)",
        )),
    }
}

/// sort_by(arr, key_fn) - Sort array by a derived key
///
/// Callback signature: key_fn(element) -> comparable key
/// The key function must return Integer, Float, or String values
/// Stable sort: equal keys preserve original order
pub(super) fn base_sort_by(
    ctx: &mut dyn RuntimeContext,
    mut args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "sort_by", "sort_by(arr, key_fn)")?;

    let func = args[1].clone();
    match &func {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
        other => {
            return Err(type_error(
                "sort_by",
                "second argument",
                "Function",
                other.type_name(),
                "sort_by(arr, key_fn)",
            ));
        }
    }

    let arr = match args.swap_remove(0) {
        Value::Array(arr) => arr,
        other => {
            return Err(type_error(
                "sort_by",
                "first argument",
                "Array",
                other.type_name(),
                "sort_by(arr, key_fn)",
            ));
        }
    };

    // Pre-compute keys so we only call the user function once per element
    let mut keyed: Vec<(Value, Value)> = arr
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let key = invoke_unary_callback(ctx, &func, item.clone())
                .map_err(|e| format!("sort_by: callback error at index {}: {}", idx, e))?;
            Ok((key, item.clone()))
        })
        .collect::<Result<Vec<_>, String>>()?;

    keyed.sort_by(|(a_key, _), (b_key, _)| {
        use std::cmp::Ordering;
        match (a_key, b_key) {
            (Value::Integer(a), Value::Integer(b)) => a.cmp(b),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Value::Integer(a), Value::Float(b)) => {
                (*a as f64).partial_cmp(b).unwrap_or(Ordering::Equal)
            }
            (Value::Float(a), Value::Integer(b)) => {
                a.partial_cmp(&(*b as f64)).unwrap_or(Ordering::Equal)
            }
            (Value::String(a), Value::String(b)) => a.cmp(b),
            _ => Ordering::Equal,
        }
    });

    let result: Vec<Value> = keyed.into_iter().map(|(_, v)| v).collect();
    Ok(Value::Array(result.into()))
}

/// zip(xs, ys) - Pair elements from two collections into an array of tuples
///
/// Stops at the shorter collection. Returns an empty array if either input is empty.
/// Works on Arrays and Lists (any combination); always returns Array of Tuples.
pub(super) fn base_zip(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "zip", "zip(xs, ys)")?;

    let xs = match &args[0] {
        Value::Array(arr) => arr.to_vec(),
        Value::None | Value::EmptyList => vec![],
        Value::Cons(_) => {
            list_ops::collect_list(ctx, &args[0]).ok_or("zip: malformed first list")?
        }
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                list_ops::collect_list(ctx, &args[0]).ok_or("zip: malformed first list")?
            }
            _ => {
                return Err(type_error(
                    "zip",
                    "first argument",
                    "Array or List",
                    "Map",
                    "zip(xs, ys)",
                ));
            }
        },
        other => {
            return Err(type_error(
                "zip",
                "first argument",
                "Array or List",
                other.type_name(),
                "zip(xs, ys)",
            ));
        }
    };

    let ys = match &args[1] {
        Value::Array(arr) => arr.to_vec(),
        Value::None | Value::EmptyList => vec![],
        Value::Cons(_) => {
            list_ops::collect_list(ctx, &args[1]).ok_or("zip: malformed second list")?
        }
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                list_ops::collect_list(ctx, &args[1]).ok_or("zip: malformed second list")?
            }
            _ => {
                return Err(type_error(
                    "zip",
                    "second argument",
                    "Array or List",
                    "Map",
                    "zip(xs, ys)",
                ));
            }
        },
        other => {
            return Err(type_error(
                "zip",
                "second argument",
                "Array or List",
                other.type_name(),
                "zip(xs, ys)",
            ));
        }
    };

    let result: Vec<Value> = xs
        .into_iter()
        .zip(ys)
        .map(|(x, y)| Value::Tuple(Rc::new(vec![x, y])))
        .collect();
    Ok(Value::Array(result.into()))
}

/// flatten(collection) - Flatten one level of nesting
///
/// For arrays of arrays: returns a single flat array.
/// For lists of lists: returns a single flat array.
/// Equivalent to flat_map(xs, \x -> x) but more readable.
pub(super) fn base_flatten(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "flatten", "flatten(collection)")?;

    match &args[0] {
        Value::Array(outer) => {
            let mut result = Vec::new();
            for (idx, item) in outer.iter().enumerate() {
                match item {
                    Value::Array(inner) => result.extend(inner.iter().cloned()),
                    Value::None | Value::EmptyList => {}
                    Value::Cons(_) => {
                        let elems = list_ops::collect_list(ctx, item)
                            .ok_or_else(|| format!("flatten: malformed list at index {}", idx))?;
                        result.extend(elems);
                    }
                    Value::Gc(h) => match ctx.gc_heap().get(*h) {
                        HeapObject::Cons { .. } => {
                            let elems = list_ops::collect_list(ctx, item).ok_or_else(|| {
                                format!("flatten: malformed list at index {}", idx)
                            })?;
                            result.extend(elems);
                        }
                        _ => {
                            return Err(format!(
                                "flatten: element at index {} must be Array or List, got Map",
                                idx
                            ));
                        }
                    },
                    other => {
                        return Err(format!(
                            "flatten: element at index {} must be Array or List, got {}",
                            idx,
                            other.type_name()
                        ));
                    }
                }
            }
            Ok(Value::Array(result.into()))
        }
        Value::None | Value::EmptyList => Ok(Value::Array(vec![].into())),
        Value::Cons(_) => {
            let outer = list_ops::collect_list(ctx, &args[0]).ok_or("flatten: malformed list")?;
            let mut result = Vec::new();
            for (idx, item) in outer.into_iter().enumerate() {
                match &item {
                    Value::Array(inner) => result.extend(inner.iter().cloned()),
                    Value::None | Value::EmptyList => {}
                    Value::Cons(_) => {
                        let elems = list_ops::collect_list(ctx, &item).ok_or_else(|| {
                            format!("flatten: malformed inner list at index {}", idx)
                        })?;
                        result.extend(elems);
                    }
                    Value::Gc(h) => match ctx.gc_heap().get(*h) {
                        HeapObject::Cons { .. } => {
                            let elems = list_ops::collect_list(ctx, &item).ok_or_else(|| {
                                format!("flatten: malformed inner list at index {}", idx)
                            })?;
                            result.extend(elems);
                        }
                        _ => {
                            return Err(format!(
                                "flatten: element at index {} must be Array or List, got Map",
                                idx
                            ));
                        }
                    },
                    other => {
                        return Err(format!(
                            "flatten: element at index {} must be Array or List, got {}",
                            idx,
                            other.type_name()
                        ));
                    }
                }
            }
            Ok(Value::Array(result.into()))
        }
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let outer =
                    list_ops::collect_list(ctx, &args[0]).ok_or("flatten: malformed list")?;
                let mut result = Vec::new();
                for (idx, item) in outer.into_iter().enumerate() {
                    match &item {
                        Value::Array(inner) => result.extend(inner.iter().cloned()),
                        Value::None | Value::EmptyList => {}
                        Value::Cons(_) => {
                            let elems = list_ops::collect_list(ctx, &item).ok_or_else(|| {
                                format!("flatten: malformed inner list at index {}", idx)
                            })?;
                            result.extend(elems);
                        }
                        Value::Gc(h) => match ctx.gc_heap().get(*h) {
                            HeapObject::Cons { .. } => {
                                let elems =
                                    list_ops::collect_list(ctx, &item).ok_or_else(|| {
                                        format!("flatten: malformed inner list at index {}", idx)
                                    })?;
                                result.extend(elems);
                            }
                            _ => {
                                return Err(format!(
                                    "flatten: element at index {} must be Array or List, got Map",
                                    idx
                                ));
                            }
                        },
                        other => {
                            return Err(format!(
                                "flatten: element at index {} must be Array or List, got {}",
                                idx,
                                other.type_name()
                            ));
                        }
                    }
                }
                Ok(Value::Array(result.into()))
            }
            _ => Err(type_error(
                "flatten",
                "argument",
                "Array or List",
                "Map",
                "flatten(collection)",
            )),
        },
        other => Err(type_error(
            "flatten",
            "argument",
            "Array or List",
            other.type_name(),
            "flatten(collection)",
        )),
    }
}

/// count(collection, pred) - Count elements where pred returns truthy
///
/// Callback signature: pred(element) - must accept exactly 1 argument
/// More efficient than len(filter(...)) — no intermediate allocation.
/// Works on Arrays and Lists
pub(super) fn base_count(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 2, "count", "count(collection, pred)")?;
    let func = &args[1];

    match func {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
        other => {
            return Err(type_error(
                "count",
                "second argument",
                "Function",
                other.type_name(),
                "count(collection, pred)",
            ));
        }
    }

    match &args[0] {
        Value::Array(arr) => {
            let mut n: i64 = 0;
            for (idx, item) in arr.iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item.clone())
                    .map_err(|e| format!("count: callback error at index {}: {}", idx, e))?;
                if result.is_truthy() {
                    n += 1;
                }
            }
            Ok(Value::Integer(n))
        }
        Value::None | Value::EmptyList => Ok(Value::Integer(0)),
        Value::Cons(_) => {
            let elements = list_ops::collect_list(ctx, &args[0]).ok_or("count: malformed list")?;
            let mut n: i64 = 0;
            for (idx, item) in elements.into_iter().enumerate() {
                let result = invoke_unary_callback(ctx, func, item)
                    .map_err(|e| format!("count: callback error at index {}: {}", idx, e))?;
                if result.is_truthy() {
                    n += 1;
                }
            }
            Ok(Value::Integer(n))
        }
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => {
                let elements =
                    list_ops::collect_list(ctx, &args[0]).ok_or("count: malformed list")?;
                let mut n: i64 = 0;
                for (idx, item) in elements.into_iter().enumerate() {
                    let result = invoke_unary_callback(ctx, func, item)
                        .map_err(|e| format!("count: callback error at index {}: {}", idx, e))?;
                    if result.is_truthy() {
                        n += 1;
                    }
                }
                Ok(Value::Integer(n))
            }
            _ => Err(type_error(
                "count",
                "first argument",
                "Array or List",
                "Map",
                "count(collection, pred)",
            )),
        },
        other => Err(type_error(
            "count",
            "first argument",
            "Array or List",
            other.type_name(),
            "count(collection, pred)",
        )),
    }
}

/// fold(collection, initial, fn) - Reduce collection to single value
///
/// Callback signature: fn(accumulator, element) - must accept exactly 2 arguments
/// Left fold (foldl) semantics: processes elements in left-to-right order
/// Returns initial value unchanged if collection is empty
/// Works on Arrays and Lists
pub(super) fn base_fold(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 3, "fold", "fold(collection, initial, fn)")?;
    let mut acc = args[1].clone();
    let func = &args[2];

    match func {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
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
        Value::Cons(_) => {
            let elements = list_ops::collect_list(ctx, &args[0]).ok_or("fold: malformed list")?;
            for (idx, item) in elements.into_iter().enumerate() {
                acc = invoke_binary_callback(ctx, func, acc, item)
                    .map_err(|e| format!("fold: callback error at index {}: {}", idx, e))?;
            }
            Ok(acc)
        }
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
