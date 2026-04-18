//! Compile-time evaluation of constant expressions.

use std::collections::HashMap;

use crate::{
    runtime::value::Value,
    syntax::{expression::Expression, interner::Interner, symbol::Symbol},
};

use super::error::ConstEvalError;

/// Evaluate a constant expression at compile time.
///
/// Supports literals, basic operations, arrays, hashes, and references
/// to already-evaluated constants.
pub fn eval_const_expr(
    expr: &Expression,
    defined: &HashMap<Symbol, Value>,
    interner: &Interner,
) -> Result<Value, ConstEvalError> {
    match expr {
        Expression::Integer { value, .. } => Ok(Value::Integer(*value)),
        Expression::Float { value, .. } => Ok(Value::Float(*value)),
        Expression::String { value, .. } => Ok(Value::String(value.clone().into())),
        Expression::Boolean { value, .. } => Ok(Value::Boolean(*value)),
        Expression::None { .. } => Ok(Value::None),

        Expression::Some { value, .. } => {
            let inner = eval_const_expr(value, defined, interner)?;
            Ok(Value::Some(std::rc::Rc::new(inner)))
        }

        Expression::ArrayLiteral { elements, .. } => {
            let mut values = Vec::with_capacity(elements.len());
            for element in elements {
                values.push(eval_const_expr(element, defined, interner)?);
            }
            Ok(Value::Array(values.into()))
        }

        Expression::EmptyList { .. } | Expression::ListLiteral { .. } => Err(ConstEvalError::new(
            "E040",
            "List literals are not supported in module constants.",
        )
        .with_hint(
            "List literals allocate runtime cons cells; use array literals (#[...]) in constants.",
        )),

        Expression::Hash { .. } => Err(ConstEvalError::new(
            "E040",
            "Hash literals are not supported in module constants.",
        )
        .with_hint(
            "Hash maps require the runtime GC heap and cannot be evaluated at compile time.",
        )),

        Expression::Identifier { name, .. } => defined.get(name).cloned().ok_or_else(|| {
            let name_str = interner.resolve(*name);
            ConstEvalError::new("E041", format!("'{}' is not a module constant.", name_str))
        }),

        Expression::Prefix {
            operator, right, ..
        } => {
            let r = eval_const_expr(right, defined, interner)?;
            eval_const_unary_op(operator, &r)
        }

        Expression::Infix {
            left,
            operator,
            right,
            ..
        } => {
            let l = eval_const_expr(left, defined, interner)?;
            let r = eval_const_expr(right, defined, interner)?;
            eval_const_binary_op(&l, operator, &r)
        }

        _ => Err(ConstEvalError::new(
            "E042",
            "Only literals, basic operations, and references to module constants are allowed.",
        )
        .with_hint("Module constants must be evaluable at compile time.")),
    }
}

fn eval_const_unary_op(op: &str, right: &Value) -> Result<Value, ConstEvalError> {
    match (op, right) {
        ("-", Value::Integer(i)) => Ok(Value::Integer(-i)),
        ("-", Value::Float(f)) => Ok(Value::Float(-f)),
        ("!", Value::Boolean(b)) => Ok(Value::Boolean(!b)),
        _ => Err(ConstEvalError::new(
            "E043",
            format!("Cannot apply '{}' to {:?} at compile time.", op, right),
        )),
    }
}

