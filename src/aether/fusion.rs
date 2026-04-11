//! Aether Phase 9: Dup/Drop fusion (Perceus Section 2.3).
//!
//! After drop specialization creates unique/shared branches, matching
//! `Dup(x)` / `Drop(x)` pairs can be cancelled — even when non-adjacent.
//!
//! The spine-based algorithm collects all leading Dup/Drop operations on
//! a spine, cancels matching pairs regardless of order, then re-wraps
//! the remaining operations. This handles patterns like:
//!
//! ```text
//! Dup(h); Dup(t); Drop(h); Drop(t); free(xs); body
//!   → free(xs); body                    // all pairs cancelled
//!
//! Dup(h); Dup(f); Drop(h); body
//!   → Dup(f); body                      // h cancelled, f kept
//! ```

use crate::aether::analysis::use_counts_aether;
use crate::core::{CoreBinder, CoreVarRef};
use crate::diagnostics::position::Span;
use crate::aether::AetherExpr;

type CoreExpr = AetherExpr;

/// Run dup/drop fusion on a Core IR expression.
pub fn fuse_dup_drop(expr: CoreExpr) -> CoreExpr {
    // Run multiple passes until no more fusions occur
    let mut result = expr;
    loop {
        let fused = fuse(result.clone());
        if exprs_equal(&fused, &result) {
            return fused;
        }
        result = fused;
    }
}

/// Run dup/drop fusion on a backend-only Aether expression.
pub fn fuse_dup_drop_aether(expr: CoreExpr) -> CoreExpr {
    fuse_dup_drop(expr)
}

/// Represents a collected Dup or Drop on a spine.
#[derive(Clone)]
enum RcOp {
    Dup(CoreVarRef, Span),
    Drop(CoreVarRef, Span),
}

#[derive(Clone)]
enum PrefixElem {
    Rc(RcOp),
    Let {
        var: CoreBinder,
        rhs: CoreExpr,
        span: Span,
    },
}

fn fuse(expr: CoreExpr) -> CoreExpr {
    match expr {
        // When we hit a Dup or Drop, collect the leading RC/pure-let prefix
        // and fuse matching pairs before re-wrapping.
        CoreExpr::Dup { .. } | CoreExpr::Drop { .. } => fuse_prefix(expr),

        // Recurse into all other forms
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(fuse(*rhs)),
            body: Box::new(fuse(*body)),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(fuse(*rhs)),
            body: Box::new(fuse(*body)),
            span,
        },
        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => CoreExpr::LetRecGroup {
            bindings: bindings
                .into_iter()
                .map(|(var, rhs)| (var, Box::new(fuse(*rhs))))
                .collect(),
            body: Box::new(fuse(*body)),
            span,
        },
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(fuse(*body)),
            span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(fuse(*func)),
            args: args.into_iter().map(fuse).collect(),
            span,
        },
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(fuse(*func)),
            args: args.into_iter().map(fuse).collect(),
            arg_modes,
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(fuse(*scrutinee)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = fuse(alt.rhs);
                    alt.guard = alt.guard.map(fuse);
                    alt
                })
                .collect(),
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields.into_iter().map(fuse).collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args.into_iter().map(fuse).collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(fuse(*value)),
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
            args: args.into_iter().map(fuse).collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(fuse(*body)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = fuse(h.body);
                    h
                })
                .collect(),
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
            fields: fields.into_iter().map(fuse).collect(),
            field_mask,
            span,
        },
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => CoreExpr::DropSpecialized {
            scrutinee,
            unique_body: Box::new(fuse(*unique_body)),
            shared_body: Box::new(fuse(*shared_body)),
            span,
        },
        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(fuse(*object)),
            member,
            span,
        },
        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(fuse(*object)),
            index,
            span,
        },
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,
    }
}

