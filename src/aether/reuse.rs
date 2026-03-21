//! Aether Phase 7: Reuse token insertion.
//!
//! Scans post-dup/drop Core IR for `Drop(x, body)` where body contains
//! a `Con(tag, fields)` of compatible shape. Transforms the pattern to
//! `Reuse(x, tag, fields)` which reuses x's allocation if uniquely owned.

use crate::core::{CoreBinderId, CoreExpr, CorePat, CoreTag, CoreVarRef};

use super::into_constructor_shape_for_tag;
use super::reuse_analysis::rewrite_drop_body;

/// Insert reuse tokens into a Core IR expression.
///
/// Walks the tree looking for `Drop { var, body }` where `body` (or the
/// first non-Drop/non-Let expression in body's spine) is a `Con` of
/// compatible shape. When found, replaces the `Con` with a `Reuse` node
/// that reuses the dropped value's allocation if uniquely owned at runtime.
pub fn insert_reuse(expr: CoreExpr) -> CoreExpr {
    rewrite(expr)
}

/// Extract field binder IDs from a constructor pattern, if applicable.
/// Returns `Some(vec)` where `vec[i]` is the binder ID for field `i`, or `None`
/// if the pattern field is not a simple variable binding.
fn pat_field_binder_ids(pat: &CorePat) -> Option<Vec<Option<CoreBinderId>>> {
    match pat {
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            let ids = fields
                .iter()
                .map(|f| match f {
                    CorePat::Var(binder) => Some(binder.id),
                    _ => None,
                })
                .collect();
            Some(ids)
        }
        _ => None,
    }
}

fn rewrite(expr: CoreExpr) -> CoreExpr {
    rewrite_with_ctx(expr, None, None, None)
}

fn rewrite_with_pat_ctx(
    expr: CoreExpr,
    pat_binders: Option<&[Option<CoreBinderId>]>,
    pat_tag: Option<&CoreTag>,
) -> CoreExpr {
    rewrite_with_ctx(expr, pat_binders, pat_tag, None)
}

fn rewrite_with_ctx(
    expr: CoreExpr,
    pat_binders: Option<&[Option<CoreBinderId>]>,
    pat_tag: Option<&CoreTag>,
    blocked_outer_token: Option<CoreBinderId>,
) -> CoreExpr {
    match expr {
        CoreExpr::Drop { var, body, span } => {
            let body = rewrite_with_ctx(*body, pat_binders, pat_tag, blocked_outer_token);
            if let CoreExpr::Reuse { token, .. } = &body
                && token.binder == var.binder
                && token.binder.is_some()
            {
                return body;
            }
            let rewritten =
                rewrite_drop_body(&var, body, span, pat_binders, pat_tag, blocked_outer_token);
            if rewritten.reused {
                return rewritten.expr;
            }
            CoreExpr::Drop {
                var,
                body: Box::new(rewritten.expr),
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
            rhs: Box::new(rewrite_with_ctx(*rhs, pat_binders, pat_tag, blocked_outer_token)),
            body: Box::new(rewrite_with_ctx(*body, pat_binders, pat_tag, blocked_outer_token)),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(rewrite_with_ctx(*rhs, pat_binders, pat_tag, blocked_outer_token)),
            body: Box::new(rewrite_with_ctx(*body, pat_binders, pat_tag, blocked_outer_token)),
            span,
        },
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(rewrite_with_ctx(*body, None, None, None)),
            span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(rewrite_with_ctx(*func, pat_binders, pat_tag, blocked_outer_token)),
            args: args
                .into_iter()
                .map(|a| rewrite_with_ctx(a, pat_binders, pat_tag, blocked_outer_token))
                .collect(),
            span,
        },
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(rewrite_with_ctx(*func, pat_binders, pat_tag, blocked_outer_token)),
            args: args
                .into_iter()
                .map(|a| rewrite_with_ctx(a, pat_binders, pat_tag, blocked_outer_token))
                .collect(),
            arg_modes,
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(rewrite_with_ctx(*scrutinee, None, None, None)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    // Extract pattern field binder IDs for reuse specialization
                    let alt_pat_binders = pat_field_binder_ids(&alt.pat);
                    let alt_pat_tag = match &alt.pat {
                        CorePat::Con { tag, .. } => Some(tag),
                        _ => None,
                    };
                    alt.rhs = rewrite_with_ctx(
                        alt.rhs,
                        alt_pat_binders.as_deref(),
                        alt_pat_tag,
                        blocked_outer_token,
                    );
                    alt.guard = alt.guard.map(|g| rewrite_with_ctx(g, None, None, None));
                    alt
                })
                .collect(),
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| rewrite_with_ctx(f, pat_binders, pat_tag, blocked_outer_token))
                .collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| rewrite_with_ctx(a, pat_binders, pat_tag, blocked_outer_token))
                .collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(rewrite_with_ctx(*value, pat_binders, pat_tag, blocked_outer_token)),
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
            args: args
                .into_iter()
                .map(|a| rewrite_with_ctx(a, pat_binders, pat_tag, blocked_outer_token))
                .collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(rewrite_with_ctx(*body, pat_binders, pat_tag, blocked_outer_token)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = rewrite_with_ctx(h.body, None, None, None);
                    h
                })
                .collect(),
            span,
        },
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(rewrite_with_ctx(*body, pat_binders, pat_tag, blocked_outer_token)),
            span,
        },
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => CoreExpr::Reuse {
            token,
            tag,
            fields: fields
                .into_iter()
                .map(|f| rewrite_with_ctx(f, pat_binders, pat_tag, blocked_outer_token))
                .collect(),
            field_mask,
            span,
        },
        // DropSpecialized — Perceus drop-reuse specialization (Section 2.4).
        // On the unique path, the scrutinee is guaranteed uniquely owned.
        // Insert Reuse nodes for compatible Con expressions — reuse always succeeds.
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => {
            let shared_body =
                rewrite_with_ctx(*shared_body, pat_binders, pat_tag, scrutinee.binder);
            let unique_body = insert_reuse_in_unique(&scrutinee, *unique_body, pat_tag);
            CoreExpr::DropSpecialized {
                scrutinee,
                unique_body: Box::new(unique_body),
                shared_body: Box::new(shared_body),
                span,
            }
        }
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

