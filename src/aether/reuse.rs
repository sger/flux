//! Aether Phase 7: Reuse token insertion.
//!
//! Scans post-dup/drop Core IR for `Drop(x, body)` where body contains
//! a `Con(tag, fields)` of compatible shape. Transforms the pattern to
//! `Reuse(x, tag, fields)` which reuses x's allocation if uniquely owned.

use crate::core::{CoreBinderId, CoreExpr, CorePat, CoreTag, CoreVarRef};

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

/// Compute the field mask for a Reuse node by comparing constructor fields
/// against the pattern binders from the enclosing Case alt.
///
/// Bit `i` is set if field `i` differs from the corresponding pattern binder
/// (i.e., the field was changed and must be written). If all fields changed,
/// returns `None` (no benefit from selective writes).
fn compute_field_mask(
    fields: &[CoreExpr],
    pat_binders: &[Option<CoreBinderId>],
) -> Option<u64> {
    if fields.len() != pat_binders.len() {
        return None; // shape mismatch — write all
    }
    let mut mask: u64 = 0;
    let mut all_changed = true;
    for (i, (field_expr, pat_id)) in fields.iter().zip(pat_binders.iter()).enumerate() {
        let is_same = match (field_expr, pat_id) {
            (CoreExpr::Var { var, .. }, Some(pat_binder_id)) => {
                var.binder == Some(*pat_binder_id)
            }
            _ => false,
        };
        if !is_same {
            mask |= 1u64 << i;
        } else {
            all_changed = false;
        }
    }
    if all_changed {
        None // all fields changed — no benefit
    } else {
        Some(mask)
    }
}

fn rewrite(expr: CoreExpr) -> CoreExpr {
    rewrite_with_pat_ctx(expr, None)
}

