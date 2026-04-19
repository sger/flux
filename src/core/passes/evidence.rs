/// Evidence-passing transformation for tail-resumptive effect handlers.
///
/// Rewrites `Handle { body, effect, handlers }` + `Perform { effect, op, args }`
/// into explicit evidence passing when all handlers are tail-resumptive:
///
/// ```text
/// Handle {
///     body: ... Perform(Eff, op, [a]) ...,
///     effect: Eff,
///     handlers: [{ op, resume, params: [x], body: resume(f(x)) }]
/// }
///   →
/// Let(ev_op, Lam([resume, x], resume(f(x))),
///   ... App(ev_op, [Lam([r], r), a]) ...)
/// ```
///
/// The `resume` parameter in each arm becomes an identity function since
/// tail-resumptive handlers always resume exactly once in tail position.
///
/// Non-tail-resumptive handlers are left unchanged — they require full
/// continuation capture at runtime.
use std::collections::HashMap;

use crate::{
    core::{CoreBinder, CoreBinderId, CoreExpr},
    diagnostics::position::Span,
    syntax::Identifier,
};

use super::tail_resumptive::is_core_handler_tail_resumptive;

/// Evidence-passing transformation entry point.
///
/// `next_id` is a mutable counter for allocating fresh `CoreBinderId`s
/// (should start above the program's max binder ID).
pub fn evidence_pass(expr: CoreExpr, next_id: &mut u32) -> CoreExpr {
    evidence_transform(expr, next_id, &HashMap::new())
}

/// Evidence map: maps (effect, operation) → CoreBinder of the evidence variable.
type EvidenceMap = HashMap<(Identifier, Identifier), CoreBinder>;

fn fresh_binder(next_id: &mut u32, name_hint: Identifier) -> CoreBinder {
    let id = *next_id;
    *next_id += 1;
    let sym = crate::syntax::symbol::Symbol::new(6_000_000 + id);
    let _ = name_hint;
    // Evidence variables are closures (handler functions) → BoxedRep.
    CoreBinder::with_rep(CoreBinderId(id), sym, crate::core::FluxRep::BoxedRep)
}

fn fresh_identity_binder(next_id: &mut u32) -> CoreBinder {
    let id = *next_id;
    *next_id += 1;
    let sym = crate::syntax::symbol::Symbol::new(6_000_000 + id);
    // Identity lambdas are closures → BoxedRep.
    CoreBinder::with_rep(CoreBinderId(id), sym, crate::core::FluxRep::BoxedRep)
}

/// Build an identity lambda `Lam([x], Var(x))` for the resume parameter.
fn make_identity_lam(next_id: &mut u32, span: Span) -> CoreExpr {
    let x = fresh_identity_binder(next_id);
    CoreExpr::Lam {
        params: vec![x],
        param_types: Vec::new(),
        result_ty: None,
        body: Box::new(CoreExpr::bound_var(&x, span)),
        span,
    }
}

