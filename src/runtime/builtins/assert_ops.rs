use crate::runtime::{RuntimeContext, gc::HeapObject, value::Value};

use super::helpers::check_arity;

/// Structural equality that handles GC cons-list comparison by value rather
/// than by heap identity. Falls back to `PartialEq` for all other types.
///
/// Special case: `Value::None` and `Value::EmptyList` are both valid empty-list
/// sentinels (produced by different code paths) and are treated as equal here.
fn values_equal(ctx: &dyn RuntimeContext, a: &Value, b: &Value) -> bool {
    match (a, b) {
        // Both are empty-list sentinels — treat as equal regardless of variant.
        (Value::None | Value::EmptyList, Value::None | Value::EmptyList) => true,
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
                // For HAMT maps fall back to identity comparison.
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
        Value::Gc(_) | Value::Tuple(_) | Value::Array(_) => super::list_ops::format_value(ctx, v),
        _ => format!("{}", v),
    }
}

pub(super) fn builtin_assert_eq(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "assert_eq", "assert_eq(actual, expected)")?;
    if values_equal(ctx, &args[0], &args[1]) {
        Ok(Value::None)
    } else {
        Err(format!(
            "assert_eq failed\n  expected: {}\n  actual:   {}",
            display_value(ctx, &args[1]),
            display_value(ctx, &args[0])
        ))
    }
}

pub(super) fn builtin_assert_neq(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "assert_neq", "assert_neq(actual, expected)")?;
    if !values_equal(ctx, &args[0], &args[1]) {
        Ok(Value::None)
    } else {
        Err(format!(
            "assert_neq failed: both values equal {}",
            display_value(ctx, &args[0])
        ))
    }
}

pub(super) fn builtin_assert_true(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "assert_true", "assert_true(cond)")?;
    match &args[0] {
        Value::Boolean(true) => Ok(Value::None),
        Value::Boolean(false) => Err("assert_true failed: got false".to_string()),
        other => Err(format!(
            "assert_true expected Boolean, got {}",
            other.type_name()
        )),
    }
}

pub(super) fn builtin_assert_false(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "assert_false", "assert_false(cond)")?;
    match &args[0] {
        Value::Boolean(false) => Ok(Value::None),
        Value::Boolean(true) => Err("assert_false failed: got true".to_string()),
        other => Err(format!(
            "assert_false expected Boolean, got {}",
            other.type_name()
        )),
    }
}

pub(super) fn builtin_assert_throws(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(
        &args,
        2,
        "assert_throws",
        "assert_throws(fn, expected_message)",
    )?;

    let expected = match &args[1] {
        Value::String(s) => s.to_string(),
        other => {
            return Err(format!(
                "assert_throws expected String as second argument, got {}",
                other.type_name()
            ));
        }
    };

    match &args[0] {
        Value::Closure(_) | Value::Builtin(_) | Value::JitClosure(_) => {}
        other => {
            return Err(format!(
                "assert_throws expected callable as first argument, got {}",
                other.type_name()
            ));
        }
    }

    match ctx.invoke_value(args[0].clone(), vec![]) {
        Ok(_) => Err("assert_throws failed: function completed without error".to_string()),
        Err(msg) => {
            if msg.contains(&expected) {
                Ok(Value::None)
            } else {
                Err(format!(
                    "assert_throws failed\n  expected error containing: {}\n  actual error: {}",
                    expected, msg
                ))
            }
        }
    }
}
