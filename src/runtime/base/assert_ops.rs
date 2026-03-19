use std::rc::Rc;

use crate::runtime::{RuntimeContext, gc, gc::HeapObject, hamt as rc_hamt, value::Value};

use super::helpers::{check_arity_range_ref, check_arity_ref};

/// Structural equality that handles GC cons-list comparison by value rather
/// than by heap identity. Falls back to `PartialEq` for all other types.
///
/// Special case: `Value::None` and `Value::EmptyList` are both valid empty-list
/// sentinels (produced by different code paths) and are treated as equal here.
fn values_equal(ctx: &dyn RuntimeContext, a: &Value, b: &Value) -> bool {
    match (a, b) {
        // Both are empty-list sentinels — treat as equal regardless of variant.
        (Value::None | Value::EmptyList, Value::None | Value::EmptyList) => true,
        (Value::AdtUnit(left), Value::AdtUnit(right)) => left == right,
        (left, right) if left.type_name() == "Adt" && right.type_name() == "Adt" => {
            match (left.as_adt(ctx.gc_heap()), right.as_adt(ctx.gc_heap())) {
                (Some(left_adt), Some(right_adt)) => {
                    if left_adt.constructor() != right_adt.constructor() {
                        return false;
                    }
                    let left_fields = left_adt.fields();
                    let right_fields = right_adt.fields();
                    if left_fields.len() != right_fields.len() {
                        return false;
                    }
                    for i in 0..left_fields.len() {
                        if !values_equal(ctx, &left_fields[i], &right_fields[i]) {
                            return false;
                        }
                    }
                    true
                }
                _ => false,
            }
        }
        (Value::Cons(a_cell), Value::Cons(b_cell)) => {
            values_equal(ctx, &a_cell.head, &b_cell.head)
                && values_equal(ctx, &a_cell.tail, &b_cell.tail)
        }
        (Value::HashMap(a_node), Value::HashMap(b_node)) => rc_hamt::hamt_equal(a_node, b_node),
        (Value::Gc(ha), Value::Gc(hb)) => {
            if ha == hb {
                return true; // Same heap slot — trivially equal.
            }
            let obj_a = ctx.gc_heap().get(*ha).clone();
            let obj_b = ctx.gc_heap().get(*hb).clone();
            match (obj_a, obj_b) {
                (
                    HeapObject::Cons { head: h1, tail: t1 },
                    HeapObject::Cons { head: h2, tail: t2 },
                ) => values_equal(ctx, &h1, &h2) && values_equal(ctx, &t1, &t2),
                (
                    HeapObject::HamtNode { .. } | HeapObject::HamtCollision { .. },
                    HeapObject::HamtNode { .. } | HeapObject::HamtCollision { .. },
                ) => gc::hamt::hamt_equal(ctx.gc_heap(), *ha, *hb),
                _ => false,
            }
        }
        // None == None (empty list tail)
        _ => a == b,
    }
}

/// Formats a Value for display in assertion messages.
/// Falls back to the list_ops formatter for GC objects so users see
/// `[1, 2, 3]` rather than `<gc@N>`.
fn display_value(ctx: &dyn RuntimeContext, v: &Value) -> String {
    match v {
        Value::HashMap(_) | Value::Gc(_) | Value::Tuple(_) | Value::Array(_) | Value::Adt(_) => {
            super::list_ops::format_value(ctx, v)
        }
        _ => format!("{}", v),
    }
}

pub(super) fn base_assert_eq(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_eq_borrowed(ctx, &borrowed)
}

pub(super) fn base_assert_eq_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "assert_eq", "assert_eq(actual, expected)")?;
    if values_equal(ctx, args[0], args[1]) {
        Ok(Value::None)
    } else {
        Err(format!(
            "assert_eq failed\n  expected: {}\n  actual:   {}",
            display_value(ctx, args[1]),
            display_value(ctx, args[0])
        ))
    }
}

pub(super) fn base_assert_neq(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_neq_borrowed(ctx, &borrowed)
}

pub(super) fn base_assert_neq_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "assert_neq", "assert_neq(actual, expected)")?;
    if !values_equal(ctx, args[0], args[1]) {
        Ok(Value::None)
    } else {
        Err(format!(
            "assert_neq failed: both values equal {}",
            display_value(ctx, args[0])
        ))
    }
}

pub(super) fn base_assert_true(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_true_borrowed(_ctx, &borrowed)
}

pub(super) fn base_assert_true_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "assert_true", "assert_true(cond)")?;
    match args[0] {
        Value::Boolean(true) => Ok(Value::None),
        Value::Boolean(false) => Err("assert_true failed: got false".to_string()),
        other => Err(format!(
            "assert_true expected Boolean, got {}",
            other.type_name()
        )),
    }
}

pub(super) fn base_assert_false(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_false_borrowed(_ctx, &borrowed)
}

pub(super) fn base_assert_false_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "assert_false", "assert_false(cond)")?;
    match args[0] {
        Value::Boolean(false) => Ok(Value::None),
        Value::Boolean(true) => Err("assert_false failed: got true".to_string()),
        other => Err(format!(
            "assert_false expected Boolean, got {}",
            other.type_name()
        )),
    }
}

pub(super) fn base_assert_throws(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_throws_borrowed(ctx, &borrowed)
}

