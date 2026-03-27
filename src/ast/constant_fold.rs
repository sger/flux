use crate::ast::fold::{self, Folder};
use crate::diagnostics::position::Span;
use crate::core::{CorePrimOp, PrimEffect};
use crate::syntax::expression::ExprId;
use crate::syntax::interner::Interner;
use crate::syntax::{expression::Expression, program::Program};

/// Evaluates compile-time-constant expressions in a single bottom-up pass.
///
/// Folds children bottom-up, then simplifies the result:
/// - Integer arithmetic: `+`, `-`, `*`, `/`, `%` (skips division by zero)
/// - Float arithmetic: `+`, `-`, `*`, `/`
/// - String concatenation: `"a" + "b"` → `"ab"`
/// - Boolean logic: `&&`, `||`
/// - Integer comparison: `==`, `!=`, `<`, `>`, `<=`, `>=`
/// - Prefix negation: `-42` → `-42`, `!true` → `false`
/// - Pure primop calls (when interner is available): `abs(-5)` → `5`
struct ConstantFolder<'a> {
    interner: Option<&'a Interner>,
}

impl Folder for ConstantFolder<'_> {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        // Fold children first (bottom-up), then simplify the root.
        let expr = fold::fold_expr(self, expr);

        match expr {
            Expression::Infix {
                ref left,
                ref operator,
                ref right,
                span,
                id,
            } => {
                if let Some(folded) = try_fold_infix(left, operator, right, span, id) {
                    return folded;
                }
                expr
            }
            Expression::Prefix {
                ref operator,
                ref right,
                span,
                id,
            } => {
                if let Some(folded) = try_fold_prefix(operator, right, span, id) {
                    return folded;
                }
                expr
            }
            Expression::Call {
                ref function,
                ref arguments,
                span,
                id,
            } => {
                if let Some(interner) = self.interner
                    && let Some(folded) =
                        try_fold_pure_primop_call(interner, function, arguments, span, id)
                {
                    return folded;
                }
                expr
            }
            other => other,
        }
    }
}

