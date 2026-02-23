use crate::ast::fold::{self, Folder};
use crate::primop::{PrimEffect, PrimOp, resolve_primop_call};
use crate::syntax::interner::Interner;
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

struct InternerAwareConstantFolder<'a> {
    interner: &'a Interner,
}

impl Folder for InternerAwareConstantFolder<'_> {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        // Fold children first (bottom-up)
        let expr = fold::fold_expr(self, expr);

        // Keep the baseline constant folding behavior.
        let expr = ConstantFolder.fold_expr(expr);

        match expr {
            Expression::Call {
                function,
                arguments,
                span,
            } => {
                if let Some(folded) =
                    fold_pure_primop_call(self.interner, function.as_ref(), &arguments, span)
                {
                    return folded;
                }
                Expression::Call {
                    function,
                    arguments,
                    span,
                }
            }
            other => other,
        }
    }
}

fn fold_pure_primop_call(
    interner: &Interner,
    function: &Expression,
    arguments: &[Expression],
    span: crate::diagnostics::position::Span,
) -> Option<Expression> {
    let Expression::Identifier { name, .. } = function else {
        return None;
    };
    let call_name = interner.resolve(*name);
    let op = resolve_primop_call(call_name, arguments.len())?;
    if op.effect_kind() != PrimEffect::Pure {
        return None;
    }

    match op {
        PrimOp::Abs => match arguments.first()? {
            Expression::Integer { value, .. } => Some(Expression::Integer {
                value: value.abs(),
                span,
            }),
            Expression::Float { value, .. } => Some(Expression::Float {
                value: value.abs(),
                span,
            }),
            _ => None,
        },
        PrimOp::Min => fold_min_max(arguments, span, true),
        PrimOp::Max => fold_min_max(arguments, span, false),
        PrimOp::StringLen => match arguments.first()? {
            Expression::String { value, .. } => Some(Expression::Integer {
                value: value.len() as i64,
                span,
            }),
            _ => None,
        },
        PrimOp::StringConcat => match (arguments.first()?, arguments.get(1)?) {
            (Expression::String { value: left, .. }, Expression::String { value: right, .. }) => {
                Some(Expression::String {
                    value: format!("{left}{right}"),
                    span,
                })
            }
            _ => None,
        },
        _ => None,
    }
}

fn fold_min_max(
    arguments: &[Expression],
    span: crate::diagnostics::position::Span,
    is_min: bool,
) -> Option<Expression> {
    let a = arguments.first()?;
    let b = arguments.get(1)?;
    match (a, b) {
        (Expression::Integer { value: a, .. }, Expression::Integer { value: b, .. }) => {
            Some(Expression::Integer {
                value: if is_min { (*a).min(*b) } else { (*a).max(*b) },
                span,
            })
        }
        (Expression::Float { value: a, .. }, Expression::Float { value: b, .. }) => {
            Some(Expression::Float {
                value: if is_min { (*a).min(*b) } else { (*a).max(*b) },
                span,
            })
        }
        (Expression::Integer { value: a, .. }, Expression::Float { value: b, .. }) => {
            let a = *a as f64;
            Some(Expression::Float {
                value: if is_min { a.min(*b) } else { a.max(*b) },
                span,
            })
        }
        (Expression::Float { value: a, .. }, Expression::Integer { value: b, .. }) => {
            let b = *b as f64;
            Some(Expression::Float {
                value: if is_min { (*a).min(b) } else { (*a).max(b) },
                span,
            })
        }
        _ => None,
    }
}

/// Apply constant folding to a program.
pub fn constant_fold(program: Program) -> Program {
    let mut folder = ConstantFolder;
    folder.fold_program(program)
}

/// Apply constant folding with identifier-aware pure-primop folding.
pub fn constant_fold_with_interner(program: Program, interner: &Interner) -> Program {
    let mut folder = InternerAwareConstantFolder { interner };
    folder.fold_program(program)
}
