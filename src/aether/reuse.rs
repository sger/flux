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

use super::analysis::{ArmShape, pat_field_binder_ids};
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
            owned_after,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(rewrite_with_ctx(*func, arm, blocked_outer_token)),
            args: args
                .into_iter()
                .map(|a| rewrite_with_ctx(a, arm, blocked_outer_token))
                .collect(),
            arg_modes,
            guarded_borrowed_args,
            owned_after,
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
            owned_after,
            span,
        } => CoreExpr::Perform {
            effect,
            operation,
            args: args
                .into_iter()
                .map(|a| rewrite_with_ctx(a, arm, blocked_outer_token))
                .collect(),
            owned_after,
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
            let unique_body =
                reuse_on_unique_path(&scrutinee, *unique_body, arm, span, blocked_outer_token);
            let shared_releases = scrutinee
                .binder
                .is_some_and(|id| uses_shell_on_every_path(&shared_body, id, false));
            let Some(unique_body) = unique_body.filter(|_| shared_releases) else {
                // The shared body retains the original copies and release.
                // Without reuse on every unique path, it is the only body
                // that can release the shell without freeing moved fields.
                return rewrite_with_ctx(*shared_body, arm, blocked_outer_token);
            };
            let shared_body = rewrite_with_ctx(*shared_body, arm, scrutinee.binder);
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
    blocked_outer_token: Option<CoreBinderId>,
) -> Option<CoreExpr> {
    let (pat_binders, pat_tag) = ArmShape::matched_by(arm, scrutinee.binder);
    let original = body.clone();
    let walked = insert_reuse_in_unique(scrutinee, body, pat_tag, arm, blocked_outer_token);
    if scrutinee
        .binder
        .is_some_and(|id| uses_shell_on_every_path(&walked, id, true))
    {
        return Some(walked);
    }
    let rewritten = rewrite_drop_body(
        scrutinee,
        rewrite_with_ctx(original, arm, blocked_outer_token),
        span,
        pat_binders,
        pat_tag,
        blocked_outer_token,
    );
    // The plain-drop rewrite has every arm that does not reuse the cell drop
    // it, which is right under a plain drop, where the fields were copied
    // first. Here drop_spec took those copies out, so that drop would release
    // the fields a second time.
    if rewritten.reused
        && scrutinee
            .binder
            .is_some_and(|id| uses_shell_on_every_path(&rewritten.expr, id, true))
    {
        Some(rewritten.expr)
    } else {
        None
    }
}

