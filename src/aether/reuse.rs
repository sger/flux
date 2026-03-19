//! Aether Phase 7: Reuse token insertion.
//!
//! Scans post-dup/drop Core IR for `Drop(x, body)` where body contains
//! a `Con(tag, fields)` of compatible shape. Transforms the pattern to
//! `Reuse(x, tag, fields)` which reuses x's allocation if uniquely owned.

use crate::core::{CoreExpr, CoreTag, CoreVarRef};

use super::analysis::use_counts;

/// Insert reuse tokens into a Core IR expression.
///
/// Walks the tree looking for `Drop { var, body }` where `body` (or the
/// first non-Drop/non-Let expression in body's spine) is a `Con` of
/// compatible shape. When found, replaces the `Con` with a `Reuse` node
/// that reuses the dropped value's allocation if uniquely owned at runtime.
pub fn insert_reuse(expr: CoreExpr) -> CoreExpr {
    rewrite(expr)
}

fn rewrite(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Drop { var, body, span } => {
            let body = rewrite(*body);
            // Try the direct pattern: Drop { var, Con { .. } }
            if let Some(result) = try_reuse_direct(&var, body.clone(), span) {
                return result;
            }
            // Try the let-chain pattern: Drop { var, Let { .., Con { .. } } }
            if let Some(result) = try_reuse_let_chain(&var, body.clone(), span) {
                return result;
            }
            CoreExpr::Drop {
                var,
                body: Box::new(body),
                span,
            }
        }
        // Recurse into all other forms
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(rewrite(*rhs)),
            body: Box::new(rewrite(*body)),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(rewrite(*rhs)),
            body: Box::new(rewrite(*body)),
            span,
        },
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(rewrite(*body)),
            span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(rewrite(*func)),
            args: args.into_iter().map(rewrite).collect(),
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(rewrite(*scrutinee)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = rewrite(alt.rhs);
                    alt.guard = alt.guard.map(rewrite);
                    alt
                })
                .collect(),
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields.into_iter().map(rewrite).collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args.into_iter().map(rewrite).collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(rewrite(*value)),
            span,
        },
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => CoreExpr::Perform {
            effect,
            operation,
            args: args.into_iter().map(rewrite).collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(rewrite(*body)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = rewrite(h.body);
                    h
                })
                .collect(),
            span,
        },
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(rewrite(*body)),
            span,
        },
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            span,
        } => CoreExpr::Reuse {
            token,
            tag,
            fields: fields.into_iter().map(rewrite).collect(),
            span,
        },
        // Atoms — no children
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,
    }
}

/// Check if a tag represents a heap-allocated constructor suitable for reuse.
///
/// Stack-allocated constructors (Nil, None) have no allocation to reuse.
fn is_heap_tag(tag: &CoreTag) -> bool {
    match tag {
        CoreTag::Cons | CoreTag::Some | CoreTag::Left | CoreTag::Right | CoreTag::Named(_) => true,
        CoreTag::Nil | CoreTag::None => false,
    }
}

/// Check that the dropped variable does NOT appear free in any of the Con's fields.
fn var_not_free_in_fields(var: &CoreVarRef, fields: &[CoreExpr]) -> bool {
    let binder_id = match var.binder {
        Some(id) => id,
        None => return true, // unresolved var can't match anything
    };
    for field in fields {
        let counts = use_counts(field);
        if counts.contains_key(&binder_id) {
            return false;
        }
    }
    true
}

/// Direct pattern: `Drop { var: x, body: Con { tag, fields, span } }`
/// becomes `Reuse { token: x, tag, fields, span }`.
fn try_reuse_direct(
    var: &CoreVarRef,
    body: CoreExpr,
    _drop_span: crate::diagnostics::position::Span,
) -> Option<CoreExpr> {
    match body {
        CoreExpr::Con { tag, fields, span } => {
            if is_heap_tag(&tag) && var_not_free_in_fields(var, &fields) {
                Some(CoreExpr::Reuse {
                    token: *var,
                    tag,
                    fields,
                    span,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Let-chain pattern:
/// ```text
/// Drop { var: x, body: Let { var: y, rhs, body: Con { tag, fields, span } } }
/// ```
/// becomes:
/// ```text
/// Let { var: y, rhs, body: Reuse { token: x, tag, fields, span } }
/// ```
///
/// Also handles chains of Lets ending in a Con.
fn try_reuse_let_chain(
    var: &CoreVarRef,
    body: CoreExpr,
    _drop_span: crate::diagnostics::position::Span,
) -> Option<CoreExpr> {
    match body {
        CoreExpr::Let {
            var: let_var,
            rhs,
            body: let_body,
            span: let_span,
        } => {
            // Check if the rhs uses the dropped var — if so, can't hoist past it
            let binder_id = var.binder?;
            let rhs_counts = use_counts(&rhs);
            if rhs_counts.contains_key(&binder_id) {
                return None;
            }

            // Try to find a Con at the end of the chain
            match *let_body {
                CoreExpr::Con { tag, fields, span } => {
                    if is_heap_tag(&tag) && var_not_free_in_fields(var, &fields) {
                        Some(CoreExpr::Let {
                            var: let_var,
                            rhs,
                            body: Box::new(CoreExpr::Reuse {
                                token: *var,
                                tag,
                                fields,
                                span,
                            }),
                            span: let_span,
                        })
                    } else {
                        None
                    }
                }
                // Recurse into nested lets
                CoreExpr::Let { .. } => {
                    let inner = try_reuse_let_chain(var, *let_body.clone(), _drop_span)?;
                    Some(CoreExpr::Let {
                        var: let_var,
                        rhs,
                        body: Box::new(inner),
                        span: let_span,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}
