//! Aether Phase 7: Reuse token insertion.
//!
//! Scans post-dup/drop Core IR for `Drop(x, body)` where body builds a
//! `Con(tag, fields)`, and turns it into `Reuse(x, tag, fields)`, which reuses
//! x's allocation if uniquely owned. Only a value matched against `tag` is a
//! token: the cell is written in place with no size check, so any other cell
//! may be too small.

use crate::core::{CoreBinderId, CorePat, CoreTag, CoreVarRef};
use crate::diagnostics::position::Span;
use crate::shared::opt_bisect;

use super::analysis::ArmShape;
use super::reuse_analysis::rewrite_drop_body;
use super::{AetherExpr, into_constructor_shape_for_tag_aether};

type CoreExpr = AetherExpr;

/// Insert reuse tokens into a Core IR expression.
///
/// Walks the tree looking for `Drop { var, body }` where `var` was matched
/// against a constructor pattern and `body` (or the first non-Drop/non-Let
/// expression in body's spine) builds the same constructor. Replaces that
/// `Con` with a `Reuse` of `var`'s allocation, taken at runtime if unique.
pub fn insert_reuse(expr: CoreExpr) -> CoreExpr {
    rewrite(expr)
}

/// Insert reuse tokens into a backend-only Aether expression.
pub fn insert_reuse_aether(expr: CoreExpr) -> CoreExpr {
    insert_reuse(expr)
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
    rewrite_with_ctx(expr, None, None)
}

fn rewrite_with_ctx(
    expr: CoreExpr,
    arm: Option<&ArmShape<'_>>,
    blocked_outer_token: Option<CoreBinderId>,
) -> CoreExpr {
    match expr {
        CoreExpr::Drop { var, body, span } => {
            let body = rewrite_with_ctx(*body, arm, blocked_outer_token);
            let (pat_binders, pat_tag) = ArmShape::matched_by(arm, var.binder);
            if let CoreExpr::Reuse { token, .. } = &body
                && token.binder == var.binder
                && token.binder.is_some()
            {
                return body;
            }
            // A bisected run (0190 §13) may have to keep the drop, so it keeps
            // the body as it was; other runs do not pay for the copy.
            let before = opt_bisect::is_active().then(|| body.clone());
            let rewritten =
                rewrite_drop_body(&var, body, span, pat_binders, pat_tag, blocked_outer_token);
            if rewritten.reused {
                match before {
                    Some(before)
                        if !opt_bisect::allow("reuse", || {
                            "reuse a dropped cell for a new one".to_string()
                        }) =>
                    {
                        return CoreExpr::Drop {
                            var,
                            body: Box::new(before),
                            span,
                        };
                    }
                    _ => return rewritten.expr,
                }
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
            rhs: Box::new(rewrite_with_ctx(*rhs, arm, blocked_outer_token)),
            body: Box::new(rewrite_with_ctx(*body, arm, blocked_outer_token)),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(rewrite_with_ctx(*rhs, arm, blocked_outer_token)),
            body: Box::new(rewrite_with_ctx(*body, arm, blocked_outer_token)),
            span,
        },
        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => CoreExpr::LetRecGroup {
            bindings: bindings
                .into_iter()
                .map(|(var, rhs)| {
                    (
                        var,
                        Box::new(rewrite_with_ctx(*rhs, arm, blocked_outer_token)),
                    )
                })
                .collect(),
            body: Box::new(rewrite_with_ctx(*body, arm, blocked_outer_token)),
            span,
        },
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
            body: Box::new(rewrite_with_ctx(*body, None, None)),
            span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(rewrite_with_ctx(*func, arm, blocked_outer_token)),
            args: args
                .into_iter()
                .map(|a| rewrite_with_ctx(a, arm, blocked_outer_token))
                .collect(),
            span,
        },
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            guarded_borrowed_args,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(rewrite_with_ctx(*func, arm, blocked_outer_token)),
            args: args
                .into_iter()
                .map(|a| rewrite_with_ctx(a, arm, blocked_outer_token))
                .collect(),
            arg_modes,
            guarded_borrowed_args,
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => {
            let matched = match scrutinee.as_ref() {
                CoreExpr::Var { var, .. } => var.binder,
                _ => None,
            };
            CoreExpr::Case {
                scrutinee: Box::new(rewrite_with_ctx(*scrutinee, None, None)),
                alts: alts
                    .into_iter()
                    .map(|mut alt| {
                        // Extract pattern field binder IDs for reuse specialization
                        let alt_pat_binders = pat_field_binder_ids(&alt.pat);
                        let alt_arm = match (matched, &alt.pat) {
                            (Some(scrutinee), CorePat::Con { tag, .. }) => Some(ArmShape {
                                scrutinee,
                                binders: alt_pat_binders.as_deref(),
                                tag,
                                outer: arm,
                            }),
                            _ => None,
                        };
                        alt.rhs = rewrite_with_ctx(
                            alt.rhs,
                            alt_arm.as_ref().or(arm),
                            blocked_outer_token,
                        );
                        alt.guard = alt.guard.map(|g| rewrite_with_ctx(g, None, None));
                        alt
                    })
                    .collect(),
                join_ty,
                span,
            }
        }
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| rewrite_with_ctx(f, arm, blocked_outer_token))
                .collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| rewrite_with_ctx(a, arm, blocked_outer_token))
                .collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(rewrite_with_ctx(*value, arm, blocked_outer_token)),
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
                .map(|a| rewrite_with_ctx(a, arm, blocked_outer_token))
                .collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            parameter,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(rewrite_with_ctx(*body, arm, blocked_outer_token)),
            effect,
            parameter: parameter.map(|p| Box::new(rewrite_with_ctx(*p, arm, blocked_outer_token))),
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = rewrite_with_ctx(h.body, None, None);
                    h
                })
                .collect(),
            span,
        },
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(rewrite_with_ctx(*body, arm, blocked_outer_token)),
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
                .map(|f| rewrite_with_ctx(f, arm, blocked_outer_token))
                .collect(),
            field_mask,
            span,
        },
        // DropSpecialized (Perceus Section 2.4): on the unique path the
        // scrutinee is uniquely owned, so a Con of the constructor it matched
        // reuses its cell without a check.
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => {
            let shared_body = rewrite_with_ctx(*shared_body, arm, scrutinee.binder);
            let unique_body = reuse_on_unique_path(&scrutinee, *unique_body, arm, span);
            CoreExpr::DropSpecialized {
                scrutinee,
                unique_body: Box::new(unique_body),
                shared_body: Box::new(shared_body),
                span,
            }
        }
        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(rewrite_with_ctx(*object, arm, blocked_outer_token)),
            member,
            span,
        },
        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(rewrite_with_ctx(*object, arm, blocked_outer_token)),
            index,
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

