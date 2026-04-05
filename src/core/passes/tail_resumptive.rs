/// Core-level tail-resumptive handler detection.
///
/// A handler is tail-resumptive when every arm's body terminates with exactly
/// one call to `resume(expr)` as its terminal expression on every code path.
/// For these handlers, the `evidence_pass` can rewrite Perform/Handle into
/// explicit evidence passing (direct function calls).
use crate::core::{CoreBinderId, CoreExpr, CoreHandler};

/// Returns `true` if **all** arms of a handler are tail-resumptive.
pub fn is_core_handler_tail_resumptive(handlers: &[CoreHandler]) -> bool {
    handlers
        .iter()
        .all(|h| is_arm_tail_resumptive(h.resume.id, &h.body))
}

/// Check whether a handler arm body terminates with `resume(v)` on every path.
fn is_arm_tail_resumptive(resume_id: CoreBinderId, body: &CoreExpr) -> bool {
    match body {
        // Terminal: App(Var(resume), [arg]) — the ideal case.
        CoreExpr::App { func, args, .. } => is_resume_var(resume_id, func) && args.len() == 1,

        // Let: let x = e in <tail-resumptive body>
        CoreExpr::Let { body, .. } => is_arm_tail_resumptive(resume_id, body),

        // LetRec: letrec x = e in <tail-resumptive body>
        CoreExpr::LetRec { body, .. } => is_arm_tail_resumptive(resume_id, body),

        // LetRecGroup: letrec group in <tail-resumptive body>
        CoreExpr::LetRecGroup { body, .. } => is_arm_tail_resumptive(resume_id, body),

        // Case: all alternatives must be tail-resumptive.
        CoreExpr::Case { alts, .. } => alts
            .iter()
            .all(|alt| is_arm_tail_resumptive(resume_id, &alt.rhs)),

        // Anything else is conservatively NOT tail-resumptive.
        _ => false,
    }
}

/// Check whether `expr` is a reference to the resume binder.
fn is_resume_var(resume_id: CoreBinderId, expr: &CoreExpr) -> bool {
    matches!(expr, CoreExpr::Var { var, .. } if var.binder == Some(resume_id))
}
