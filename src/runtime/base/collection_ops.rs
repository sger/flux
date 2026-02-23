use std::rc::Rc;

use crate::runtime::{
    RuntimeContext,
    gc::{HeapObject, hamt::hamt_len},
    value::Value,
};

use super::helpers::{arg_array, arg_int, check_arity, format_hint, type_error};
use super::list_ops;

pub(super) fn base_len(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
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

pub(super) fn base_first(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
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

pub(super) fn base_last(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
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

pub(super) fn base_rest(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
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

pub(super) fn base_push(
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

pub(super) fn base_concat(
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

pub(super) fn base_reverse(
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

pub(super) fn base_contains(
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

pub(super) fn base_slice(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
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
pub(super) fn base_range(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
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
pub(super) fn base_sum(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
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
pub(super) fn base_product(
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
