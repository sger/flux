use std::rc::Rc;

use crate::runtime::{
    RuntimeContext,
    gc::{
        HeapObject,
        hamt::{format_hamt, is_hamt},
    },
    value::Value,
};

use super::helpers::{check_arity, type_error};

/// hd(list) - Returns the head (first element) of a cons list.
pub(super) fn builtin_hd(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 1, "hd", "hd(list)")?;
    match &args[0] {
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { head, .. } => Ok(head.clone()),
            _ => Err(type_error(
                "hd",
                "argument",
                "List",
                args[0].type_name(),
                "hd(list)",
            )),
        },
        _ => Err(type_error(
            "hd",
            "argument",
            "List",
            args[0].type_name(),
            "hd(list)",
        )),
    }
}

/// tl(list) - Returns the tail of a cons list.
pub(super) fn builtin_tl(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    check_arity(&args, 1, "tl", "tl(list)")?;
    match &args[0] {
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { tail, .. } => Ok(tail.clone()),
            _ => Err(type_error(
                "tl",
                "argument",
                "List",
                args[0].type_name(),
                "tl(list)",
            )),
        },
        _ => Err(type_error(
            "tl",
            "argument",
            "List",
            args[0].type_name(),
            "tl(list)",
        )),
    }
}

/// is_list(x) - Returns true if x is a cons list or empty list (None).
pub(super) fn builtin_is_list(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "is_list", "is_list(x)")?;
    let result = match &args[0] {
        Value::None => true,
        Value::Gc(h) => matches!(ctx.gc_heap().get(*h), HeapObject::Cons { .. }),
        _ => false,
    };
    Ok(Value::Boolean(result))
}

/// to_list(arr) - Converts an array to a cons list.
pub(super) fn builtin_to_list(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "to_list", "to_list(arr)")?;
    match &args[0] {
        Value::Array(arr) => {
            let mut list = Value::None;
            for elem in arr.iter().rev() {
                let handle = ctx.gc_heap_mut().alloc(HeapObject::Cons {
                    head: elem.clone(),
                    tail: list,
                });
                list = Value::Gc(handle);
            }
            Ok(list)
        }
        _ => Err(type_error(
            "to_list",
            "argument",
            "Array",
            args[0].type_name(),
            "to_list(arr)",
        )),
    }
}

/// to_array(list) - Converts a cons list to an array.
pub(super) fn builtin_to_array(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "to_array", "to_array(list)")?;
    let mut elements = Vec::new();
    let mut current = args[0].clone();
    loop {
        match &current {
            Value::None => break,
            Value::Gc(h) => match ctx.gc_heap().get(*h) {
                HeapObject::Cons { head, tail } => {
                    elements.push(head.clone());
                    current = tail.clone();
                }
                _ => {
                    return Err(type_error(
                        "to_array",
                        "argument",
                        "List",
                        "non-list Gc value",
                        "to_array(list)",
                    ));
                }
            },
            _ => {
                return Err(type_error(
                    "to_array",
                    "argument",
                    "List",
                    current.type_name(),
                    "to_array(list)",
                ));
            }
        }
    }
    Ok(Value::Array(Rc::new(elements)))
}

/// list(...) - Varargs constructor that builds a cons list from arguments.
/// list(1, 2, 3) → [1 | [2 | [3 | None]]]
pub(super) fn builtin_list(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let mut list = Value::None;
    for elem in args.into_iter().rev() {
        let handle = ctx.gc_heap_mut().alloc(HeapObject::Cons {
            head: elem,
            tail: list,
        });
        list = Value::Gc(handle);
    }
    Ok(list)
}

/// Helper: collects a cons list into a Vec for internal use by builtins.
pub(super) fn collect_list(ctx: &dyn RuntimeContext, value: &Value) -> Option<Vec<Value>> {
    let mut elements = Vec::new();
    let mut current = value.clone();
    loop {
        match &current {
            Value::None => return Some(elements),
            Value::Gc(h) => match ctx.gc_heap().get(*h) {
                HeapObject::Cons { head, tail } => {
                    elements.push(head.clone());
                    current = tail.clone();
                }
                _ => return None,
            },
            _ => return None,
        }
    }
}

/// Helper: counts the length of a cons list.
pub(super) fn list_len(ctx: &dyn RuntimeContext, value: &Value) -> Option<usize> {
    let mut count = 0;
    let mut current = value.clone();
    loop {
        match &current {
            Value::None => return Some(count),
            Value::Gc(h) => match ctx.gc_heap().get(*h) {
                HeapObject::Cons { tail, .. } => {
                    count += 1;
                    current = tail.clone();
                }
                _ => return None,
            },
            _ => return None,
        }
    }
}

/// Helper: formats a cons list as "[1, 2, 3]".
pub(super) fn format_list(ctx: &dyn RuntimeContext, value: &Value) -> Option<String> {
    let elements = collect_list(ctx, value)?;
    let items: Vec<String> = elements.iter().map(|e| format_value(ctx, e)).collect();
    Some(format!("[{}]", items.join(", ")))
}

/// Format a value with GC-aware display.
pub fn format_value(ctx: &dyn RuntimeContext, value: &Value) -> String {
    match value {
        Value::Gc(h) => {
            if is_hamt(ctx.gc_heap(), *h) {
                format_hamt(ctx.gc_heap(), *h)
            } else {
                match ctx.gc_heap().get(*h) {
                    HeapObject::Cons { .. } => {
                        format_list(ctx, value).unwrap_or_else(|| format!("<gc@{}>", h.index()))
                    }
                    _ => format!("<gc@{}>", h.index()),
                }
            }
        }
        _ => value.to_string(),
    }
}