pub(super) fn base_assert_throws_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_range_ref(
        args,
        1,
        2,
        "assert_throws",
        "assert_throws(fn) or assert_throws(fn, expected_message)",
    )?;

    let expected_msg: Option<String> = if args.len() == 2 {
        match args[1] {
            Value::String(s) => Some(s.to_string()),
            other => {
                return Err(format!(
                    "assert_throws expected String as second argument, got {}",
                    other.type_name()
                ));
            }
        }
    } else {
        None
    };

    match args[0] {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
        other => {
            return Err(format!(
                "assert_throws expected callable as first argument, got {}",
                other.type_name()
            ));
        }
    }

    match ctx.invoke_value(args[0].clone(), vec![]) {
        Ok(_) => Err("assert_throws failed: function completed without error".to_string()),
        Err(msg) => match &expected_msg {
            Some(expected) if msg.contains(expected.as_str()) => Ok(Value::None),
            Some(expected) => Err(format!(
                "assert_throws failed\n  expected error containing: {}\n  actual error: {}",
                expected, msg
            )),
            None => Ok(Value::None),
        },
    }
}

pub(super) fn base_assert_msg(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_msg_borrowed(_ctx, &borrowed)
}

pub(super) fn base_assert_msg_borrowed(
    _ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 2, "assert_msg", "assert_msg(condition, message)")?;
    match args[0] {
        Value::Boolean(true) => Ok(Value::None),
        Value::Boolean(false) => match args[1] {
            Value::String(s) => Err(format!("assertion failed: {}", s)),
            other => Err(format!("assertion failed: {}", other)),
        },
        other => Err(format!(
            "assert_msg expected Boolean as first argument, got {}",
            other.type_name()
        )),
    }
}

// ---------------------------------------------------------------------------
// Comparison assertions
// ---------------------------------------------------------------------------

fn value_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

fn comparison_assert(
    ctx: &dyn RuntimeContext,
    args: &[&Value],
    name: &str,
    op_symbol: &str,
    cmp: fn(f64, f64) -> bool,
) -> Result<Value, String> {
    check_arity_ref(args, 2, name, &format!("{name}(a, b)"))?;
    let a = value_as_f64(args[0]).ok_or_else(|| {
        format!(
            "{} expected numeric first argument, got {}",
            name,
            args[0].type_name()
        )
    })?;
    let b = value_as_f64(args[1]).ok_or_else(|| {
        format!(
            "{} expected numeric second argument, got {}",
            name,
            args[1].type_name()
        )
    })?;
    if cmp(a, b) {
        Ok(Value::None)
    } else {
        Err(format!(
            "{} failed: expected {} {} {}",
            name,
            display_value(ctx, args[0]),
            op_symbol,
            display_value(ctx, args[1])
        ))
    }
}

pub(super) fn base_assert_gt(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_gt_borrowed(ctx, &borrowed)
}

pub(super) fn base_assert_gt_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    comparison_assert(ctx, args, "assert_gt", ">", |a, b| a > b)
}

pub(super) fn base_assert_lt(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_lt_borrowed(ctx, &borrowed)
}

pub(super) fn base_assert_lt_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    comparison_assert(ctx, args, "assert_lt", "<", |a, b| a < b)
}

pub(super) fn base_assert_gte(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_gte_borrowed(ctx, &borrowed)
}

pub(super) fn base_assert_gte_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    comparison_assert(ctx, args, "assert_gte", ">=", |a, b| a >= b)
}

pub(super) fn base_assert_lte(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_lte_borrowed(ctx, &borrowed)
}

pub(super) fn base_assert_lte_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    comparison_assert(ctx, args, "assert_lte", "<=", |a, b| a <= b)
}

// ---------------------------------------------------------------------------
// Length assertion
// ---------------------------------------------------------------------------

pub(super) fn base_assert_len(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_assert_len_borrowed(ctx, &borrowed)
}

pub(super) fn base_assert_len_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(
        args,
        2,
        "assert_len",
        "assert_len(collection, expected_length)",
    )?;
    let expected = match args[1] {
        Value::Integer(n) => *n,
        other => {
            return Err(format!(
                "assert_len expected Integer as second argument, got {}",
                other.type_name()
            ));
        }
    };
    let actual_val = super::collection_ops::base_len_borrowed(ctx, &[args[0]])?;
    let actual = match actual_val {
        Value::Integer(n) => n,
        _ => return Err("assert_len: could not determine collection length".to_string()),
    };
    if actual == expected {
        Ok(Value::None)
    } else {
        Err(format!(
            "assert_len failed\n  expected length: {}\n  actual length:   {}",
            expected, actual
        ))
    }
}

pub(super) fn base_try(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_try_borrowed(ctx, &borrowed)
}

pub(super) fn base_try_borrowed(
    ctx: &mut dyn RuntimeContext,
    args: &[&Value],
) -> Result<Value, String> {
    check_arity_ref(args, 1, "try", "try(fn)")?;
    match args[0] {
        Value::Closure(_) | Value::BaseFunction(_) | Value::JitClosure(_) => {}
        other => {
            return Err(format!(
                "try expected callable as first argument, got {}",
                other.type_name()
            ));
        }
    }

    match ctx.invoke_value(args[0].clone(), vec![]) {
        Ok(val) => Ok(Value::Tuple(Rc::new(vec![
            Value::String("ok".to_string().into()),
            val,
        ]))),
        Err(msg) => Ok(Value::Tuple(Rc::new(vec![
            Value::String("error".to_string().into()),
            Value::String(Rc::new(msg)),
        ]))),
    }
}
