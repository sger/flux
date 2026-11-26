//! Whether a handle's clauses are tail-resumptive, as both backends decide
//! it from Aether: natively the evidence entry's flag, on the VM the
//! handler frame's. A tail-resumptive clause runs where the perform was,
//! under the evidence there; any other runs under its handle's.

use crate::aether::{AetherExpr, AetherHandler};
use crate::core::CoreBinderId;
use crate::syntax::Identifier;

/// Whether every clause of a handle for `effect` resumes only as its last
/// act, and none performs `effect`.
///
/// @KI-185: the entry has one flag for all its clauses, so it is set only
/// when every clause resumes as its last act. @KI-233: a clause that
/// performs the handle's own effect runs outside the handle, which a
/// tail-resumptive clause, run under the evidence where the perform was,
/// would not.
pub fn handle_is_tail_resumptive(effect: Identifier, handlers: &[AetherHandler]) -> bool {
    handlers.iter().all(|h| {
        resumes_only_as_last_act(&h.body, h.resume.id) && !performs_effect(&h.body, effect)
    })
}

/// Whether `expr` performs `effect` anywhere in it, lambdas included, other
/// than in the body of a `handle` for `effect` inside it, which reaches that
/// handle. Core's `performs_effect`, over Aether.
pub fn performs_effect(expr: &AetherExpr, effect: Identifier) -> bool {
    let children: Vec<&AetherExpr> = match expr {
        AetherExpr::Var { .. } | AetherExpr::Lit(..) => Vec::new(),
        AetherExpr::Perform { effect: performed, args, .. } => {
            if *performed == effect {
                return true;
            }
            args.iter().collect()
        }
        AetherExpr::Lam { body, .. }
        | AetherExpr::Dup { body, .. }
        | AetherExpr::Drop { body, .. }
        | AetherExpr::FreeShell { body, .. } => vec![body],
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            std::iter::once(func.as_ref()).chain(args).collect()
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            vec![rhs, body]
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => bindings
            .iter()
            .map(|(_, rhs)| rhs.as_ref())
            .chain(std::iter::once(body.as_ref()))
            .collect(),
        AetherExpr::Case { scrutinee, alts, .. } => std::iter::once(scrutinee.as_ref())
            .chain(alts.iter().flat_map(|alt| alt.guard.iter().chain(std::iter::once(&alt.rhs))))
            .collect(),
        AetherExpr::Con { fields: args, .. }
        | AetherExpr::Reuse { fields: args, .. }
        | AetherExpr::PrimOp { args, .. } => args.iter().collect(),
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            vec![object]
        }
        AetherExpr::Return { value, .. } => vec![value],
        AetherExpr::DropSpecialized { unique_body, shared_body, .. } => {
            vec![unique_body, shared_body]
        }
        AetherExpr::Handle { body, effect: handled, parameter, handlers, .. } => {
            std::iter::once(body.as_ref())
                .filter(|_| *handled != effect)
                .chain(parameter.as_deref())
                .chain(handlers.iter().map(|h| &h.body))
                .collect()
        }
    };
    children.into_iter().any(|child| performs_effect(child, effect))
}

/// Whether a handler clause uses `resume` only as a call it ends with: after
/// such a call the clause has nothing left to do, so the resumed code needs no
/// handler of its own around it.
pub fn resumes_only_as_last_act(body: &AetherExpr, resume: CoreBinderId) -> bool {
    let mentions = |expr: &AetherExpr| {
        crate::aether::free_vars::collect_free_vars_aether(expr).contains(&resume)
    };
    let last = |expr: &AetherExpr| resumes_only_as_last_act(expr, resume);
    match body {
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } if matches!(func.as_ref(), AetherExpr::Var { var, .. } if var.binder == Some(resume)) => {
            !args.iter().any(mentions)
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            !mentions(rhs) && last(body)
        }
        AetherExpr::Case { scrutinee, alts, .. } => {
            !mentions(scrutinee)
                && alts
                    .iter()
                    .all(|alt| !alt.guard.as_ref().is_some_and(mentions) && last(&alt.rhs))
        }
        AetherExpr::Dup { var, body, .. }
        | AetherExpr::Drop { var, body, .. }
        | AetherExpr::FreeShell { var, body, .. } => var.binder != Some(resume) && last(body),
        AetherExpr::DropSpecialized { unique_body, shared_body, .. } => {
            last(unique_body) && last(shared_body)
        }
        other => !mentions(other),
    }
}