/// Perceus drop-reuse specialization (Section 2.4, Fig 1e-1g).
///
/// On the unique path of a `DropSpecialized`, the scrutinee is guaranteed
/// uniquely owned. Walk the body spine looking for a `Con` of compatible
/// shape and convert it to `Reuse` — the reuse always succeeds since
/// the uniqueness check already passed.
fn insert_reuse_in_unique(
    scrutinee: &CoreVarRef,
    body: CoreExpr,
    pat_tag: Option<&CoreTag>,
) -> CoreExpr {
    match body {
        // Direct constructor shape — convert to Reuse
        ctor_like => {
            if let Some((tag, fields, span)) =
                into_constructor_shape_for_tag(ctor_like.clone(), pat_tag)
                && is_heap_tag(&tag)
            {
                return CoreExpr::Reuse {
                    token: *scrutinee,
                    tag,
                    fields,
                    field_mask: None,
                    span,
                };
            }
            match ctor_like {
        // Walk through Let spine
        CoreExpr::Let {
            var,
            rhs,
            body: let_body,
            span,
        } => CoreExpr::Let {
            var,
            rhs,
            body: Box::new(insert_reuse_in_unique(scrutinee, *let_body, pat_tag)),
            span,
        },
        // Walk through Dup spine
        CoreExpr::Dup {
            var,
            body: dup_body,
            span,
        } => CoreExpr::Dup {
            var,
            body: Box::new(insert_reuse_in_unique(scrutinee, *dup_body, pat_tag)),
            span,
        },
        // Walk through Drop spine
        CoreExpr::Drop {
            var,
            body: drop_body,
            span,
        } => {
            let body = insert_reuse_in_unique(scrutinee, *drop_body, pat_tag);
            if var.binder == scrutinee.binder {
                body
            } else {
                CoreExpr::Drop {
                    var,
                    body: Box::new(body),
                    span,
                }
            }
        }
        // Recurse into branchy bodies while preserving the outer reuse token.
        CoreExpr::Case {
            scrutinee: case_scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(rewrite_with_pat_ctx(*case_scrutinee, None, None)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = insert_reuse_in_unique(scrutinee, alt.rhs, pat_tag);
                    alt.guard = alt.guard.map(|g| rewrite_with_pat_ctx(g, None, None));
                    alt
                })
                .collect(),
            span,
        },
        // No compatible Con found — return body unchanged, recurse normally
        other => rewrite_with_pat_ctx(other, None, pat_tag),
            }
        }
    }
}
