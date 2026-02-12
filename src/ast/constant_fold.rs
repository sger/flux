use crate::ast::fold::{self, Folder};
use crate::syntax::{expression::Expression, program::Program};

/// Evaluates compile-time-constant expressions.
///
/// Folds children bottom-up, then simplifies the result:
/// - Integer arithmetic: `+`, `-`, `*`, `/`, `%` (skips division by zero)
/// - Float arithmetic: `+`, `-`, `*`, `/`
/// - String concatenation: `"a" + "b"` → `"ab"`
/// - Boolean logic: `&&`, `||`
/// - Integer comparison: `==`, `!=`, `<`, `>`, `<=`, `>=`
/// - Prefix negation: `-42` → `-42`, `!true` → `false`
struct ConstantFolder;

impl Folder for ConstantFolder {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        // Fold children first (bottom-up)
        let expr = fold::fold_expr(self, expr);

        match expr {
            // Integer infix operations
            Expression::Infix {
                left,
                ref operator,
                right,
                span,
            } => match (left.as_ref(), right.as_ref()) {
                (Expression::Integer { value: a, .. }, Expression::Integer { value: b, .. }) => {
                    let a = *a;
                    let b = *b;
                    match operator.as_str() {
                        "+" => return Expression::Integer { value: a + b, span },
                        "-" => return Expression::Integer { value: a - b, span },
                        "*" => return Expression::Integer { value: a * b, span },
                        "/" if b != 0 => return Expression::Integer { value: a / b, span },
                        "%" if b != 0 => return Expression::Integer { value: a % b, span },
                        "==" => {
                            return Expression::Boolean {
                                value: a == b,
                                span,
                            };
                        }
                        "!=" => {
                            return Expression::Boolean {
                                value: a != b,
                                span,
                            };
                        }
                        "<" => return Expression::Boolean { value: a < b, span },
                        ">" => return Expression::Boolean { value: a > b, span },
                        "<=" => {
                            return Expression::Boolean {
                                value: a <= b,
                                span,
                            };
                        }
                        ">=" => {
                            return Expression::Boolean {
                                value: a >= b,
                                span,
                            };
                        }
                        _ => {}
                    }
                    // Return the original expression if no fold applied
                    Expression::Infix {
                        left,
                        operator: operator.clone(),
                        right,
                        span,
                    }
                }
                // Float infix operations
                (Expression::Float { value: a, .. }, Expression::Float { value: b, .. }) => {
                    let a = *a;
                    let b = *b;
                    match operator.as_str() {
                        "+" => return Expression::Float { value: a + b, span },
                        "-" => return Expression::Float { value: a - b, span },
                        "*" => return Expression::Float { value: a * b, span },
                        "/" => return Expression::Float { value: a / b, span },
                        _ => {}
                    }
                    Expression::Infix {
                        left,
                        operator: operator.clone(),
                        right,
                        span,
                    }
                }
                // String concatenation
                (Expression::String { value: a, .. }, Expression::String { value: b, .. }) => {
                    if operator == "+" {
                        let mut result = a.clone();
                        result.push_str(b);
                        return Expression::String {
                            value: result,
                            span,
                        };
                    }
                    Expression::Infix {
                        left,
                        operator: operator.clone(),
                        right,
                        span,
                    }
                }
                // Boolean logic
                (Expression::Boolean { value: a, .. }, Expression::Boolean { value: b, .. }) => {
                    let a = *a;
                    let b = *b;
                    match operator.as_str() {
                        "&&" => {
                            return Expression::Boolean {
                                value: a && b,
                                span,
                            };
                        }
                        "||" => {
                            return Expression::Boolean {
                                value: a || b,
                                span,
                            };
                        }
                        _ => {}
                    }
                    Expression::Infix {
                        left,
                        operator: operator.clone(),
                        right,
                        span,
                    }
                }
                _ => Expression::Infix {
                    left,
                    operator: operator.clone(),
                    right,
                    span,
                },
            },
            // Prefix operations
            Expression::Prefix {
                ref operator,
                right,
                span,
            } => match (operator.as_str(), right.as_ref()) {
                ("-", Expression::Integer { value, .. }) => Expression::Integer {
                    value: -value,
                    span,
                },
                ("-", Expression::Float { value, .. }) => Expression::Float {
                    value: -value,
                    span,
                },
                ("!", Expression::Boolean { value, .. }) => Expression::Boolean {
                    value: !value,
                    span,
                },
                _ => Expression::Prefix {
                    operator: operator.clone(),
                    right,
                    span,
                },
            },
            other => other,
        }
    }
}

/// Apply constant folding to a program.
pub fn constant_fold(program: Program) -> Program {
    let mut folder = ConstantFolder;
    folder.fold_program(program)
}