/// The unique path of a `DropSpecialized`: a Con on its spine reuses the
/// matched cell. When the walk finds none, the new cell may be bound by a
/// `let` and passed to a wrapper; the plain-drop rewrite finds that case, so
/// splitting the arm does not lose the reuse it had before.
fn reuse_on_unique_path(
    scrutinee: &CoreVarRef,
    body: CoreExpr,
    arm: Option<&ArmShape<'_>>,
    span: Span,
) -> CoreExpr {
    let (pat_binders, pat_tag) = ArmShape::matched_by(arm, scrutinee.binder);
    let original = body.clone();
    let walked = insert_reuse_in_unique(scrutinee, body, pat_tag, arm);
    // drop_spec keeps the cell out of the unique path, so any use of it now
    // is a Reuse the walk put there.
    let reused = scrutinee
        .binder
        .is_some_and(|id| super::analysis::use_counts_aether(&walked).contains_key(&id));
    if reused {
        return walked;
    }
    let rewritten = rewrite_drop_body(
        scrutinee,
        rewrite_with_ctx(original, arm, None),
        span,
        pat_binders,
        pat_tag,
        None,
    );
    // The plain-drop rewrite has every arm that does not reuse the cell drop
    // it, which is right under a plain drop, where the fields were copied
    // first. Here drop_spec took those copies out, so that drop would release
    // the fields a second time.
    let releases = scrutinee
        .binder
        .is_some_and(|id| super::insert::expr_drops_binder(&rewritten.expr, id));
    if rewritten.reused && !releases {
        rewritten.expr
    } else {
        walked
    }
}

