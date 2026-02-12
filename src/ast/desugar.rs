use crate::ast::fold::{self, Folder};
use crate::syntax::{expression::Expression, program::Program};

/// AST desugaring pass.
///
/// Rewrites syntactic sugar into simpler core constructs. Current rules:
///
/// 1. **Double negation elimination:** `!!x` → `x`
/// 2. **Negated comparison:** `!(a == b)` → `a != b`, `!(a != b)` → `a == b`
///
/// This module is designed to be extended as new syntax sugar is added
/// (e.g., cons syntax from proposal 017, async/await from proposal 026).
struct DesugarPass;

impl DesugarPass {
    /// Try to simplify a prefix `!` expression. Returns `Some(simplified)` if
    /// a rule applies, `None` otherwise.
    fn try_simplify_not(inner: &Expression) -> Option<Expression> {
        match inner {
            // Rule 1: !!x → x
            Expression::Prefix {
                operator, right, ..
            } if operator == "!" => Some(right.as_ref().clone()),

            // Rule 2: !(a == b) → a != b, !(a != b) → a == b
            Expression::Infix {
                left,
                operator,
                right,
                span,
            } => {
                let flipped = match operator.as_str() {
                    "==" => Some("!="),
                    "!=" => Some("=="),
                    _ => None,
                };
                flipped.map(|op| Expression::Infix {
                    left: left.clone(),
                    operator: op.to_string(),
                    right: right.clone(),
                    span: *span,
                })
            }

            _ => None,
        }
    }
}

impl Folder for DesugarPass {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        // Fold children first (bottom-up)
        let expr = fold::fold_expr(self, expr);

        match &expr {
            Expression::Prefix {
                operator, right, ..
            } if operator == "!" => {
                if let Some(simplified) = Self::try_simplify_not(right) {
                    return simplified;
                }
                expr
            }
            _ => expr,
        }
    }
}

/// Apply desugaring rules to a program.
pub fn desugar(program: Program) -> Program {
    let mut pass = DesugarPass;
    pass.fold_program(program)
}
