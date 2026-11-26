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
    arms.iter().all(|arm| is_arm_tail_resumptive(arm.resume_param, &arm.body))
}

/// Check whether an arm body terminates with `resume(v)` on every path.
fn is_arm_tail_resumptive(resume: Identifier, body: &Expression) -> bool {
    match body {
        // Direct: `resume(v)` — the ideal case.
        // @KI-243: a clause runs in place only if `resume` appears nowhere
        // but in the call it ends with.
        Expression::Call { function, arguments, .. } => {
            is_resume_call(resume, function)
                && arguments.len() == 1
                && !mentions(resume, &arguments[0])
        }

        // Do-block: `do { stmts; resume(v) }` — last statement must be
        // tail-resumptive.
        Expression::DoBlock { block, .. } => {
            let Some((last, before)) = block.statements.split_last() else {
                return false;
            };
            if before.iter().any(|s| statement_mentions(resume, s)) {
                return false;
            }
            match last {
                crate::syntax::statement::Statement::Expression { expression, .. } => {
                    is_arm_tail_resumptive(resume, expression)
                }
                _ => false,
            }
        }

        // If-else: both branches must be tail-resumptive.
        Expression::If { condition, consequence, alternative, .. } => {
            if mentions(resume, condition) {
                return false;
            }
            let block_ok = |block: &crate::syntax::block::Block| {
                block.statements.split_last().is_some_and(|(_, before)| {
                    !before.iter().any(|s| statement_mentions(resume, s))
                })
            };
            if !block_ok(consequence) || !alternative.as_ref().is_some_and(block_ok) {
                return false;
            }
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
        Expression::Match { scrutinee, arms, .. } => {
            !mentions(resume, scrutinee)
                && arms.iter().all(|arm| {
                    !arm.guard.as_ref().is_some_and(|g| mentions(resume, g))
                        && is_arm_tail_resumptive(resume, &arm.body)
                })
        }

        // Anything else is conservatively NOT tail-resumptive.
        _ => false,
    }
}

/// Check whether `expr` is a call to the resume parameter.
fn is_resume_call(resume: Identifier, expr: &Expression) -> bool {
    matches!(expr, Expression::Identifier { name, .. } if *name == resume)
}

/// Whether `resume` is free in `expr`.
fn mentions(resume: Identifier, expr: &Expression) -> bool {
    crate::ast::free_vars::collect_free_vars(expr).contains(&resume)
}

/// Whether `resume` is free in a statement before a clause's last one.
fn statement_mentions(resume: Identifier, statement: &crate::syntax::statement::Statement) -> bool {
    crate::ast::free_vars::collect_free_vars_in_statement(statement).contains(&resume)
}

/// Whether every arm reads no evidence: none performs, installs a handler or
/// calls anything but its `resume` and the functions `pure_call` accepts,
/// any of which could perform. Only such a handle is direct, its clauses
/// called where the perform was with the perform's evidence left in place
/// (KI-243).
pub fn arms_perform_nothing(arms: &[HandleArm], pure_call: &dyn Fn(Identifier) -> bool) -> bool {
    use crate::ast::visit::{Visitor, walk_expr};
    struct Performs<'a> {
        resume: Identifier,
        pure_call: &'a dyn Fn(Identifier) -> bool,
        found: bool,
    }
    impl<'ast> Visitor<'ast> for Performs<'_> {
        fn visit_expr(&mut self, expr: &'ast Expression) {
            match expr {
                Expression::Perform { .. } | Expression::Handle { .. } => self.found = true,
                Expression::Call { function, .. } => {
                    let pure = match function.as_ref() {
                        Expression::Identifier { name, .. } => {
                            *name == self.resume || (self.pure_call)(*name)
                        }
                        _ => false,
                    };
                    if !pure {
                        self.found = true;
                    }
                }
                _ => {}
            }
            if !self.found {
                walk_expr(self, expr);
            }
        }
    }
    arms.iter().all(|arm| {
        let mut performs = Performs { resume: arm.resume_param, pure_call, found: false };
        performs.visit_expr(&arm.body);
        !performs.found
    })
}