/// Perceus drop-reuse specialization (Section 2.4, Fig 1e-1g).
///
/// On the unique path of a `DropSpecialized`, the scrutinee is guaranteed
/// uniquely owned. Walk the body spine looking for a `Con` of the matched
/// constructor and convert it to `Reuse`: the reuse always succeeds since
/// the uniqueness check already passed. Another constructor may not fit.
fn insert_reuse_in_unique(
    scrutinee: &CoreVarRef,
    body: CoreExpr,
    pat_tag: Option<&CoreTag>,
    arm: Option<&ArmShape<'_>>,
) -> CoreExpr {
    if let Some(expected) = pat_tag
        && let Some((tag, fields, span)) =
            into_constructor_shape_for_tag_aether(body.clone(), pat_tag)
        && tag == *expected
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
    match body {
        // Walk through Let spine
        CoreExpr::Let {
            var,
            rhs,
            body: let_body,
            span,
        } => CoreExpr::Let {
            var,
            rhs,
            body: Box::new(insert_reuse_in_unique(scrutinee, *let_body, pat_tag, arm)),
            span,
        },
        // Walk through Dup spine
        CoreExpr::Dup {
            var,
            body: dup_body,
            span,
        } => CoreExpr::Dup {
            var,
            body: Box::new(insert_reuse_in_unique(scrutinee, *dup_body, pat_tag, arm)),
            span,
        },
        // Walk through Drop spine
        CoreExpr::Drop {
            var,
            body: drop_body,
            span,
        } => {
            let body = insert_reuse_in_unique(scrutinee, *drop_body, pat_tag, arm);
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
            join_ty,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(rewrite_with_ctx(*case_scrutinee, None, None)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = insert_reuse_in_unique(scrutinee, alt.rhs, pat_tag, arm);
                    alt.guard = alt.guard.map(|g| rewrite_with_ctx(g, None, None));
                    alt
                })
                .collect(),
            join_ty,
            span,
        },
        // No Con of the matched constructor on the spine: rewrite the rest as usual.
        other => rewrite_with_ctx(other, arm, None),
    }
}

#[cfg(test)]
mod tests {
    use crate::aether::borrow_infer::BorrowMode;
    use crate::aether::{AetherAlt as CoreAlt, AetherExpr as CoreExpr};
    use crate::core::{CoreBinder, CoreBinderId, CorePat, CoreTag, CoreVarRef};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    fn v(binder: CoreBinder) -> CoreExpr {
        CoreExpr::bound_var(binder, Span::default())
    }

    fn reuses_of(expr: &CoreExpr, token: CoreBinderId) -> usize {
        match expr {
            CoreExpr::Reuse {
                token: t, fields, ..
            } => {
                usize::from(t.binder == Some(token))
                    + fields.iter().map(|f| reuses_of(f, token)).sum::<usize>()
            }
            CoreExpr::Let { rhs, body, .. } => reuses_of(rhs, token) + reuses_of(body, token),
            CoreExpr::Con { fields: args, .. } | CoreExpr::AetherCall { args, .. } => {
                args.iter().map(|a| reuses_of(a, token)).sum()
            }
            CoreExpr::Case { alts, .. } => alts.iter().map(|alt| reuses_of(&alt.rhs, token)).sum(),
            CoreExpr::DropSpecialized { unique_body, .. } => reuses_of(unique_body, token),
            CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => reuses_of(body, token),
            _ => 0,
        }
    }

    fn drops_of(expr: &CoreExpr, token: CoreBinderId) -> usize {
        match expr {
            CoreExpr::Drop { var, body, .. } => {
                usize::from(var.binder == Some(token)) + drops_of(body, token)
            }
            CoreExpr::Let { rhs, body, .. } => drops_of(rhs, token) + drops_of(body, token),
            CoreExpr::Case { alts, .. } => alts.iter().map(|alt| drops_of(&alt.rhs, token)).sum(),
            CoreExpr::DropSpecialized { unique_body, .. } => drops_of(unique_body, token),
            CoreExpr::Dup { body, .. } => drops_of(body, token),
            _ => 0,
        }
    }

