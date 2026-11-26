/// Core-level tail-resumptive handler detection.
///
/// A handler is tail-resumptive when every arm's body terminates with exactly
/// one call to `resume(expr)` as its terminal expression on every code path.
/// For these handlers, the `evidence_pass` can rewrite Perform/Handle into
/// explicit evidence passing (direct function calls).
use crate::{
    core::{CoreBinderId, CoreExpr, CoreHandler},
    syntax::Identifier,
};

/// Returns `true` if **all** arms of a handler for `effect` are
/// tail-resumptive.
///
/// @KI-233: an arm that performs `effect` itself is not. A clause runs
/// outside its own handler, and the paths for a tail-resumptive handler call
/// the clause where the perform was, under that handler: the clause's perform
/// would reach it again.
pub fn is_core_handler_tail_resumptive(effect: Identifier, handlers: &[CoreHandler]) -> bool {
    handlers
        .iter()
        .all(|h| is_arm_tail_resumptive(h.resume.id, &h.body) && !performs_effect(&h.body, effect))
}

/// Whether `expr` performs `effect` anywhere in it, lambdas included, other
/// than in the body of a `handle` for `effect` inside it, which reaches that
/// handle.
pub fn performs_effect(expr: &CoreExpr, effect: Identifier) -> bool {
    let children: Vec<&CoreExpr> = match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(..) => Vec::new(),
        CoreExpr::Perform { effect: performed, args, .. } => {
            if *performed == effect {
                return true;
            }
            args.iter().collect()
        }
        CoreExpr::Lam { body, .. } => vec![body],
        CoreExpr::App { func, args, .. } => std::iter::once(func.as_ref()).chain(args).collect(),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => vec![rhs, body],
        CoreExpr::LetRecGroup { bindings, body, .. } => bindings
            .iter()
            .map(|(_, rhs)| rhs.as_ref())
            .chain(std::iter::once(body.as_ref()))
            .collect(),
        CoreExpr::Case { scrutinee, alts, .. } => std::iter::once(scrutinee.as_ref())
            .chain(alts.iter().flat_map(|alt| alt.guard.iter().chain(std::iter::once(&alt.rhs))))
            .collect(),
        CoreExpr::Con { fields: args, .. } | CoreExpr::PrimOp { args, .. } => args.iter().collect(),
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            vec![object]
        }
        CoreExpr::Return { value, .. } => vec![value],
        CoreExpr::Handle { body, effect: handled, parameter, handlers, .. } => {
            std::iter::once(body.as_ref())
                .filter(|_| *handled != effect)
                .chain(parameter.as_deref())
                .chain(handlers.iter().map(|h| &h.body))
                .collect()
        }
    };
    children.into_iter().any(|child| performs_effect(child, effect))
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
        CoreExpr::Case { alts, .. } => {
            alts.iter().all(|alt| is_arm_tail_resumptive(resume_id, &alt.rhs))
        }

        // Anything else is conservatively NOT tail-resumptive.
        _ => false,
    }
}

/// Check whether `expr` is a reference to the resume binder.
fn is_resume_var(resume_id: CoreBinderId, expr: &CoreExpr) -> bool {
    matches!(expr, CoreExpr::Var { var, .. } if var.binder == Some(resume_id))
}
