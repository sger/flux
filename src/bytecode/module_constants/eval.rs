//! Compile-time evaluation of constant expressions.

use std::collections::HashMap;


use crate::{
    syntax::{expression::Expression, interner::Interner, symbol::Symbol},
    runtime::{hash_key::HashKey, object::Object},
};

use super::error::ConstEvalError;

/// Evaluate a constant expression at compile time.
///
/// Supports literals, basic operations, arrays, hashes, and references
/// to already-evaluated constants.
pub fn eval_const_expr(
    expr: &Expression,
    defined: &HashMap<Symbol, Object>,
    interner: &Interner,
) -> Result<Object, ConstEvalError> {
    match expr {
        Expression::Integer { value, .. } => Ok(Object::Integer(*value)),
        Expression::Float { value, .. } => Ok(Object::Float(*value)),
        Expression::String { value, .. } => Ok(Object::String(value.clone())),
        Expression::Boolean { value, .. } => Ok(Object::Boolean(*value)),
        Expression::None { .. } => Ok(Object::None),

        Expression::Some { value, .. } => {
            let inner = eval_const_expr(value, defined, interner)?;
            Ok(Object::Some(Box::new(inner)))
        }

        Expression::Array { elements, .. } => {
            let mut values = Vec::with_capacity(elements.len());
            for element in elements {
                values.push(eval_const_expr(element, defined, interner)?);
            }
            Ok(Object::Array(values))
        }

        Expression::Hash { pairs, .. } => {
            let mut map = HashMap::with_capacity(pairs.len());
            for (key, value) in pairs {
                let k = eval_const_expr(key, defined, interner)?;
                let v = eval_const_expr(value, defined, interner)?;

                let hash_key = match &k {
                    Object::Integer(i) => HashKey::Integer(*i),
                    Object::Boolean(b) => HashKey::Boolean(*b),
                    Object::String(s) => HashKey::String(s.clone()),
                    _ => {
                        return Err(ConstEvalError::new(
                            "E040",
                            "Hash keys must be integers, booleans, or strings.",
                        ));
                    }
                };
                map.insert(hash_key, v);
            }
            Ok(Object::Hash(map))
        }

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

fn eval_const_unary_op(op: &str, right: &Object) -> Result<Object, ConstEvalError> {
    match (op, right) {
        ("-", Object::Integer(i)) => Ok(Object::Integer(-i)),
        ("-", Object::Float(f)) => Ok(Object::Float(-f)),
        ("!", Object::Boolean(b)) => Ok(Object::Boolean(!b)),
        _ => Err(ConstEvalError::new(
            "E043",
            format!("Cannot apply '{}' to {:?} at compile time.", op, right),
        )),
    }
}

fn eval_const_binary_op(left: &Object, op: &str, right: &Object) -> Result<Object, ConstEvalError> {
    match (left, op, right) {
        // Integer arithmetic
        (Object::Integer(a), "+", Object::Integer(b)) => Ok(Object::Integer(a + b)),
        (Object::Integer(a), "-", Object::Integer(b)) => Ok(Object::Integer(a - b)),
        (Object::Integer(a), "*", Object::Integer(b)) => Ok(Object::Integer(a * b)),
        (Object::Integer(_), "/", Object::Integer(0)) => Err(ConstEvalError::new(
            "E059",
            "Division by zero in module constant.",
        )),
        (Object::Integer(a), "/", Object::Integer(b)) => Ok(Object::Integer(a / b)),
        (Object::Integer(_), "%", Object::Integer(0)) => Err(ConstEvalError::new(
            "E059",
            "Modulo by zero in module constant.",
        )),
        (Object::Integer(a), "%", Object::Integer(b)) => Ok(Object::Integer(a % b)),

        // Float arithmetic
        (Object::Float(a), "+", Object::Float(b)) => Ok(Object::Float(a + b)),
        (Object::Float(a), "-", Object::Float(b)) => Ok(Object::Float(a - b)),
        (Object::Float(a), "*", Object::Float(b)) => Ok(Object::Float(a * b)),
        (Object::Float(_), "/", Object::Float(b)) if *b == 0.0 => Err(ConstEvalError::new(
            "E059",
            "Division by zero in module constant.",
        )),
        (Object::Float(a), "/", Object::Float(b)) => Ok(Object::Float(a / b)),

        // Mixed numeric - promote to float
        (Object::Integer(i), op, Object::Float(_)) => {
            eval_const_binary_op(&Object::Float(*i as f64), op, right)
        }
        (Object::Float(_), op, Object::Integer(i)) => {
            eval_const_binary_op(left, op, &Object::Float(*i as f64))
        }

        // String concatenation
        (Object::String(a), "+", Object::String(b)) => Ok(Object::String(format!("{}{}", a, b))),

        // Boolean operations
        (Object::Boolean(a), "&&", Object::Boolean(b)) => Ok(Object::Boolean(*a && *b)),
        (Object::Boolean(a), "||", Object::Boolean(b)) => Ok(Object::Boolean(*a || *b)),

        // Integer comparisons
        (Object::Integer(a), "==", Object::Integer(b)) => Ok(Object::Boolean(a == b)),
        (Object::Integer(a), "!=", Object::Integer(b)) => Ok(Object::Boolean(a != b)),
        (Object::Integer(a), "<", Object::Integer(b)) => Ok(Object::Boolean(a < b)),
        (Object::Integer(a), ">", Object::Integer(b)) => Ok(Object::Boolean(a > b)),
        (Object::Integer(a), "<=", Object::Integer(b)) => Ok(Object::Boolean(a <= b)),
        (Object::Integer(a), ">=", Object::Integer(b)) => Ok(Object::Boolean(a >= b)),

        // Float comparisons
        (Object::Float(a), "==", Object::Float(b)) => Ok(Object::Boolean(a == b)),
        (Object::Float(a), "!=", Object::Float(b)) => Ok(Object::Boolean(a != b)),
        (Object::Float(a), "<", Object::Float(b)) => Ok(Object::Boolean(a < b)),
        (Object::Float(a), ">", Object::Float(b)) => Ok(Object::Boolean(a > b)),
        (Object::Float(a), "<=", Object::Float(b)) => Ok(Object::Boolean(a <= b)),
        (Object::Float(a), ">=", Object::Float(b)) => Ok(Object::Boolean(a >= b)),

        // String comparisons
        (Object::String(a), "==", Object::String(b)) => Ok(Object::Boolean(a == b)),
        (Object::String(a), "!=", Object::String(b)) => Ok(Object::Boolean(a != b)),
        (Object::String(a), "<", Object::String(b)) => Ok(Object::Boolean(a < b)),
        (Object::String(a), ">", Object::String(b)) => Ok(Object::Boolean(a > b)),
        (Object::String(a), "<=", Object::String(b)) => Ok(Object::Boolean(a <= b)),
        (Object::String(a), ">=", Object::String(b)) => Ok(Object::Boolean(a >= b)),

        // Boolean comparisons
        (Object::Boolean(a), "==", Object::Boolean(b)) => Ok(Object::Boolean(a == b)),
        (Object::Boolean(a), "!=", Object::Boolean(b)) => Ok(Object::Boolean(a != b)),

        _ => Err(ConstEvalError::new(
            "E048",
            format!(
                "Cannot apply '{}' to {:?} and {:?} at compile time.",
                op, left, right
            ),
        )),
    }
}