    /// `[y | ys] -> Pair2([y | acc], ys)` once drop_spec has split it: the
    /// unique path holds `xs`'s cell alone, so the new cell bound in a `let`
    /// and passed to the wrapper reuses it, as it does under a plain `drop`.
    #[test]
    fn a_unique_path_reuses_a_cell_passed_through_a_let() {
        let mut interner = Interner::new();
        let pair2 = interner.intern("Pair2");
        let [xs, y, ys, acc, tmp] = [(1, "xs"), (2, "y"), (3, "ys"), (4, "acc"), (5, "tmp")]
            .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let wrap = |tail| CoreExpr::Let {
            var: tmp,
            rhs: Box::new(CoreExpr::Con {
                tag: CoreTag::Cons,
                fields: vec![v(y), v(acc)],
                span: Span::default(),
            }),
            body: Box::new(CoreExpr::AetherCall {
                func: Box::new(CoreExpr::Var {
                    var: CoreVarRef::unresolved(pair2),
                    span: Span::default(),
                }),
                args: vec![v(tmp), tail],
                arg_modes: vec![BorrowMode::Owned, BorrowMode::Owned],
                guarded_borrowed_args: vec![false, false],
                span: Span::default(),
            }),
            span: Span::default(),
        };
        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(y), CorePat::Var(ys)],
                },
                guard: None,
                rhs: CoreExpr::DropSpecialized {
                    scrutinee: CoreVarRef::resolved(&xs),
                    unique_body: Box::new(wrap(v(ys))),
                    shared_body: Box::new(wrap(CoreExpr::Dup {
                        var: CoreVarRef::resolved(&ys),
                        body: Box::new(v(ys)),
                        span: Span::default(),
                    })),
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };

        let rewritten = super::insert_reuse(expr);
        assert_eq!(reuses_of(&rewritten, xs.id), 1, "{rewritten:?}");
    }

    /// `[y | ys] -> if c { Pair2([y | acc], ys) } else { Pair2(acc, ys) }`
    /// once split: only one arm can reuse `xs`. The other still owns `y` and
    /// `ys` with no copy of its own (drop_spec took the copies out), so
    /// releasing `xs` there would release them a second time.
    #[test]
    fn a_unique_path_never_releases_its_cell_in_an_arm_that_does_not_reuse_it() {
        let mut interner = Interner::new();
        let pair2 = interner.intern("Pair2");
        let [xs, y, ys, acc, tmp, c] = [
            (1, "xs"),
            (2, "y"),
            (3, "ys"),
            (4, "acc"),
            (5, "tmp"),
            (6, "c"),
        ]
        .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let pair = |first, second| CoreExpr::AetherCall {
            func: Box::new(CoreExpr::Var {
                var: CoreVarRef::unresolved(pair2),
                span: Span::default(),
            }),
            args: vec![first, second],
            arg_modes: vec![BorrowMode::Owned, BorrowMode::Owned],
            guarded_borrowed_args: vec![false, false],
            span: Span::default(),
        };
        let arms = |tail: &dyn Fn() -> CoreExpr| CoreExpr::Case {
            scrutinee: Box::new(v(c)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Lit(crate::core::CoreLit::Bool(true)),
                    guard: None,
                    rhs: CoreExpr::Let {
                        var: tmp,
                        rhs: Box::new(CoreExpr::Con {
                            tag: CoreTag::Cons,
                            fields: vec![v(y), v(acc)],
                            span: Span::default(),
                        }),
                        body: Box::new(pair(v(tmp), tail())),
                        span: Span::default(),
                    },
                    span: Span::default(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: pair(v(acc), tail()),
                    span: Span::default(),
                },
            ],
            join_ty: None,
            span: Span::default(),
        };
        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(y), CorePat::Var(ys)],
                },
                guard: None,
                rhs: CoreExpr::DropSpecialized {
                    scrutinee: CoreVarRef::resolved(&xs),
                    unique_body: Box::new(arms(&|| v(ys))),
                    shared_body: Box::new(arms(&|| CoreExpr::Dup {
                        var: CoreVarRef::resolved(&ys),
                        body: Box::new(v(ys)),
                        span: Span::default(),
                    })),
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };

        let rewritten = super::insert_reuse(expr);
        assert_eq!(drops_of(&rewritten, xs.id), 0, "{rewritten:?}");
    }

    fn con(tag: CoreTag, fields: Vec<CoreExpr>) -> CoreExpr {
        CoreExpr::Con {
            tag,
            fields,
            span: Span::default(),
        }
    }

    fn drop_in(var: CoreBinder, body: CoreExpr) -> CoreExpr {
        CoreExpr::Drop {
            var: CoreVarRef::resolved(&var),
            body: Box::new(body),
            span: Span::default(),
        }
    }

    fn case(scrutinee: CoreBinder, alts: Vec<(CorePat, CoreExpr)>) -> CoreExpr {
        CoreExpr::Case {
            scrutinee: Box::new(v(scrutinee)),
            alts: alts
                .into_iter()
                .map(|(pat, rhs)| CoreAlt {
                    pat,
                    guard: None,
                    rhs,
                    span: Span::default(),
                })
                .collect(),
            join_ty: None,
            span: Span::default(),
        }
    }

    /// The `Big` arm drops only `o`, whose cell may be smaller than a `Big`:
    /// building one in it overran the heap (audit M1).
    #[test]
    fn a_dead_binder_that_is_not_the_matched_value_keeps_its_drop() {
        let mut interner = Interner::new();
        let big = CoreTag::Named(interner.intern("Big"));
        let [b, o, a1, a2, a3] = [(1, "b"), (2, "o"), (3, "a1"), (4, "a2"), (5, "a3")]
            .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let expr = case(
            b,
            vec![(
                CorePat::Con {
                    tag: big.clone(),
                    fields: vec![CorePat::Var(a1), CorePat::Var(a2), CorePat::Var(a3)],
                },
                drop_in(o, con(big, vec![v(a1), v(a2), v(a3)])),
            )],
        );

        let rewritten = super::insert_reuse(expr);
        assert_eq!(reuses_of(&rewritten, o.id), 0, "{rewritten:?}");
        assert_eq!(drops_of(&rewritten, o.id), 1, "{rewritten:?}");
    }

    /// A literal arm says nothing about the shape of `o`'s cell: a one-field
    /// `Some` was reused as a two-field cons.
    #[test]
    fn an_arm_with_no_constructor_pattern_reuses_nothing() {
        let mut interner = Interner::new();
        let [n, o, rest] = [(1, "n"), (2, "o"), (3, "rest")]
            .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let expr = case(
            n,
            vec![
                (CorePat::Lit(crate::core::CoreLit::Int(0)), v(rest)),
                (
                    CorePat::Wildcard,
                    drop_in(o, con(CoreTag::Cons, vec![v(n), v(rest)])),
                ),
            ],
        );

        let rewritten = super::insert_reuse(expr);
        assert_eq!(reuses_of(&rewritten, o.id), 0, "{rewritten:?}");
    }

    #[test]
    fn the_matched_value_is_still_reused() {
        let mut interner = Interner::new();
        let [xs, h, t] = [(1, "xs"), (2, "h"), (3, "t")]
            .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let expr = case(
            xs,
            vec![(
                CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                drop_in(xs, con(CoreTag::Cons, vec![v(h), v(t)])),
            )],
        );

        let rewritten = super::insert_reuse(expr);
        assert_eq!(reuses_of(&rewritten, xs.id), 1, "{rewritten:?}");
    }

    /// `merge_by_key`'s shape: `xs` is matched outside a case on a bool, and
    /// still reuses its cell inside it.
    #[test]
    fn a_value_matched_by_an_outer_arm_is_reused_in_an_inner_case() {
        let mut interner = Interner::new();
        let [xs, h, t, c] = [(1, "xs"), (2, "h"), (3, "t"), (4, "c")]
            .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let expr = case(
            xs,
            vec![(
                CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                case(
                    c,
                    vec![
                        (
                            CorePat::Lit(crate::core::CoreLit::Bool(true)),
                            drop_in(xs, con(CoreTag::Cons, vec![v(h), v(t)])),
                        ),
                        (CorePat::Wildcard, v(xs)),
                    ],
                ),
            )],
        );

        let rewritten = super::insert_reuse(expr);
        assert_eq!(reuses_of(&rewritten, xs.id), 1, "{rewritten:?}");
    }

    /// A wrapper around a new cons, in an arm that matched a `Some`: the cons
    /// would be rebuilt in the one-field cell.
    #[test]
    fn a_forwarded_child_of_another_constructor_is_not_reused() {
        let mut interner = Interner::new();
        let pair2 = CoreTag::Named(interner.intern("Pair2"));
        let [o, y, acc, ys] = [(1, "o"), (2, "y"), (3, "acc"), (4, "ys")]
            .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let expr = case(
            o,
            vec![(
                CorePat::Con {
                    tag: CoreTag::Some,
                    fields: vec![CorePat::Var(y)],
                },
                drop_in(
                    o,
                    con(pair2, vec![con(CoreTag::Cons, vec![v(y), v(acc)]), v(ys)]),
                ),
            )],
        );

        let rewritten = super::insert_reuse(expr);
        assert_eq!(reuses_of(&rewritten, o.id), 0, "{rewritten:?}");
    }

    /// drop_spec's unique path holds a cons cell; a `Some` built there must
    /// not take it.
    #[test]
    fn a_unique_path_reuses_only_for_the_matched_constructor() {
        let mut interner = Interner::new();
        let [xs, h, t] = [(1, "xs"), (2, "h"), (3, "t")]
            .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let expr = case(
            xs,
            vec![(
                CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                CoreExpr::DropSpecialized {
                    scrutinee: CoreVarRef::resolved(&xs),
                    unique_body: Box::new(con(CoreTag::Some, vec![v(h)])),
                    shared_body: Box::new(con(CoreTag::Some, vec![v(h)])),
                    span: Span::default(),
                },
            )],
        );

        let rewritten = super::insert_reuse(expr);
        assert_eq!(reuses_of(&rewritten, xs.id), 0, "{rewritten:?}");
    }
}
