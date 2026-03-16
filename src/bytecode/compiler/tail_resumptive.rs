//! Tail-resumptive handler detection.
//!
//! A handler is *tail-resumptive* when every arm's body ends with exactly
//! one call to `resume(expr)` as its terminal expression on every code path.
//! For these handlers the continuation is never stored, multi-shot, or
//! discarded — so we can skip the heap allocation entirely and dispatch
//! the handler arm as a direct function call.

use crate::syntax::Identifier;
use crate::syntax::expression::{Expression, HandleArm};

/// Returns `true` if **all** arms of a handler are tail-resumptive.
pub fn is_handler_tail_resumptive(arms: &[HandleArm]) -> bool {
    arms.iter()
        .all(|arm| is_arm_tail_resumptive(arm.resume_param, &arm.body))
}

/// Check whether an arm body terminates with `resume(v)` on every path.
fn is_arm_tail_resumptive(resume: Identifier, body: &Expression) -> bool {
    match body {
        // Direct: `resume(v)` — the ideal case.
        Expression::Call {
            function,
            arguments,
            ..
        } => is_resume_call(resume, function) && arguments.len() == 1,

        // Do-block: `do { stmts; resume(v) }` — last statement must be
        // tail-resumptive.
        Expression::DoBlock { block, .. } => {
            let Some(last) = block.statements.last() else {
                return false;
            };
            match last {
                crate::syntax::statement::Statement::Expression { expression, .. } => {
                    is_arm_tail_resumptive(resume, expression)
                }
                _ => false,
            }
        }

        // If-else: both branches must be tail-resumptive.
        Expression::If {
            consequence,
            alternative,
            ..
        } => {
            let then_ok = consequence.statements.last().is_some_and(|s| match s {
                crate::syntax::statement::Statement::Expression { expression, .. } => {
                    is_arm_tail_resumptive(resume, expression)
                }
                _ => false,
            });
            let else_ok = alternative.as_ref().is_some_and(|alt| {
                alt.statements.last().is_some_and(|s| match s {
                    crate::syntax::statement::Statement::Expression { expression, .. } => {
                        is_arm_tail_resumptive(resume, expression)
                    }
                    _ => false,
                })
            });
            then_ok && else_ok
        }

        // Match: every arm must be tail-resumptive.
        Expression::Match { arms, .. } => arms
            .iter()
            .all(|arm| is_arm_tail_resumptive(resume, &arm.body)),

        // Anything else is conservatively NOT tail-resumptive.
        _ => false,
    }
}

/// Check whether `expr` is a call to the resume parameter.
fn is_resume_call(resume: Identifier, expr: &Expression) -> bool {
    matches!(expr, Expression::Identifier { name, .. } if *name == resume)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{lexer::Lexer, parser::Parser};

    fn parse_handle_arms(input: &str) -> Vec<HandleArm> {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        for stmt in &program.statements {
            if let crate::syntax::statement::Statement::Expression {
                expression: Expression::Handle { arms, .. },
                ..
            } = stmt
            {
                return arms.clone();
            }
        }
        panic!("no Handle expression found in: {input}");
    }

    #[test]
    fn direct_resume_is_tail_resumptive() {
        let arms = parse_handle_arms(
            "effect E { op : (Int) -> Int }\n() handle E { op(resume, x) -> resume(x) }",
        );
        assert!(is_handler_tail_resumptive(&arms));
    }

    #[test]
    fn do_block_ending_with_resume_is_tail_resumptive() {
        let arms = parse_handle_arms(
            "effect E { op : (Int) -> Int }\n() handle E { op(resume, x) -> do { let y = x + 1; resume(y) } }",
        );
        assert!(is_handler_tail_resumptive(&arms));
    }

    #[test]
    fn no_resume_is_not_tail_resumptive() {
        let arms = parse_handle_arms(
            "effect E { op : (Int) -> Int }\n() handle E { op(resume, x) -> x + 1 }",
        );
        assert!(!is_handler_tail_resumptive(&arms));
    }

    #[test]
    fn resume_with_wrong_arity_is_not_tail_resumptive() {
        let arms = parse_handle_arms(
            "effect E { op : (Int) -> Int }\n() handle E { op(resume, x) -> resume(x, x) }",
        );
        assert!(!is_handler_tail_resumptive(&arms));
    }
}