fn eval_const_binary_op(left: &Value, op: &str, right: &Value) -> Result<Value, ConstEvalError> {
    match (left, op, right) {
        // Integer arithmetic
        (Value::Integer(a), "+", Value::Integer(b)) => Ok(Value::Integer(a + b)),
        (Value::Integer(a), "-", Value::Integer(b)) => Ok(Value::Integer(a - b)),
        (Value::Integer(a), "*", Value::Integer(b)) => Ok(Value::Integer(a * b)),
        (Value::Integer(_), "/", Value::Integer(0)) => Err(ConstEvalError::new(
            "E059",
            "Division by zero in module constant.",
        )),
        (Value::Integer(a), "/", Value::Integer(b)) => Ok(Value::Integer(a / b)),
        (Value::Integer(_), "%", Value::Integer(0)) => Err(ConstEvalError::new(
            "E059",
            "Modulo by zero in module constant.",
        )),
        (Value::Integer(a), "%", Value::Integer(b)) => Ok(Value::Integer(a % b)),

        // Float arithmetic
        (Value::Float(a), "+", Value::Float(b)) => Ok(Value::Float(a + b)),
        (Value::Float(a), "-", Value::Float(b)) => Ok(Value::Float(a - b)),
        (Value::Float(a), "*", Value::Float(b)) => Ok(Value::Float(a * b)),
        (Value::Float(_), "/", Value::Float(b)) if *b == 0.0 => Err(ConstEvalError::new(
            "E059",
            "Division by zero in module constant.",
        )),
        (Value::Float(a), "/", Value::Float(b)) => Ok(Value::Float(a / b)),

        // Mixed numeric - promote to float
        (Value::Integer(i), op, Value::Float(_)) => {
            eval_const_binary_op(&Value::Float(*i as f64), op, right)
        }
        (Value::Float(_), op, Value::Integer(i)) => {
            eval_const_binary_op(left, op, &Value::Float(*i as f64))
        }

        // String concatenation
        (Value::String(a), "+", Value::String(b)) => {
            Ok(Value::String(format!("{}{}", a, b).into()))
        }

        // Boolean operations
        (Value::Boolean(a), "&&", Value::Boolean(b)) => Ok(Value::Boolean(*a && *b)),
        (Value::Boolean(a), "||", Value::Boolean(b)) => Ok(Value::Boolean(*a || *b)),

        // Integer comparisons
        (Value::Integer(a), "==", Value::Integer(b)) => Ok(Value::Boolean(a == b)),
        (Value::Integer(a), "!=", Value::Integer(b)) => Ok(Value::Boolean(a != b)),
        (Value::Integer(a), "<", Value::Integer(b)) => Ok(Value::Boolean(a < b)),
        (Value::Integer(a), ">", Value::Integer(b)) => Ok(Value::Boolean(a > b)),
        (Value::Integer(a), "<=", Value::Integer(b)) => Ok(Value::Boolean(a <= b)),
        (Value::Integer(a), ">=", Value::Integer(b)) => Ok(Value::Boolean(a >= b)),

        // Float comparisons
        (Value::Float(a), "==", Value::Float(b)) => Ok(Value::Boolean(a == b)),
        (Value::Float(a), "!=", Value::Float(b)) => Ok(Value::Boolean(a != b)),
        (Value::Float(a), "<", Value::Float(b)) => Ok(Value::Boolean(a < b)),
        (Value::Float(a), ">", Value::Float(b)) => Ok(Value::Boolean(a > b)),
        (Value::Float(a), "<=", Value::Float(b)) => Ok(Value::Boolean(a <= b)),
        (Value::Float(a), ">=", Value::Float(b)) => Ok(Value::Boolean(a >= b)),

        // String comparisons
        (Value::String(a), "==", Value::String(b)) => Ok(Value::Boolean(a == b)),
        (Value::String(a), "!=", Value::String(b)) => Ok(Value::Boolean(a != b)),
        (Value::String(a), "<", Value::String(b)) => Ok(Value::Boolean(a < b)),
        (Value::String(a), ">", Value::String(b)) => Ok(Value::Boolean(a > b)),
        (Value::String(a), "<=", Value::String(b)) => Ok(Value::Boolean(a <= b)),
        (Value::String(a), ">=", Value::String(b)) => Ok(Value::Boolean(a >= b)),

        // Boolean comparisons
        (Value::Boolean(a), "==", Value::Boolean(b)) => Ok(Value::Boolean(a == b)),
        (Value::Boolean(a), "!=", Value::Boolean(b)) => Ok(Value::Boolean(a != b)),

        _ => Err(ConstEvalError::new(
            "E048",
            format!(
                "Cannot apply '{}' to {:?} and {:?} at compile time.",
                op, left, right
            ),
        )),
    }
}