/// Try to fold a constant infix expression. Returns `None` when no rule applies.
fn try_fold_infix(
    left: &Expression,
    operator: &str,
    right: &Expression,
    span: Span,
    id: ExprId,
) -> Option<Expression> {
    match (left, right) {
        (Expression::Integer { value: a, .. }, Expression::Integer { value: b, .. }) => {
            let (a, b) = (*a, *b);
            match operator {
                "+" => Some(Expression::Integer {
                    value: a + b,
                    span,
                    id,
                }),
                "-" => Some(Expression::Integer {
                    value: a - b,
                    span,
                    id,
                }),
                "*" => Some(Expression::Integer {
                    value: a * b,
                    span,
                    id,
                }),
                "/" if b != 0 => Some(Expression::Integer {
                    value: a / b,
                    span,
                    id,
                }),
                "%" if b != 0 => Some(Expression::Integer {
                    value: a % b,
                    span,
                    id,
                }),
                "==" => Some(Expression::Boolean {
                    value: a == b,
                    span,
                    id,
                }),
                "!=" => Some(Expression::Boolean {
                    value: a != b,
                    span,
                    id,
                }),
                "<" => Some(Expression::Boolean {
                    value: a < b,
                    span,
                    id,
                }),
                ">" => Some(Expression::Boolean {
                    value: a > b,
                    span,
                    id,
                }),
                "<=" => Some(Expression::Boolean {
                    value: a <= b,
                    span,
                    id,
                }),
                ">=" => Some(Expression::Boolean {
                    value: a >= b,
                    span,
                    id,
                }),
                _ => None,
            }
        }
        (Expression::Float { value: a, .. }, Expression::Float { value: b, .. }) => {
            let (a, b) = (*a, *b);
            match operator {
                "+" => Some(Expression::Float {
                    value: a + b,
                    span,
                    id,
                }),
                "-" => Some(Expression::Float {
                    value: a - b,
                    span,
                    id,
                }),
                "*" => Some(Expression::Float {
                    value: a * b,
                    span,
                    id,
                }),
                "/" => Some(Expression::Float {
                    value: a / b,
                    span,
                    id,
                }),
                _ => None,
            }
        }
        (Expression::String { value: a, .. }, Expression::String { value: b, .. }) => {
            if operator == "+" {
                let mut result = a.clone();
                result.push_str(b);
                Some(Expression::String {
                    value: result,
                    span,
                    id,
                })
            } else {
                None
            }
        }
        (Expression::Boolean { value: a, .. }, Expression::Boolean { value: b, .. }) => {
            let (a, b) = (*a, *b);
            match operator {
                "&&" => Some(Expression::Boolean {
                    value: a && b,
                    span,
                    id,
                }),
                "||" => Some(Expression::Boolean {
                    value: a || b,
                    span,
                    id,
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Try to fold a constant prefix expression. Returns `None` when no rule applies.
fn try_fold_prefix(
    operator: &str,
    right: &Expression,
    span: Span,
    id: ExprId,
) -> Option<Expression> {
    match (operator, right) {
        ("-", Expression::Integer { value, .. }) => Some(Expression::Integer {
            value: -value,
            span,
            id,
        }),
        ("-", Expression::Float { value, .. }) => Some(Expression::Float {
            value: -value,
            span,
            id,
        }),
        ("!", Expression::Boolean { value, .. }) => Some(Expression::Boolean {
            value: !value,
            span,
            id,
        }),
        _ => None,
    }
}

/// Try to fold a pure primop call with all-constant arguments.
fn try_fold_pure_primop_call(
    interner: &Interner,
    function: &Expression,
    arguments: &[Expression],
    span: Span,
    id: ExprId,
) -> Option<Expression> {
    let Expression::Identifier { name, .. } = function else {
        return None;
    };
    let call_name = interner.resolve(*name);
    let op = CorePrimOp::from_name(call_name, arguments.len())?;
    if op.effect_kind() != PrimEffect::Pure {
        return None;
    }

    match op {
        CorePrimOp::Abs => match arguments.first()? {
            Expression::Integer { value, .. } => Some(Expression::Integer {
                value: value.abs(),
                span,
                id,
            }),
            Expression::Float { value, .. } => Some(Expression::Float {
                value: value.abs(),
                span,
                id,
            }),
            _ => None,
        },
        CorePrimOp::Min => fold_min_max(arguments, span, true, id),
        CorePrimOp::Max => fold_min_max(arguments, span, false, id),
        CorePrimOp::StringLength => match arguments.first()? {
            Expression::String { value, .. } => Some(Expression::Integer {
                value: value.len() as i64,
                span,
                id,
            }),
            _ => None,
        },
        CorePrimOp::StringConcat => match (arguments.first()?, arguments.get(1)?) {
            (Expression::String { value: left, .. }, Expression::String { value: right, .. }) => {
                Some(Expression::String {
                    value: format!("{left}{right}"),
                    span,
                    id,
                })
            }
            _ => None,
        },
        _ => None,
    }
}

fn fold_min_max(
    arguments: &[Expression],
    span: Span,
    is_min: bool,
    id: ExprId,
) -> Option<Expression> {
    let a = arguments.first()?;
    let b = arguments.get(1)?;
    match (a, b) {
        (Expression::Integer { value: a, .. }, Expression::Integer { value: b, .. }) => {
            Some(Expression::Integer {
                value: if is_min { (*a).min(*b) } else { (*a).max(*b) },
                span,
                id,
            })
        }
        (Expression::Float { value: a, .. }, Expression::Float { value: b, .. }) => {
            Some(Expression::Float {
                value: if is_min { (*a).min(*b) } else { (*a).max(*b) },
                span,
                id,
            })
        }
        (Expression::Integer { value: a, .. }, Expression::Float { value: b, .. }) => {
            let a = *a as f64;
            Some(Expression::Float {
                value: if is_min { a.min(*b) } else { a.max(*b) },
                span,
                id,
            })
        }
        (Expression::Float { value: a, .. }, Expression::Integer { value: b, .. }) => {
            let b = *b as f64;
            Some(Expression::Float {
                value: if is_min { (*a).min(b) } else { (*a).max(b) },
                span,
                id,
            })
        }
        _ => None,
    }
}

/// Apply constant folding to a program (without primop folding).
pub fn constant_fold(program: Program) -> Program {
    let mut folder = ConstantFolder { interner: None };
    folder.fold_program(program)
}

/// Apply constant folding with identifier-aware pure-primop folding.
pub fn constant_fold_with_interner(program: Program, interner: &Interner) -> Program {
    let mut folder = ConstantFolder {
        interner: Some(interner),
    };
    folder.fold_program(program)
}