/// Collect all consecutive Dup/Drop operations on a spine, cancel matching
/// pairs, then re-wrap the remaining operations around the fused body.
///
/// This handles non-adjacent pairs like:
///   `Dup(a); Dup(b); Drop(a); Drop(c); body`
///   → `Dup(b); Drop(c); body`     (a cancelled)
fn fuse_prefix(expr: CoreExpr) -> CoreExpr {
    let mut elems: Vec<PrefixElem> = Vec::new();
    let mut cursor = expr;

    // Collect all leading Dup/Drop nodes, optionally crossing pure admin lets
    // whose RHS does not need ownership effects from the surrounding RC ops.
    loop {
        match cursor {
            CoreExpr::Dup { var, body, span } => {
                elems.push(PrefixElem::Rc(RcOp::Dup(var, span)));
                cursor = *body;
            }
            CoreExpr::Drop { var, body, span } => {
                elems.push(PrefixElem::Rc(RcOp::Drop(var, span)));
                cursor = *body;
            }
            CoreExpr::Let {
                var,
                rhs,
                body,
                span,
            } if is_safe_fusion_wrapper_rhs(&rhs) => {
                elems.push(PrefixElem::Let {
                    var,
                    rhs: fuse(*rhs),
                    span,
                });
                cursor = *body;
            }
            _ => break,
        }
    }

    let mut cancelled = vec![false; elems.len()];
    for i in 0..elems.len() {
        if cancelled[i] || !matches!(elems[i], PrefixElem::Rc(RcOp::Drop(_, _))) {
            continue;
        }
        let PrefixElem::Rc(RcOp::Drop(ref drop_var, _)) = elems[i] else {
            continue;
        };
        for j in 0..i {
            if cancelled[j] || !matches!(elems[j], PrefixElem::Rc(RcOp::Dup(_, _))) {
                continue;
            }
            if let PrefixElem::Rc(RcOp::Dup(ref dup_var, _)) = elems[j]
                && same_var(dup_var, drop_var)
                && can_cross_pure_wrappers(drop_var, &elems[j + 1..i])
            {
                cancelled[i] = true;
                cancelled[j] = true;
                break;
            }
        }
    }

    for i in 0..elems.len() {
        if cancelled[i] || !matches!(elems[i], PrefixElem::Rc(RcOp::Dup(_, _))) {
            continue;
        }
        let PrefixElem::Rc(RcOp::Dup(ref dup_var, _)) = elems[i] else {
            continue;
        };
        for j in 0..i {
            if cancelled[j] || !matches!(elems[j], PrefixElem::Rc(RcOp::Drop(_, _))) {
                continue;
            }
            if let PrefixElem::Rc(RcOp::Drop(ref drop_var, _)) = elems[j]
                && same_var(dup_var, drop_var)
                && can_cross_pure_wrappers(dup_var, &elems[j + 1..i])
            {
                cancelled[i] = true;
                cancelled[j] = true;
                break;
            }
        }
    }

    let body = fuse(cursor);

    let mut result = body;
    for (idx, elem) in elems.into_iter().enumerate().rev() {
        if cancelled[idx] {
            continue;
        }
        match elem {
            PrefixElem::Rc(RcOp::Dup(var, span)) => {
                result = CoreExpr::Dup {
                    var,
                    body: Box::new(result),
                    span,
                };
            }
            PrefixElem::Rc(RcOp::Drop(var, span)) => {
                result = CoreExpr::Drop {
                    var,
                    body: Box::new(result),
                    span,
                };
            }
            PrefixElem::Let { var, rhs, span } => {
                result = CoreExpr::Let {
                    var,
                    rhs: Box::new(rhs),
                    body: Box::new(result),
                    span,
                };
            }
        }
    }

    result
}

fn can_cross_pure_wrappers(target: &CoreVarRef, elems: &[PrefixElem]) -> bool {
    let Some(target_id) = target.binder else {
        return false;
    };
    elems.iter().all(|elem| match elem {
        PrefixElem::Rc(_) => true,
        PrefixElem::Let { rhs, .. } => !use_counts_aether(rhs).contains_key(&target_id),
    })
}

fn is_safe_fusion_wrapper_rhs(expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. }
        | CoreExpr::LetRec { .. }
        | CoreExpr::LetRecGroup { .. }
        | CoreExpr::DropSpecialized { .. }
        | CoreExpr::Lam { .. } => false,
        CoreExpr::Var { .. }
        | CoreExpr::Lit(_, _)
        | CoreExpr::Con { .. }
        | CoreExpr::Reuse { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::App { .. }
        | CoreExpr::AetherCall { .. }
        | CoreExpr::Let { .. }
        | CoreExpr::Case { .. }
        | CoreExpr::Return { .. }
        | CoreExpr::Dup { .. }
        | CoreExpr::Drop { .. }
        | CoreExpr::MemberAccess { .. }
        | CoreExpr::TupleField { .. } => true,
    }
}

fn same_var(a: &CoreVarRef, b: &CoreVarRef) -> bool {
    match (a.binder, b.binder) {
        (Some(a_id), Some(b_id)) => a_id == b_id,
        _ => false,
    }
}

/// Quick structural equality check (conservative — may return false for equal exprs).
fn exprs_equal(a: &CoreExpr, b: &CoreExpr) -> bool {
    // Use debug representation for comparison — not perfect but sufficient
    // for detecting when fusion made no changes.
    format!("{:?}", a) == format!("{:?}", b)
}

