use std::rc::Rc;

use crate::runtime::{RuntimeContext, value::Value};

use super::helpers::{arg_string, check_arity_range, format_hint, type_error};

/// sort(arr) or sort(arr, order) - Return a new sorted array
/// order: "asc" (default) or "desc"
/// Only works with integers/floats
pub(super) fn base_sort(
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
