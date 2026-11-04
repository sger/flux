use crate::core::CoreExpr;
/// Case-of-known-constructor pass.
///
/// Reduces `Case(Con(tag, fields), alts)` and `Case(Lit(l), alts)` when the
/// scrutinee is a statically-known value.
///
/// It doesn't look into a field's own pattern: at an arm like `Some(Some(a))`
/// or `Some(1)` the `case` stays, for lowering to match at run time.
///
/// Examples:
/// ```text
/// Case(Con(Some, [Lit(42)]), [Con(Some, [Var(x)]) → x])  →  Lit(42)
/// Case(Lit(true), [Lit(true) → a, Wildcard → b])          →  a
/// ```
use crate::shared::opt_bisect;

use super::helpers::{bind_pattern, map_children, select_known_arm};

pub fn case_of_known_constructor(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Case { scrutinee, alts, join_ty, span } => {
            let scrutinee = case_of_known_constructor(*scrutinee);
            let alts: Vec<_> = alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = case_of_known_constructor(alt.rhs);
                    alt.guard = alt.guard.map(case_of_known_constructor);
                    alt
                })
                .collect();
            // A bisected run past its limit (0190 §13) keeps the case.
            if let Some((alt, bindings)) = select_known_arm(&scrutinee, &alts)
                && opt_bisect::allow("case_of_known_constructor", || {
                    "select the arm a known value matches".to_string()
                })
            {
                let body = bind_pattern(alt.rhs.clone(), bindings, alt.span);
                return case_of_known_constructor(body);
            }
            CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, join_ty, span }
        }
        CoreExpr::Lam { .. }
        | CoreExpr::App { .. }
        | CoreExpr::Let { .. }
        | CoreExpr::LetRec { .. }
        | CoreExpr::LetRecGroup { .. }
        | CoreExpr::Con { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::Return { .. }
        | CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. } => map_children(expr, case_of_known_constructor),
        other => other,
    }
}
