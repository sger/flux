//! Aether Phase 7: Reuse token insertion.
//!
//! Scans post-dup/drop Core IR for `Drop(x, body)` where body contains
//! a `Con(tag, fields)` of compatible shape. Transforms the pattern to
//! `Reuse(x, tag, fields)` which reuses x's allocation if uniquely owned.

use crate::core::{CoreBinderId, CoreExpr, CorePat, CoreTag, CoreVarRef};

use super::analysis::use_counts;
use super::into_constructor_shape_for_tag;

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
    rewrite_with_pat_ctx(expr, None, None)
}

fn rewrite_with_pat_ctx(
    expr: CoreExpr,
    pat_binders: Option<&[Option<CoreBinderId>]>,
    pat_tag: Option<&CoreTag>,
) -> CoreExpr {
    match expr {
        CoreExpr::Drop { var, body, span } => {
            let body = rewrite_with_pat_ctx(*body, pat_binders, pat_tag);
            if matches!(
                &body,
                CoreExpr::Reuse { token, .. } if token.binder == var.binder
            ) {
                return body;
            }
            // Try the direct pattern: Drop { var, Con { .. } }
            if let Some(result) = try_reuse_direct(&var, body.clone(), span, pat_binders, pat_tag)
            {
                return result;
            }
            // Try the spine pattern: Drop { var, Let/Drop chain ending in Con { .. } }
            if let Some(result) = try_reuse_in_spine(&var, body.clone(), span, pat_binders, pat_tag)
            {
                return result;
            }
            // Try a branch-sensitive pattern: Drop { var, Let* / Case { ... } }
            // where only some branches rebuild a compatible constructor.
            if let Some(result) =
                try_reuse_in_branching_case(&var, body.clone(), span, pat_binders, pat_tag)
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
            rhs: Box::new(rewrite_with_pat_ctx(*rhs, pat_binders, pat_tag)),
            body: Box::new(rewrite_with_pat_ctx(*body, pat_binders, pat_tag)),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(rewrite_with_pat_ctx(*rhs, pat_binders, pat_tag)),
            body: Box::new(rewrite_with_pat_ctx(*body, pat_binders, pat_tag)),
            span,
        },
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(rewrite_with_pat_ctx(*body, None, None)),
            span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(rewrite_with_pat_ctx(*func, pat_binders, pat_tag)),
            args: args
                .into_iter()
                .map(|a| rewrite_with_pat_ctx(a, pat_binders, pat_tag))
                .collect(),
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(rewrite_with_pat_ctx(*scrutinee, None, None)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    // Extract pattern field binder IDs for reuse specialization
                    let alt_pat_binders = pat_field_binder_ids(&alt.pat);
                    let alt_pat_tag = match &alt.pat {
                        CorePat::Con { tag, .. } => Some(tag),
                        _ => None,
                    };
                    alt.rhs = rewrite_with_pat_ctx(alt.rhs, alt_pat_binders.as_deref(), alt_pat_tag);
                    alt.guard = alt.guard.map(|g| rewrite_with_pat_ctx(g, None, None));
                    alt
                })
                .collect(),
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| rewrite_with_pat_ctx(f, pat_binders, pat_tag))
                .collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| rewrite_with_pat_ctx(a, pat_binders, pat_tag))
                .collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(rewrite_with_pat_ctx(*value, pat_binders, pat_tag)),
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
                .map(|a| rewrite_with_pat_ctx(a, pat_binders, pat_tag))
                .collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(rewrite_with_pat_ctx(*body, pat_binders, pat_tag)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = rewrite_with_pat_ctx(h.body, None, None);
                    h
                })
                .collect(),
            span,
        },
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(rewrite_with_pat_ctx(*body, pat_binders, pat_tag)),
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
                .map(|f| rewrite_with_pat_ctx(f, pat_binders, pat_tag))
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
            // The shared branch no longer owns the outer scrutinee token. Reusing
            // an unrelated inner drop under the outer pattern context can produce
            // invalid field-mask assumptions for masked ADT/list updates, so keep
            // this branch conservative and only apply context-free rewrites there.
            let shared_body = rewrite_with_pat_ctx(*shared_body, None, None);
            let unique_body =
                insert_reuse_in_unique(&scrutinee, *unique_body, pat_binders, pat_tag);
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
    pat_tag: Option<&CoreTag>,
) -> Option<CoreExpr> {
    let (tag, fields, span) = into_constructor_shape_for_tag(body, pat_tag)?;
    if is_heap_tag(&tag) && var_not_free_in_fields(var, &fields) {
        let field_mask = pat_binders.and_then(|pb| compute_field_mask(&fields, pb));
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
    pat_tag: Option<&CoreTag>,
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
                ctor_like => {
                    if let Some((tag, fields, span)) =
                        into_constructor_shape_for_tag(ctor_like.clone(), pat_tag)
                    {
                        if is_heap_tag(&tag) && var_not_free_in_fields(var, &fields) {
                            let field_mask =
                                pat_binders.and_then(|pb| compute_field_mask(&fields, pb));
                            return Some(CoreExpr::Let {
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
                            });
                        }
                    }

                    match ctor_like {
                        // Recurse into nested lets or drops
                        CoreExpr::Let { .. } | CoreExpr::Drop { .. } => {
                            let inner = try_reuse_in_spine(
                                var,
                                ctor_like,
                                _drop_span,
                                pat_binders,
                                pat_tag,
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
            }
        }
        CoreExpr::Drop {
            var: inner_var,
            body: inner_body,
            span: drop_span,
        } => {
            // Keep the inner Drop, try reuse in its body
            match *inner_body {
                ctor_like => {
                    if let Some((tag, fields, span)) =
                        into_constructor_shape_for_tag(ctor_like.clone(), pat_tag)
                    {
                        if is_heap_tag(&tag) && var_not_free_in_fields(var, &fields) {
                            let field_mask =
                                pat_binders.and_then(|pb| compute_field_mask(&fields, pb));
                            return Some(CoreExpr::Drop {
                                var: inner_var,
                                body: Box::new(CoreExpr::Reuse {
                                    token: *var,
                                    tag,
                                    fields,
                                    field_mask,
                                    span,
                                }),
                                span: drop_span,
                            });
                        }
                    }

                    match ctor_like {
                        // Recurse into nested lets or drops
                        CoreExpr::Let { .. } | CoreExpr::Drop { .. } => {
                            let result = try_reuse_in_spine(
                                var,
                                ctor_like,
                                _drop_span,
                                pat_binders,
                                pat_tag,
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
            }
        }
        _ => None,
    }
}

/// Branch-sensitive reuse: push `Drop(var)` through a let spine into a `Case`
/// when the dropped token is dead before the branch. Each branch then gets its
/// own `Drop(var, rhs)` wrapper, allowing rebuilding branches to become `Reuse`
/// while non-rebuilding branches keep the explicit drop.
fn try_reuse_in_branching_case(
    var: &CoreVarRef,
    body: CoreExpr,
    drop_span: crate::diagnostics::position::Span,
    pat_binders: Option<&[Option<CoreBinderId>]>,
    pat_tag: Option<&CoreTag>,
) -> Option<CoreExpr> {
    let binder_id = var.binder?;

    match body {
        CoreExpr::Let {
            var: let_var,
            rhs,
            body: let_body,
            span: let_span,
        } => {
            if use_counts(&rhs).contains_key(&binder_id) {
                return None;
            }
            let inner = try_reuse_in_branching_case(
                var,
                *let_body,
                drop_span,
                pat_binders,
                pat_tag,
            )?;
            Some(CoreExpr::Let {
                var: let_var,
                rhs,
                body: Box::new(inner),
                span: let_span,
            })
        }
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => {
            if use_counts(&scrutinee).contains_key(&binder_id)
                || alts.iter().any(|alt| {
                    alt.guard
                        .as_ref()
                        .is_some_and(|guard| use_counts(guard).contains_key(&binder_id))
                })
            {
                return None;
            }

            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = rewrite_with_pat_ctx(
                        CoreExpr::Drop {
                            var: *var,
                            body: Box::new(alt.rhs),
                            span: drop_span,
                        },
                        pat_binders,
                        pat_tag,
                    );
                    alt
                })
                .collect();

            Some(CoreExpr::Case {
                scrutinee,
                alts,
                span,
            })
        }
        _ => None,
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
    pat_binders: Option<&[Option<CoreBinderId>]>,
    pat_tag: Option<&CoreTag>,
) -> CoreExpr {
    match body {
        // Direct constructor shape — convert to Reuse
        ctor_like => {
            if let Some((tag, fields, span)) =
                into_constructor_shape_for_tag(ctor_like.clone(), pat_tag)
                && is_heap_tag(&tag)
            {
                let field_mask = pat_binders.and_then(|pb| compute_field_mask(&fields, pb));
                return CoreExpr::Reuse {
                    token: *scrutinee,
                    tag,
                    fields,
                    field_mask,
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
            body: Box::new(insert_reuse_in_unique(scrutinee, *let_body, pat_binders, pat_tag)),
            span,
        },
        // Walk through Dup spine
        CoreExpr::Dup {
            var,
            body: dup_body,
            span,
        } => CoreExpr::Dup {
            var,
            body: Box::new(insert_reuse_in_unique(scrutinee, *dup_body, pat_binders, pat_tag)),
            span,
        },
        // Walk through Drop spine
        CoreExpr::Drop {
            var,
            body: drop_body,
            span,
        } => {
            let body = insert_reuse_in_unique(scrutinee, *drop_body, pat_binders, pat_tag);
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
                    alt.rhs =
                        insert_reuse_in_unique(scrutinee, alt.rhs, pat_binders, pat_tag);
                    alt.guard = alt.guard.map(|g| rewrite_with_pat_ctx(g, None, None));
                    alt
                })
                .collect(),
            span,
        },
        // No compatible Con found — return body unchanged, recurse normally
        other => rewrite_with_pat_ctx(other, pat_binders, pat_tag),
            }
        }
    }
}