fn rewrite_with_pat_ctx(
    expr: CoreExpr,
    pat_binders: Option<&[Option<CoreBinderId>]>,
) -> CoreExpr {
    match expr {
        CoreExpr::Drop { var, body, span } => {
            let body = rewrite_with_pat_ctx(*body, pat_binders);
            // Try the direct pattern: Drop { var, Con { .. } }
            if let Some(result) = try_reuse_direct(&var, body.clone(), span, pat_binders) {
                return result;
            }
            // Try the spine pattern: Drop { var, Let/Drop chain ending in Con { .. } }
            if let Some(result) =
                try_reuse_in_spine(&var, body.clone(), span, pat_binders)
            {
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
            rhs: Box::new(rewrite_with_pat_ctx(*rhs, pat_binders)),
            body: Box::new(rewrite_with_pat_ctx(*body, pat_binders)),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(rewrite_with_pat_ctx(*rhs, pat_binders)),
            body: Box::new(rewrite_with_pat_ctx(*body, pat_binders)),
            span,
        },
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(rewrite_with_pat_ctx(*body, None)),
            span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(rewrite_with_pat_ctx(*func, pat_binders)),
            args: args
                .into_iter()
                .map(|a| rewrite_with_pat_ctx(a, pat_binders))
                .collect(),
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(rewrite_with_pat_ctx(*scrutinee, None)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    // Extract pattern field binder IDs for reuse specialization
                    let alt_pat_binders = pat_field_binder_ids(&alt.pat);
                    let ctx = alt_pat_binders.as_deref();
                    alt.rhs = rewrite_with_pat_ctx(alt.rhs, ctx);
                    alt.guard = alt.guard.map(|g| rewrite_with_pat_ctx(g, None));
                    alt
                })
                .collect(),
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| rewrite_with_pat_ctx(f, pat_binders))
                .collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| rewrite_with_pat_ctx(a, pat_binders))
                .collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(rewrite_with_pat_ctx(*value, pat_binders)),
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
                .map(|a| rewrite_with_pat_ctx(a, pat_binders))
                .collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(rewrite_with_pat_ctx(*body, pat_binders)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = rewrite_with_pat_ctx(h.body, None);
                    h
                })
                .collect(),
            span,
        },
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(rewrite_with_pat_ctx(*body, pat_binders)),
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
                .map(|f| rewrite_with_pat_ctx(f, pat_binders))
                .collect(),
            field_mask,
            span,
        },
        // DropSpecialized — pass-through, recurse both branches
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => CoreExpr::DropSpecialized {
            scrutinee,
            unique_body: Box::new(rewrite_with_pat_ctx(*unique_body, pat_binders)),
            shared_body: Box::new(rewrite_with_pat_ctx(*shared_body, pat_binders)),
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
/// becomes `Reuse { token: x, tag, fields, field_mask, span }`.
fn try_reuse_direct(
    var: &CoreVarRef,
    body: CoreExpr,
    _drop_span: crate::diagnostics::position::Span,
    pat_binders: Option<&[Option<CoreBinderId>]>,
) -> Option<CoreExpr> {
    match body {
        CoreExpr::Con { tag, fields, span } => {
            if is_heap_tag(&tag) && var_not_free_in_fields(var, &fields) {
                let field_mask =
                    pat_binders.and_then(|pb| compute_field_mask(&fields, pb));
                Some(CoreExpr::Reuse {
                    token: *var,
                    tag,
                    fields,
                    field_mask,
                    span,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Spine pattern: searches through a spine of Let and Drop nodes for a Con
/// at the end that can be reused.
///
/// Handles patterns like:
/// ```text
/// Drop { var: x, body: Let { var: y, rhs, body: Con { .. } } }
/// Drop { var: x, body: Drop { var: z, body: Con { .. } } }
/// Drop { var: x, body: Let { var: y, rhs, body: Drop { var: z, body: Con { .. } } } }
/// ```
fn try_reuse_in_spine(
    var: &CoreVarRef,
    body: CoreExpr,
    _drop_span: crate::diagnostics::position::Span,
    pat_binders: Option<&[Option<CoreBinderId>]>,
) -> Option<CoreExpr> {
    let binder_id = var.binder?;

    match body {
        CoreExpr::Let {
            var: let_var,
            rhs,
            body: let_body,
            span: let_span,
        } => {
            // Check if the rhs uses the dropped var — if so, can't hoist past it
            let rhs_counts = use_counts(&rhs);
            if rhs_counts.contains_key(&binder_id) {
                return None;
            }

            // Try to find a Con at the end of the chain
            match *let_body {
                CoreExpr::Con { tag, fields, span } => {
                    if is_heap_tag(&tag) && var_not_free_in_fields(var, &fields) {
                        let field_mask =
                            pat_binders.and_then(|pb| compute_field_mask(&fields, pb));
                        Some(CoreExpr::Let {
                            var: let_var,
                            rhs,
                            body: Box::new(CoreExpr::Reuse {
                                token: *var,
                                tag,
                                fields,
                                field_mask,
                                span,
                            }),
                            span: let_span,
                        })
                    } else {
                        None
                    }
                }
                // Recurse into nested lets or drops
                CoreExpr::Let { .. } | CoreExpr::Drop { .. } => {
                    let inner = try_reuse_in_spine(
                        var,
                        *let_body.clone(),
                        _drop_span,
                        pat_binders,
                    )?;
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
        CoreExpr::Drop {
            var: inner_var,
            body: inner_body,
            span: drop_span,
        } => {
            // Keep the inner Drop, try reuse in its body
            match *inner_body {
                CoreExpr::Con { tag, fields, span } => {
                    if is_heap_tag(&tag) && var_not_free_in_fields(var, &fields) {
                        let field_mask =
                            pat_binders.and_then(|pb| compute_field_mask(&fields, pb));
                        Some(CoreExpr::Drop {
                            var: inner_var,
                            body: Box::new(CoreExpr::Reuse {
                                token: *var,
                                tag,
                                fields,
                                field_mask,
                                span,
                            }),
                            span: drop_span,
                        })
                    } else {
                        None
                    }
                }
                // Recurse into nested lets or drops
                CoreExpr::Let { .. } | CoreExpr::Drop { .. } => {
                    let result = try_reuse_in_spine(
                        var,
                        *inner_body.clone(),
                        _drop_span,
                        pat_binders,
                    )?;
                    Some(CoreExpr::Drop {
                        var: inner_var,
                        body: Box::new(result),
                        span: drop_span,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}
