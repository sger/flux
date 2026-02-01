//! Compile-time evaluation of constant expressions.

use std::collections::HashMap;

use crate::{
    frontend::expression::Expression,
    runtime::{hash_key::HashKey, object::Object},
};

use super::error::ConstEvalError;

/// Evaluate a constant expression at compile time.
///
/// Supports literals, basic operations, arrays, hashes, and references
/// to already-evaluated constants.
pub fn eval_const_expr(
    expr: &Expression,
    defined: &HashMap<String, Object>,
) -> Result<Object, ConstEvalError> {
    match expr {
        Expression::Integer { value, .. } => Ok(Object::Integer(*value)),
        Expression::Float { value, .. } => Ok(Object::Float(*value)),
        Expression::String { value, .. } => Ok(Object::String(value.clone())),
        Expression::Boolean { value, .. } => Ok(Object::Boolean(*value)),
        Expression::None { .. } => Ok(Object::None),

        Expression::Some { value, .. } => {
            let inner = eval_const_expr(value, defined)?;
            Ok(Object::Some(Box::new(inner)))
        }

        Expression::Array { elements, .. } => {
            let mut values = Vec::with_capacity(elements.len());
            for element in elements {
                values.push(eval_const_expr(element, defined)?);
            }
            Ok(Object::Array(values))
        }

        Expression::Hash { pairs, .. } => {
            let mut map = HashMap::with_capacity(pairs.len());
            for (key, value) in pairs {
                let k = eval_const_expr(key, defined)?;
                let v = eval_const_expr(value, defined)?;

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
            ConstEvalError::new("E041", format!("'{}' is not a module constant.", name))
        }),

        Expression::Prefix {
            operator, right, ..
        } => {
            let r = eval_const_expr(right, defined)?;
            eval_const_unary_op(operator, &r)
        }

        Expression::Infix {
            left,
            operator,
            right,
            ..
        } => {
            let l = eval_const_expr(left, defined)?;
            let r = eval_const_expr(right, defined)?;
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
            "E046",
            "Division by zero in module constant.",
        )),
        (Object::Integer(a), "/", Object::Integer(b)) => Ok(Object::Integer(a / b)),
        (Object::Integer(_), "%", Object::Integer(0)) => Err(ConstEvalError::new(
            "E046",
            "Modulo by zero in module constant.",
        )),
        (Object::Integer(a), "%", Object::Integer(b)) => Ok(Object::Integer(a % b)),

        // Float arithmetic
        (Object::Float(a), "+", Object::Float(b)) => Ok(Object::Float(a + b)),
        (Object::Float(a), "-", Object::Float(b)) => Ok(Object::Float(a - b)),
        (Object::Float(a), "*", Object::Float(b)) => Ok(Object::Float(a * b)),
        (Object::Float(_), "/", Object::Float(b)) if *b == 0.0 => Err(ConstEvalError::new(
            "E046",
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
            "E044",
            format!(
                "Cannot apply '{}' to {:?} and {:?} at compile time.",
                op, left, right
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(expr: &Expression) -> Result<Object, ConstEvalError> {
        eval_const_expr(expr, &HashMap::new())
    }

    #[test]
    fn test_const_divide_by_zero() {
        let expr = Expression::Infix {
            left: Box::new(Expression::Integer {
                value: 1,
                span: Default::default(),
            }),
            operator: "/".to_string(),
            right: Box::new(Expression::Integer {
                value: 0,
                span: Default::default(),
            }),
            span: Default::default(),
        };
        let err = eval(&expr).unwrap_err();
        assert_eq!(err.code, "E046");
        assert!(err.message.contains("Division by zero"));
    }

    #[test]
    fn test_const_mod_by_zero() {
        let expr = Expression::Infix {
            left: Box::new(Expression::Integer {
                value: 1,
                span: Default::default(),
            }),
            operator: "%".to_string(),
            right: Box::new(Expression::Integer {
                value: 0,
                span: Default::default(),
            }),
            span: Default::default(),
        };
        let err = eval(&expr).unwrap_err();
        assert_eq!(err.code, "E046");
        assert!(err.message.contains("Modulo by zero"));
    }

    #[test]
    fn test_const_string_ordering() {
        let expr = Expression::Infix {
            left: Box::new(Expression::String {
                value: "b".to_string(),
                span: Default::default(),
            }),
            operator: ">".to_string(),
            right: Box::new(Expression::String {
                value: "a".to_string(),
                span: Default::default(),
            }),
            span: Default::default(),
        };
        let result = eval(&expr).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_const_float_divide_by_zero() {
        let expr = Expression::Infix {
            left: Box::new(Expression::Float {
                value: 1.0,
                span: Default::default(),
            }),
            operator: "/".to_string(),
            right: Box::new(Expression::Float {
                value: 0.0,
                span: Default::default(),
            }),
            span: Default::default(),
        };
        let err = eval(&expr).unwrap_err();
        assert_eq!(err.code, "E046");
        assert!(err.message.contains("Division by zero"));
    }

    #[test]
    fn test_const_deep_recursion() {
        // Test nested binary operations (simulates deep recursion)
        let mut expr = Expression::Integer {
            value: 1,
            span: Default::default(),
        };

        // Create a deeply nested expression: 1 + 1 + 1 + ... (50 levels)
        for _ in 0..50 {
            expr = Expression::Infix {
                left: Box::new(expr),
                operator: "+".to_string(),
                right: Box::new(Expression::Integer {
                    value: 1,
                    span: Default::default(),
                }),
                span: Default::default(),
            };
        }

        let result = eval(&expr).unwrap();
        assert_eq!(result, Object::Integer(51));
    }

    #[test]
    fn test_const_large_array() {
        // Test array with 100 elements
        let elements: Vec<Expression> = (0..100)
            .map(|i| Expression::Integer {
                value: i,
                span: Default::default(),
            })
            .collect();

        let expr = Expression::Array {
            elements,
            span: Default::default(),
        };

        let result = eval(&expr).unwrap();
        match result {
            Object::Array(arr) => {
                assert_eq!(arr.len(), 100);
                assert_eq!(arr[0], Object::Integer(0));
                assert_eq!(arr[99], Object::Integer(99));
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_const_large_hash() {
        // Test hash with 50 key-value pairs
        let pairs: Vec<(Expression, Expression)> = (0..50)
            .map(|i| {
                (
                    Expression::Integer {
                        value: i,
                        span: Default::default(),
                    },
                    Expression::Integer {
                        value: i * 2,
                        span: Default::default(),
                    },
                )
            })
            .collect();

        let expr = Expression::Hash {
            pairs,
            span: Default::default(),
        };

        let result = eval(&expr).unwrap();
        match result {
            Object::Hash(map) => {
                assert_eq!(map.len(), 50);
                assert_eq!(
                    map.get(&crate::runtime::hash_key::HashKey::Integer(0)),
                    Some(&Object::Integer(0))
                );
                assert_eq!(
                    map.get(&crate::runtime::hash_key::HashKey::Integer(49)),
                    Some(&Object::Integer(98))
                );
            }
            _ => panic!("Expected hash"),
        }
    }

    #[test]
    fn test_const_error_with_hint() {
        // Test that hints are properly included in errors
        let expr = Expression::Call {
            function: Box::new(Expression::Identifier {
                name: "foo".to_string(),
                span: Default::default(),
            }),
            arguments: vec![],
            span: Default::default(),
        };

        let err = eval(&expr).unwrap_err();
        assert_eq!(err.code, "E042");
        assert!(err.hint.is_some());
        assert!(err
            .hint
            .unwrap()
            .contains("Module constants must be evaluable at compile time"));
    }
}