/// Returns `true` if **all** arms of a handler are "discard" — they never
/// reference `resume`. These handlers can skip continuation capture entirely.
/// (Perceus Section 2.7.1: non-linear control flow safety.)
pub fn is_handler_discard(arms: &[HandleArm]) -> bool {
    arms.iter().all(|arm| !identifier_appears_in_expr(arm.resume_param, &arm.body))
}

/// Check whether an identifier appears anywhere in an expression tree.
/// Conservative: returns `true` (appears) for unrecognized expression forms.
fn identifier_appears_in_expr(name: Identifier, expr: &Expression) -> bool {
    match expr {
        Expression::Identifier { name: n, .. } => *n == name,
        Expression::Call { function, arguments, .. } => {
            identifier_appears_in_expr(name, function)
                || arguments.iter().any(|a| identifier_appears_in_expr(name, a))
        }
        Expression::DoBlock { block, .. } => block.statements.iter().any(|s| match s {
            crate::syntax::statement::Statement::Expression { expression, .. } => {
                identifier_appears_in_expr(name, expression)
            }
            crate::syntax::statement::Statement::Let { value, .. } => {
                identifier_appears_in_expr(name, value)
            }
            _ => false,
        }),
        Expression::If { condition, consequence, alternative, .. } => {
            identifier_appears_in_expr(name, condition)
                || consequence.statements.iter().any(|s| match s {
                    crate::syntax::statement::Statement::Expression { expression, .. } => {
                        identifier_appears_in_expr(name, expression)
                    }
                    _ => false,
                })
                || alternative.as_ref().is_some_and(|alt| {
                    alt.statements.iter().any(|s| match s {
                        crate::syntax::statement::Statement::Expression { expression, .. } => {
                            identifier_appears_in_expr(name, expression)
                        }
                        _ => false,
                    })
                })
        }
        Expression::Match { arms, .. } => {
            arms.iter().any(|arm| identifier_appears_in_expr(name, &arm.body))
        }
        Expression::Infix { left, right, .. } => {
            identifier_appears_in_expr(name, left) || identifier_appears_in_expr(name, right)
        }
        Expression::Prefix { right, .. } => identifier_appears_in_expr(name, right),
        Expression::Index { left, index, .. } => {
            identifier_appears_in_expr(name, left) || identifier_appears_in_expr(name, index)
        }
        Expression::MemberAccess { object, .. } => identifier_appears_in_expr(name, object),
        Expression::ArrayLiteral { elements, .. } | Expression::TupleLiteral { elements, .. } => {
            elements.iter().any(|e| identifier_appears_in_expr(name, e))
        }
        // Conservative default: for any unhandled expression form,
        // check all child expressions via the Expression::children() method
        // if available, or assume resume might appear.
        _ => {
            // For literals and other leaf nodes, resume can't appear.
            // For complex forms we don't handle, conservatively say true.
            // This is safe — it just prevents the discard optimization for
            // handler arms with unusual expression forms.
            true
        }
    }
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

    /// `resume` given to itself can't run in place: the inner call would
    /// give back its argument instead of running what it resumes (KI-243).
    #[test]
    fn resume_in_its_own_argument_is_not_tail_resumptive() {
        let arms = parse_handle_arms(
            "effect E { op : (Int) -> Int }\n() handle E { op(resume, x) -> resume(resume(x)) }",
        );
        assert!(!is_handler_tail_resumptive(&arms));
    }

    /// A clause that performs or calls a function may read evidence; one
    /// that only computes and resumes doesn't (KI-243).
    #[test]
    fn a_clause_that_calls_or_performs_reads_evidence() {
        let pure = parse_handle_arms(
            "effect E { op : (Int) -> Int }\n() handle E { op(resume, x) -> resume(x + 1) }",
        );
        assert!(arms_perform_nothing(&pure, &|_| false));
        let calls = parse_handle_arms(
            "effect E { op : (Int) -> Int }\n() handle E { op(resume, x) -> resume(f(x)) }",
        );
        assert!(!arms_perform_nothing(&calls, &|_| false));
        let performs = parse_handle_arms(
            "effect E { op : (Int) -> Int }\n() handle E { op(resume, x) -> resume(perform E.op(x)) }",
        );
        assert!(!arms_perform_nothing(&performs, &|_| false));
    }
}