/// Recursively transform an expression, rewriting TR Handle/Perform pairs.
fn evidence_transform(expr: CoreExpr, next_id: &mut u32, evidence: &EvidenceMap) -> CoreExpr {
    match expr {
        // Handle: check if all handlers are tail-resumptive.
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => {
            if is_core_handler_tail_resumptive(&handlers) {
                // Build evidence bindings for each handler arm.
                let mut new_evidence = evidence.clone();
                let mut ev_bindings: Vec<(CoreBinder, CoreExpr)> = Vec::new();

                for handler in &handlers {
                    let ev_binder = fresh_binder(next_id, handler.operation);

                    // Build the evidence lambda: Lam([resume, params...], body)
                    let mut lam_params = vec![handler.resume];
                    lam_params.extend_from_slice(&handler.params);

                    let ev_lam = CoreExpr::Lam {
                        params: lam_params,
                        param_types: Vec::new(),
                        result_ty: None,
                        body: Box::new(handler.body.clone()),
                        span: handler.span,
                    };

                    new_evidence.insert((effect, handler.operation), ev_binder);
                    ev_bindings.push((ev_binder, ev_lam));
                }

                // Transform the body with the new evidence map.
                // Directly-contained Perform nodes become App(evidence, ...).
                // Called functions may still Perform at runtime — the Handle
                // node is preserved as fallback for cross-function performs.
                let transformed_body = evidence_transform(*body, next_id, &new_evidence);

                // Wrap the Handle (preserved for runtime fallback) with evidence Let bindings.
                let handle_with_evidence = CoreExpr::Handle {
                    body: Box::new(transformed_body),
                    effect,
                    handlers,
                    span,
                };

                ev_bindings
                    .into_iter()
                    .rev()
                    .fold(handle_with_evidence, |acc, (binder, rhs)| CoreExpr::Let {
                        var: binder,
                        rhs: Box::new(rhs),
                        body: Box::new(acc),
                        span,
                    })
            } else {
                // Non-TR: recurse into body and handler bodies, but don't transform.
                CoreExpr::Handle {
                    body: Box::new(evidence_transform(*body, next_id, evidence)),
                    effect,
                    handlers: handlers
                        .into_iter()
                        .map(|mut h| {
                            h.body = evidence_transform(h.body, next_id, evidence);
                            h
                        })
                        .collect(),
                    span,
                }
            }
        }

        // Perform: check if evidence is available for this effect+operation.
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => {
            if let Some(ev_binder) = evidence.get(&(effect, operation)) {
                // Rewrite: Perform(effect, op, args) → App(ev_binder, [identity, args...])
                let identity = make_identity_lam(next_id, span);
                let mut call_args = vec![identity];
                call_args.extend(
                    args.into_iter()
                        .map(|a| evidence_transform(a, next_id, evidence)),
                );
                CoreExpr::App {
                    func: Box::new(CoreExpr::bound_var(ev_binder, span)),
                    args: call_args,
                    span,
                }
            } else {
                // No evidence — leave Perform unchanged, but transform args.
                CoreExpr::Perform {
                    effect,
                    operation,
                    args: args
                        .into_iter()
                        .map(|a| evidence_transform(a, next_id, evidence))
                        .collect(),
                    span,
                }
            }
        }

        // All other expressions: recurse into children.
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,

        CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body,
            span,
        } => CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body: Box::new(evidence_transform(*body, next_id, evidence)),
            span,
        },

        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(evidence_transform(*func, next_id, evidence)),
            args: args
                .into_iter()
                .map(|a| evidence_transform(a, next_id, evidence))
                .collect(),
            span,
        },
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(evidence_transform(*rhs, next_id, evidence)),
            body: Box::new(evidence_transform(*body, next_id, evidence)),
            span,
        },

        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(evidence_transform(*rhs, next_id, evidence)),
            body: Box::new(evidence_transform(*body, next_id, evidence)),
            span,
        },

        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => CoreExpr::LetRecGroup {
            bindings: bindings
                .into_iter()
                .map(|(b, rhs)| (b, Box::new(evidence_transform(*rhs, next_id, evidence))))
                .collect(),
            body: Box::new(evidence_transform(*body, next_id, evidence)),
            span,
        },

        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(evidence_transform(*scrutinee, next_id, evidence)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = evidence_transform(alt.rhs, next_id, evidence);
                    alt.guard = alt.guard.map(|g| evidence_transform(g, next_id, evidence));
                    alt
                })
                .collect(),
            join_ty,
            span,
        },

        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| evidence_transform(f, next_id, evidence))
                .collect(),
            span,
        },

        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| evidence_transform(a, next_id, evidence))
                .collect(),
            span,
        },

        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(evidence_transform(*value, next_id, evidence)),
            span,
        },

        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(evidence_transform(*object, next_id, evidence)),
            member,
            span,
        },

        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(evidence_transform(*object, next_id, evidence)),
            index,
            span,
        },
    }
}
