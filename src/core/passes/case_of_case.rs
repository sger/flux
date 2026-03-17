/// Case-of-case transformation.
///
/// When the scrutinee of a `Case` is itself a `Case`, push the outer
/// alternatives into each arm of the inner `Case`. This eliminates
/// intermediate constructor allocation.
///
/// ```text
/// case (case x of { P1 -> e1; P2 -> e2 }) of { Q1 -> r1; Q2 -> r2 }
///   →
/// case x of { P1 -> case e1 of { Q1 -> r1; Q2 -> r2 };
///              P2 -> case e2 of { Q1 -> r1; Q2 -> r2 } }
/// ```
///
/// Guards in the **inner** case are preserved. The outer alternatives are
/// cloned into each inner arm. A size limit prevents code explosion when
/// the outer alternatives are large.
use crate::core::{CoreAlt, CoreExpr};

use super::helpers::{expr_size, map_children};

/// Maximum total node count of outer alternatives before we skip the
/// transformation to avoid code size explosion.
const MAX_OUTER_ALTS_SIZE: usize = 50;

pub fn case_of_case(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Case {
            scrutinee,
            alts: outer_alts,
            span: outer_span,
        } => {
            // First, recursively transform the scrutinee and outer alts.
            let scrutinee = case_of_case(*scrutinee);
            let outer_alts: Vec<CoreAlt> = outer_alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = case_of_case(alt.rhs);
                    alt.guard = alt.guard.map(case_of_case);
                    alt
                })
                .collect();

            // Check if the (now-simplified) scrutinee is itself a Case.
            if let CoreExpr::Case {
                scrutinee: inner_scrutinee,
                alts: inner_alts,
                span: inner_span,
            } = scrutinee
            {
                // Skip if outer alternatives are too large (prevents code explosion).
                let outer_size: usize = outer_alts.iter().map(|a| expr_size(&a.rhs)).sum();
                if outer_size > MAX_OUTER_ALTS_SIZE {
                    return CoreExpr::Case {
                        scrutinee: Box::new(CoreExpr::Case {
                            scrutinee: inner_scrutinee,
                            alts: inner_alts,
                            span: inner_span,
                        }),
                        alts: outer_alts,
                        span: outer_span,
                    };
                }

                // Push outer case into each inner arm's RHS.
                let new_inner_alts: Vec<CoreAlt> = inner_alts
                    .into_iter()
                    .map(|mut inner_alt| {
                        inner_alt.rhs = CoreExpr::Case {
                            scrutinee: Box::new(inner_alt.rhs),
                            alts: outer_alts.clone(),
                            span: outer_span,
                        };
                        // Recursively simplify — the pushed case may now be
                        // a case-of-known-constructor or another case-of-case.
                        inner_alt.rhs = case_of_case(inner_alt.rhs);
                        inner_alt
                    })
                    .collect();

                CoreExpr::Case {
                    scrutinee: inner_scrutinee,
                    alts: new_inner_alts,
                    span: inner_span,
                }
            } else {
                CoreExpr::Case {
                    scrutinee: Box::new(scrutinee),
                    alts: outer_alts,
                    span: outer_span,
                }
            }
        }
        CoreExpr::Lam { .. }
        | CoreExpr::App { .. }
        | CoreExpr::Let { .. }
        | CoreExpr::LetRec { .. }
        | CoreExpr::Con { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::Return { .. }
        | CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. } => map_children(expr, case_of_case),
        other => other,
    }
}