/// A unique shell has no recursive release available once its fields move.
/// Count reuse along evaluation paths, rather than counting sites: one site
/// in a branch does not dispose of the shell on the other branch.
fn uses_shell_on_every_path(expr: &CoreExpr, token: CoreBinderId, reuse: bool) -> bool {
    fn sequence<'a>(
        children: impl IntoIterator<Item = &'a CoreExpr>,
        token: CoreBinderId,
        reuse: bool,
    ) -> Option<(u8, u8)> {
        children
            .into_iter()
            .try_fold((0u8, 0u8), |(min, max), child| {
                let (child_min, child_max) = counts(child, token, reuse)?;
                Some(((min + child_min).min(2), (max + child_max).min(2)))
            })
    }

    fn branches<'a>(
        children: impl IntoIterator<Item = &'a CoreExpr>,
        token: CoreBinderId,
        reuse: bool,
    ) -> Option<(u8, u8)> {
        children
            .into_iter()
            .try_fold(None::<(u8, u8)>, |joined, child| {
                let (min, max) = counts(child, token, reuse)?;
                Some(Some(match joined {
                    None => (min, max),
                    Some((lo, hi)) => (min.min(lo), max.max(hi)),
                }))
            })?
    }

    fn counts(expr: &CoreExpr, token: CoreBinderId, reuse: bool) -> Option<(u8, u8)> {
        match expr {
            CoreExpr::Var { var, .. } => (var.binder != Some(token)).then_some((0, 0)),
            CoreExpr::Lit(..) => Some((0, 0)),
            CoreExpr::Lam { .. } | CoreExpr::Handle { .. } | CoreExpr::Perform { .. } => {
                (!super::analysis::use_counts_aether(expr).contains_key(&token)).then_some((0, 0))
            }
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                sequence([rhs.as_ref(), body.as_ref()], token, reuse)
            }
            CoreExpr::LetRecGroup { bindings, body, .. } => sequence(
                bindings
                    .iter()
                    .map(|(_, rhs)| rhs.as_ref())
                    .chain([body.as_ref()]),
                token,
                reuse,
            ),
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                for guard in alts.iter().filter_map(|alt| alt.guard.as_ref()) {
                    if counts(guard, token, reuse)? != (0, 0) {
                        return None;
                    }
                }
                let (before_min, before_max) = counts(scrutinee, token, reuse)?;
                let (arm_min, arm_max) = branches(alts.iter().map(|alt| &alt.rhs), token, reuse)?;
                Some(((before_min + arm_min).min(2), (before_max + arm_max).min(2)))
            }
            CoreExpr::Dup { var, body, .. } => {
                if var.binder == Some(token) {
                    None
                } else {
                    counts(body, token, reuse)
                }
            }
            CoreExpr::Drop { var, body, .. } => {
                if var.binder == Some(token) {
                    if reuse {
                        return None;
                    }
                    let (min, max) = counts(body, token, reuse)?;
                    Some(((min + 1).min(2), (max + 1).min(2)))
                } else {
                    counts(body, token, reuse)
                }
            }
            CoreExpr::Reuse {
                token: reused,
                fields,
                ..
            } => {
                if !reuse && reused.binder == Some(token) {
                    return None;
                }
                let (min, max) = sequence(fields, token, reuse)?;
                let own = u8::from(reused.binder == Some(token));
                Some(((min + own).min(2), (max + own).min(2)))
            }
            CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
                sequence(fields, token, reuse)
            }
            CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
                sequence(std::iter::once(func.as_ref()).chain(args), token, reuse)
            }
            CoreExpr::Return { value, .. } => counts(value, token, reuse),
            CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
                counts(object, token, reuse)
            }
            CoreExpr::DropSpecialized {
                scrutinee,
                unique_body,
                shared_body,
                ..
            } => {
                if scrutinee.binder == Some(token) {
                    None
                } else {
                    branches([unique_body.as_ref(), shared_body.as_ref()], token, reuse)
                }
            }
        }
    }

    counts(expr, token, reuse) == Some((1, 1))
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
    blocked_outer_token: Option<CoreBinderId>,
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
            body: Box::new(insert_reuse_in_unique(
                scrutinee,
                *let_body,
                pat_tag,
                arm,
                blocked_outer_token,
            )),
            span,
        },
        // Walk through Dup spine
        CoreExpr::Dup {
            var,
            body: dup_body,
            span,
        } => CoreExpr::Dup {
            var,
            body: Box::new(insert_reuse_in_unique(
                scrutinee,
                *dup_body,
                pat_tag,
                arm,
                blocked_outer_token,
            )),
            span,
        },
        // Walk through Drop spine
        CoreExpr::Drop {
            var,
            body: drop_body,
            span,
        } => {
            let body =
                insert_reuse_in_unique(scrutinee, *drop_body, pat_tag, arm, blocked_outer_token);
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
                    alt.rhs = insert_reuse_in_unique(
                        scrutinee,
                        alt.rhs,
                        pat_tag,
                        arm,
                        blocked_outer_token,
                    );
                    alt.guard = alt.guard.map(|g| rewrite_with_ctx(g, None, None));
                    alt
                })
                .collect(),
            join_ty,
            span,
        },
        // No Con of the matched constructor on the spine: rewrite the rest as usual.
        other => rewrite_with_ctx(other, arm, blocked_outer_token),
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
                owned_after: Vec::new(),
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
                    shared_body: Box::new(CoreExpr::Dup {
                        var: CoreVarRef::resolved(&y),
                        body: Box::new(CoreExpr::Dup {
                            var: CoreVarRef::resolved(&ys),
                            body: Box::new(drop_in(xs, wrap(v(ys)))),
                            span: Span::default(),
                        }),
                        span: Span::default(),
                    }),
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
    /// Only one arm can reuse `xs`, so specialization falls back to the
    /// copied fields and a plain release instead of losing the other shell.
    #[test]
    fn a_forwarded_constructor_reused_on_one_path_keeps_the_other_release() {
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
            owned_after: Vec::new(),
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
                    rhs: drop_in(y, pair(v(acc), tail())),
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
                rhs: CoreExpr::Dup {
                    var: CoreVarRef::resolved(&y),
                    body: Box::new(CoreExpr::Dup {
                        var: CoreVarRef::resolved(&ys),
                        body: Box::new(drop_in(xs, arms(&|| v(ys)))),
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };

        let rewritten = specialize_and_reuse(expr);
        assert_eq!(reuses_of(&rewritten, xs.id), 1, "{rewritten:?}");
        assert_eq!(drops_of(&rewritten, xs.id), 1, "{rewritten:?}");
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

    fn copied_fields_release(
        xs: CoreBinder,
        h: CoreBinder,
        t: CoreBinder,
        body: CoreExpr,
    ) -> CoreExpr {
        let copy = |field: CoreBinder, body| CoreExpr::Dup {
            var: CoreVarRef::resolved(&field),
            body: Box::new(body),
            span: Span::default(),
        };
        case(
            xs,
            vec![(
                CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                copy(h, copy(t, drop_in(xs, body))),
            )],
        )
    }

    fn specialize_and_reuse(expr: CoreExpr) -> CoreExpr {
        let expr = crate::aether::drop_spec::specialize_drops(expr);
        let expr = crate::aether::fusion::fuse_dup_drop_aether(expr);
        super::insert_reuse(expr)
    }

    #[test]
    fn an_unreused_specialization_keeps_its_plain_release() {
        let mut interner = Interner::new();
        let [xs, h, t] = [(1, "xs"), (2, "h"), (3, "t")]
            .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let body = con(CoreTag::Named(interner.intern("Pair2")), vec![v(h), v(t)]);
        let rewritten = specialize_and_reuse(copied_fields_release(xs, h, t, body));
        let CoreExpr::Case { alts, .. } = &rewritten else {
            panic!("expected a case: {rewritten:?}");
        };
        assert!(
            !matches!(alts[0].rhs, CoreExpr::DropSpecialized { .. }),
            "{rewritten:?}"
        );
        assert_eq!(drops_of(&rewritten, xs.id), 1, "{rewritten:?}");
        assert_eq!(reuses_of(&rewritten, xs.id), 0, "{rewritten:?}");
        assert!(
            crate::aether::verify::verify_contract_aether(&rewritten).is_ok(),
            "{rewritten:?}"
        );
    }

    #[test]
    fn a_partially_reused_specialization_keeps_a_release_on_the_other_path() {
        let mut interner = Interner::new();
        let [xs, h, t, c] = [(1, "xs"), (2, "h"), (3, "t"), (4, "c")]
            .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let body = case(
            c,
            vec![
                (
                    CorePat::Lit(crate::core::CoreLit::Bool(true)),
                    drop_in(c, con(CoreTag::Cons, vec![v(h), v(t)])),
                ),
                (
                    CorePat::Wildcard,
                    con(CoreTag::Named(interner.intern("Pair2")), vec![v(h), v(t)]),
                ),
            ],
        );
        let rewritten = specialize_and_reuse(copied_fields_release(xs, h, t, body));
        let CoreExpr::Case { alts, .. } = &rewritten else {
            panic!("expected a case: {rewritten:?}");
        };
        assert!(
            !matches!(alts[0].rhs, CoreExpr::DropSpecialized { .. }),
            "{rewritten:?}"
        );
        assert_eq!(reuses_of(&rewritten, xs.id), 1, "{rewritten:?}");
        assert_eq!(drops_of(&rewritten, xs.id), 1, "{rewritten:?}");
        assert!(
            crate::aether::verify::verify_contract_aether(&rewritten).is_ok(),
            "{rewritten:?}"
        );
    }

    #[test]
    fn a_specialized_shared_path_copies_fields_before_its_release() {
        let mut interner = Interner::new();
        let [xs, h, t] = [(1, "xs"), (2, "h"), (3, "t")]
            .map(|(id, name)| CoreBinder::new(CoreBinderId(id), interner.intern(name)));
        let body = con(CoreTag::Cons, vec![v(h), v(t)]);
        let rewritten = specialize_and_reuse(copied_fields_release(xs, h, t, body));
        let CoreExpr::Case { alts, .. } = &rewritten else {
            panic!("expected a case: {rewritten:?}");
        };
        let CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } = &alts[0].rhs
        else {
            panic!("expected specialization: {rewritten:?}");
        };
        assert_eq!(reuses_of(unique_body, xs.id), 1, "{rewritten:?}");
        assert_eq!(drops_of(unique_body, xs.id), 0, "{rewritten:?}");
        let CoreExpr::Dup {
            var: first, body, ..
        } = shared_body.as_ref()
        else {
            panic!("expected first field copy: {rewritten:?}");
        };
        assert_eq!(first.binder, Some(h.id));
        let CoreExpr::Dup {
            var: second, body, ..
        } = body.as_ref()
        else {
            panic!("expected second field copy: {rewritten:?}");
        };
        assert_eq!(second.binder, Some(t.id));
        assert!(
            matches!(body.as_ref(), CoreExpr::Drop { var, .. } if var.binder == Some(xs.id)),
            "{rewritten:?}"
        );
        assert!(
            crate::aether::verify::verify_contract_aether(&rewritten).is_ok(),
            "{rewritten:?}"
        );
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
                    shared_body: Box::new(CoreExpr::Dup {
                        var: CoreVarRef::resolved(&h),
                        body: Box::new(drop_in(xs, con(CoreTag::Some, vec![v(h)]))),
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
            )],
        );

        let rewritten = super::insert_reuse(expr);
        assert_eq!(reuses_of(&rewritten, xs.id), 0, "{rewritten:?}");
        assert_eq!(drops_of(&rewritten, xs.id), 1, "{rewritten:?}");
    }
}