#[cfg(test)]
mod tests {
    use super::fuse_dup_drop;
    use crate::aether::AetherExpr as CoreExpr;
    use crate::core::{CoreBinder, CoreBinderId, CoreLit, CoreVarRef};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    fn binder(interner: &mut Interner, raw: u32, name: &str) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), interner.intern(name))
    }

    fn var_expr(binder: CoreBinder) -> CoreExpr {
        CoreExpr::Var {
            var: CoreVarRef::resolved(binder),
            span: Span::default(),
        }
    }

    fn count_matching<F>(expr: &CoreExpr, predicate: &F) -> usize
    where
        F: Fn(&CoreExpr) -> bool,
    {
        let here = usize::from(predicate(expr));
        match expr {
            CoreExpr::Lam { body, .. } => here + count_matching(body, predicate),
            CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
                here + count_matching(func, predicate)
                    + args
                        .iter()
                        .map(|arg| count_matching(arg, predicate))
                        .sum::<usize>()
            }
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                here + count_matching(rhs, predicate) + count_matching(body, predicate)
            }
            CoreExpr::LetRecGroup { bindings, body, .. } => {
                here + bindings
                    .iter()
                    .map(|(_, rhs)| count_matching(rhs, predicate))
                    .sum::<usize>()
                    + count_matching(body, predicate)
            }
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                here + count_matching(scrutinee, predicate)
                    + alts
                        .iter()
                        .map(|alt| {
                            count_matching(&alt.rhs, predicate)
                                + alt
                                    .guard
                                    .as_ref()
                                    .map(|guard| count_matching(guard, predicate))
                                    .unwrap_or(0)
                        })
                        .sum::<usize>()
            }
            CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => {
                here + fields
                    .iter()
                    .map(|field| count_matching(field, predicate))
                    .sum::<usize>()
            }
            CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
                here + args
                    .iter()
                    .map(|arg| count_matching(arg, predicate))
                    .sum::<usize>()
            }
            CoreExpr::Return { value, .. } => here + count_matching(value, predicate),
            CoreExpr::Handle { body, handlers, .. } => {
                here + count_matching(body, predicate)
                    + handlers
                        .iter()
                        .map(|handler| count_matching(&handler.body, predicate))
                        .sum::<usize>()
            }
            CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => {
                here + count_matching(body, predicate)
            }
            CoreExpr::DropSpecialized {
                unique_body,
                shared_body,
                ..
            } => {
                here + count_matching(unique_body, predicate)
                    + count_matching(shared_body, predicate)
            }
            CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
                here + count_matching(object, predicate)
            }
            CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => here,
        }
    }

    #[test]
    fn fuses_matching_rc_ops_across_safe_pure_lets() {
        let mut interner = Interner::new();
        let h = binder(&mut interner, 1, "h");
        let tmp = binder(&mut interner, 2, "tmp");

        let expr = CoreExpr::Dup {
            var: CoreVarRef::resolved(h),
            body: Box::new(CoreExpr::Let {
                var: tmp,
                rhs: Box::new(CoreExpr::Lit(CoreLit::Int(0), Span::default())),
                body: Box::new(CoreExpr::Drop {
                    var: CoreVarRef::resolved(h),
                    body: Box::new(var_expr(tmp)),
                    span: Span::default(),
                }),
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let fused = fuse_dup_drop(expr);
        assert_eq!(
            count_matching(&fused, &|expr| matches!(expr, CoreExpr::Dup { .. })),
            0
        );
        assert_eq!(
            count_matching(&fused, &|expr| matches!(expr, CoreExpr::Drop { .. })),
            0
        );
    }

    #[test]
    fn does_not_fuse_across_let_rhs_that_reads_target() {
        let mut interner = Interner::new();
        let h = binder(&mut interner, 1, "h");
        let tmp = binder(&mut interner, 2, "tmp");

        let expr = CoreExpr::Dup {
            var: CoreVarRef::resolved(h),
            body: Box::new(CoreExpr::Let {
                var: tmp,
                rhs: Box::new(var_expr(h)),
                body: Box::new(CoreExpr::Drop {
                    var: CoreVarRef::resolved(h),
                    body: Box::new(var_expr(tmp)),
                    span: Span::default(),
                }),
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let fused = fuse_dup_drop(expr);
        assert_eq!(
            count_matching(&fused, &|expr| matches!(expr, CoreExpr::Dup { .. })),
            1
        );
        assert_eq!(
            count_matching(&fused, &|expr| matches!(expr, CoreExpr::Drop { .. })),
            1
        );
    }
}
