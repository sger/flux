/// Case-of-known-constructor pass.
///
/// Reduces `Case(Con(tag, fields), alts)` and `Case(Lit(l), alts)` when the
/// scrutinee is a statically-known value.
///
/// Only unguarded arms are considered.  Nested constructor patterns in field
/// position are left alone (handled by a future pattern-compilation pass).
///
/// Examples:
/// ```text
/// Case(Con(Some, [Lit(42)]), [Con(Some, [Var(x)]) → x])  →  Lit(42)
/// Case(Lit(true), [Lit(true) → a, Wildcard → b])          →  a
/// ```
use crate::core::{CoreBinderId, CoreExpr, CoreLit, CorePat, CoreTag};

use super::helpers::{map_children, subst};

pub fn case_of_known_constructor(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => {
            let scrutinee = case_of_known_constructor(*scrutinee);
            let alts: Vec<_> = alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = case_of_known_constructor(alt.rhs);
                    alt.guard = alt.guard.map(case_of_known_constructor);
                    alt
                })
                .collect();
            match &scrutinee {
                CoreExpr::Con { tag, fields, .. } => {
                    for alt in &alts {
                        if let Some(bindings) = match_con_pat(&alt.pat, tag, fields) {
                            match &alt.guard {
                                Some(CoreExpr::Lit(CoreLit::Bool(false), _)) => continue,
                                Some(CoreExpr::Lit(CoreLit::Bool(true), _)) | None => {}
                                Some(_) => {
                                    return CoreExpr::Case {
                                        scrutinee: Box::new(scrutinee),
                                        alts,
                                        join_ty: join_ty.clone(),
                                        span,
                                    };
                                }
                            }
                            let mut body = alt.rhs.clone();
                            for (var, val) in bindings {
                                body = subst(body, var, &val);
                            }
                            return case_of_known_constructor(body);
                        }
                    }
                    CoreExpr::Case {
                        scrutinee: Box::new(scrutinee),
                        alts,
                        join_ty: join_ty.clone(),
                        span,
                    }
                }
                CoreExpr::Lit(lit, lit_span) => {
                    let lit = lit.clone();
                    let lit_span = *lit_span;
                    for alt in &alts {
                        if let Some(bindings) = match_lit_pat(&alt.pat, &lit, lit_span) {
                            match &alt.guard {
                                Some(CoreExpr::Lit(CoreLit::Bool(false), _)) => continue,
                                Some(CoreExpr::Lit(CoreLit::Bool(true), _)) | None => {}
                                Some(_) => {
                                    return CoreExpr::Case {
                                        scrutinee: Box::new(scrutinee),
                                        alts,
                                        join_ty: join_ty.clone(),
                                        span,
                                    };
                                }
                            }
                            let mut body = alt.rhs.clone();
                            for (var, val) in bindings {
                                body = subst(body, var, &val);
                            }
                            return case_of_known_constructor(body);
                        }
                    }
                    CoreExpr::Case {
                        scrutinee: Box::new(scrutinee),
                        alts,
                        join_ty: join_ty.clone(),
                        span,
                    }
                }
                _ => CoreExpr::Case {
                    scrutinee: Box::new(scrutinee),
                    alts,
                    join_ty,
                    span,
                },
            }
        }
        CoreExpr::Lam { .. }
        | CoreExpr::App { .. }
        | CoreExpr::Let { .. }
        | CoreExpr::LetRec { .. }
        | CoreExpr::LetRecGroup { .. }
        | CoreExpr::Con { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::Return { .. }
        | CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. } => map_children(expr, case_of_known_constructor),
        other => other,
    }
}

/// Try to match `Con { tag, fields }` against `pat`.
///
/// Returns `Some(bindings)` on success or `None` if the pattern cannot match
/// statically (wrong tag, arity mismatch, or nested non-trivial sub-pattern).
fn match_con_pat(
    pat: &CorePat,
    tag: &CoreTag,
    fields: &[CoreExpr],
) -> Option<Vec<(CoreBinderId, CoreExpr)>> {
    use crate::diagnostics::position::Span;
    match pat {
        CorePat::Wildcard => Some(vec![]),
        CorePat::Var(binder) => {
            let val = CoreExpr::Con {
                tag: tag.clone(),
                fields: fields.to_vec(),
                span: Span::default(),
            };
            Some(vec![(binder.id, val)])
        }
        CorePat::Con {
            tag: pat_tag,
            fields: pat_fields,
        } => {
            if pat_tag != tag || pat_fields.len() != fields.len() {
                return None;
            }
            let mut bindings = vec![];
            for (pat_field, val) in pat_fields.iter().zip(fields.iter()) {
                match pat_field {
                    CorePat::Wildcard => {}
                    CorePat::Var(binder) => bindings.push((binder.id, val.clone())),
                    // Nested non-trivial pattern — too complex for this pass.
                    _ => return None,
                }
            }
            Some(bindings)
        }
        CorePat::EmptyList => {
            if *tag == CoreTag::Nil && fields.is_empty() {
                Some(vec![])
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Try to match a literal scrutinee against `pat`.
fn match_lit_pat(
    pat: &CorePat,
    lit: &CoreLit,
    lit_span: crate::diagnostics::position::Span,
) -> Option<Vec<(CoreBinderId, CoreExpr)>> {
    match pat {
        CorePat::Wildcard => Some(vec![]),
        CorePat::Var(binder) => Some(vec![(binder.id, CoreExpr::Lit(lit.clone(), lit_span))]),
        CorePat::Lit(pat_lit) => {
            if pat_lit == lit {
                Some(vec![])
            } else {
                None
            }
        }
        _ => None,
    }
}
