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

impl Folder for DesugarPass {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        // Fold children first (bottom-up)
        let expr = fold::fold_expr(self, expr);

        // Only act on `!inner` expressions.
        let is_not = matches!(&expr, Expression::Prefix { operator, .. } if operator == "!");
        if !is_not {
            return expr;
        }

        // Destructure owned expression to avoid cloning sub-trees.
        let Expression::Prefix { right, span, .. } = expr else {
            unreachable!()
        };

        match *right {
            // Rule 1: !!x → x
            Expression::Prefix {
                ref operator,
                right: inner,
                ..
            } if operator == "!" => *inner,

            // Rule 2: !(a == b) → a != b, !(a != b) → a == b
            Expression::Infix {
                left,
                ref operator,
                right,
                span: infix_span,
            } if operator == "==" || operator == "!=" => {
                let flipped = if operator == "==" { "!=" } else { "==" };
                Expression::Infix {
                    left,
                    operator: flipped.to_string(),
                    right,
                    span: infix_span,
                }
            }

            // No simplification; reconstruct the `!inner` expression.
            inner => Expression::Prefix {
                operator: "!".to_string(),
                right: Box::new(inner),
                span,
            },
        }
    }
}

/// Apply desugaring rules to a program.
pub fn desugar(program: Program) -> Program {
    let mut pass = DesugarPass;
    pass.fold_program(program)
}
