use crate::core::{CoreAlt, CoreBinderId, CoreExpr, CoreLit, CorePat, CorePrimOp, CoreTag};

use super::helpers::subst;

/// Fold literal-only Core primops and known branch conditions.
///
/// Scope: **compile-time evaluation** of primops whose operands are all
/// literals (e.g. `3 + 4 → 7`, `not(true) → false`), plus `Case` scrutinees
/// that reduce to a known constructor or literal.
///
/// Companion pass: [`super::algebraic_simplify`] handles the complementary
/// case where only one operand is a literal — those rewrites use algebraic
/// identities (`x * 0 → 0`, `x + 0 → x`) and fire without knowing `x`. When
/// in doubt: if your rewrite computes a concrete result from known inputs,
/// it belongs here; if it rewrites a symbolic expression through an identity,
/// it belongs in `algebraic_simplify`.
pub fn constant_fold(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::PrimOp { op, args, span } => {
            let args: Vec<CoreExpr> = args.into_iter().map(constant_fold).collect();
            fold_primop(op, &args, span).unwrap_or(CoreExpr::PrimOp { op, args, span })
        }
        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => {
            let scrutinee = constant_fold(*scrutinee);
            let alts: Vec<CoreAlt> = alts
                .into_iter()
                .map(|mut alt| {
                    alt.guard = alt.guard.map(constant_fold);
                    alt.rhs = constant_fold(alt.rhs);
                    alt
                })
                .collect();
            fold_case(&scrutinee, &alts).unwrap_or(CoreExpr::Case {
                scrutinee: Box::new(scrutinee),
                alts,
                join_ty,
                span,
            })
        }
        other => super::helpers::map_children(other, constant_fold),
    }
}

fn fold_primop(
    op: CorePrimOp,
    args: &[CoreExpr],
    span: crate::diagnostics::position::Span,
) -> Option<CoreExpr> {
    use CoreLit::{Bool, Float, Int};
    use CorePrimOp::*;
    Some(match (op, args) {
        (IAdd | Add, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(a + b), span)
        }
        (ISub | Sub, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(a - b), span)
        }
        (IMul | Mul, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(a * b), span)
        }
        (IDiv | Div, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) if *b != 0 => {
            CoreExpr::Lit(Int(a / b), span)
        }
        (IMod | Mod, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) if *b != 0 => {
            CoreExpr::Lit(Int(a % b), span)
        }
        (FAdd | Add, [CoreExpr::Lit(Float(a), _), CoreExpr::Lit(Float(b), _)]) => {
            CoreExpr::Lit(Float(a + b), span)
        }
        (FSub | Sub, [CoreExpr::Lit(Float(a), _), CoreExpr::Lit(Float(b), _)]) => {
            CoreExpr::Lit(Float(a - b), span)
        }
        (FMul | Mul, [CoreExpr::Lit(Float(a), _), CoreExpr::Lit(Float(b), _)]) => {
            CoreExpr::Lit(Float(a * b), span)
        }
        (FDiv | Div, [CoreExpr::Lit(Float(a), _), CoreExpr::Lit(Float(b), _)]) if *b != 0.0 => {
            CoreExpr::Lit(Float(a / b), span)
        }
        (FSqrt, [CoreExpr::Lit(Float(a), _)]) => CoreExpr::Lit(Float(a.sqrt()), span),
        (FSin, [CoreExpr::Lit(Float(a), _)]) => CoreExpr::Lit(Float(a.sin()), span),
        (FCos, [CoreExpr::Lit(Float(a), _)]) => CoreExpr::Lit(Float(a.cos()), span),
        (FExp, [CoreExpr::Lit(Float(a), _)]) => CoreExpr::Lit(Float(a.exp()), span),
        (FLog, [CoreExpr::Lit(Float(a), _)]) => CoreExpr::Lit(Float(a.ln()), span),
        (FFloor, [CoreExpr::Lit(Float(a), _)]) => CoreExpr::Lit(Float(a.floor()), span),
        (FCeil, [CoreExpr::Lit(Float(a), _)]) => CoreExpr::Lit(Float(a.ceil()), span),
        (FRound, [CoreExpr::Lit(Float(a), _)]) => CoreExpr::Lit(Float(a.round()), span),
        (BitAnd, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(a & b), span)
        }
        (BitOr, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(a | b), span)
        }
        (BitXor, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(a ^ b), span)
        }
        (BitShl, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(a.wrapping_shl(masked_shift_amount(*b))), span)
        }
        (BitShr, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(a.wrapping_shr(masked_shift_amount(*b))), span)
        }
        (ICmpEq | Eq, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Bool(a == b), span)
        }
        (ICmpNe | NEq, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Bool(a != b), span)
        }
        (ICmpLt | Lt, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Bool(a < b), span)
        }
        (ICmpLe | Le, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Bool(a <= b), span)
        }
        (ICmpGt | Gt, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Bool(a > b), span)
        }
        (ICmpGe | Ge, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Bool(a >= b), span)
        }
        (FCmpEq | Eq, [CoreExpr::Lit(Float(a), _), CoreExpr::Lit(Float(b), _)]) => {
            CoreExpr::Lit(Bool(a == b), span)
        }
        (FCmpNe | NEq, [CoreExpr::Lit(Float(a), _), CoreExpr::Lit(Float(b), _)]) => {
            CoreExpr::Lit(Bool(a != b), span)
        }
        (FCmpLt | Lt, [CoreExpr::Lit(Float(a), _), CoreExpr::Lit(Float(b), _)]) => {
            CoreExpr::Lit(Bool(a < b), span)
        }
        (FCmpLe | Le, [CoreExpr::Lit(Float(a), _), CoreExpr::Lit(Float(b), _)]) => {
            CoreExpr::Lit(Bool(a <= b), span)
        }
        (FCmpGt | Gt, [CoreExpr::Lit(Float(a), _), CoreExpr::Lit(Float(b), _)]) => {
            CoreExpr::Lit(Bool(a > b), span)
        }
        (FCmpGe | Ge, [CoreExpr::Lit(Float(a), _), CoreExpr::Lit(Float(b), _)]) => {
            CoreExpr::Lit(Bool(a >= b), span)
        }
        (And, [CoreExpr::Lit(Bool(a), _), CoreExpr::Lit(Bool(b), _)]) => {
            CoreExpr::Lit(Bool(*a && *b), span)
        }
        (Or, [CoreExpr::Lit(Bool(a), _), CoreExpr::Lit(Bool(b), _)]) => {
            CoreExpr::Lit(Bool(*a || *b), span)
        }
        (Not, [CoreExpr::Lit(Bool(a), _)]) => CoreExpr::Lit(Bool(!a), span),
        _ => return None,
    })
}

fn masked_shift_amount(value: i64) -> u32 {
    (value as u64 & 63) as u32
}

fn fold_case(scrutinee: &CoreExpr, alts: &[CoreAlt]) -> Option<CoreExpr> {
    match scrutinee {
        CoreExpr::Lit(lit, lit_span) => {
            for alt in alts {
                let Some(bindings) = match_lit_pat(&alt.pat, lit, *lit_span) else {
                    continue;
                };
                match &alt.guard {
                    Some(CoreExpr::Lit(CoreLit::Bool(false), _)) => continue,
                    Some(CoreExpr::Lit(CoreLit::Bool(true), _)) | None => {}
                    Some(_) => return None,
                }
                let mut body = alt.rhs.clone();
                for (var, val) in bindings {
                    body = subst(body, var, &val);
                }
                return Some(constant_fold(body));
            }
            None
        }
        CoreExpr::Con { tag, fields, span } => {
            for alt in alts {
                let Some(bindings) = match_con_pat(&alt.pat, tag, fields, *span) else {
                    continue;
                };
                match &alt.guard {
                    Some(CoreExpr::Lit(CoreLit::Bool(false), _)) => continue,
                    Some(CoreExpr::Lit(CoreLit::Bool(true), _)) | None => {}
                    Some(_) => return None,
                }
                let mut body = alt.rhs.clone();
                for (var, val) in bindings {
                    body = subst(body, var, &val);
                }
                return Some(constant_fold(body));
            }
            None
        }
        _ => None,
    }
}

fn match_lit_pat(
    pat: &CorePat,
    lit: &CoreLit,
    lit_span: crate::diagnostics::position::Span,
) -> Option<Vec<(CoreBinderId, CoreExpr)>> {
    match pat {
        CorePat::Wildcard => Some(Vec::new()),
        CorePat::Var(binder) => Some(vec![(binder.id, CoreExpr::Lit(lit.clone(), lit_span))]),
        CorePat::Lit(pat_lit) if pat_lit == lit => Some(Vec::new()),
        _ => None,
    }
}

fn match_con_pat(
    pat: &CorePat,
    tag: &CoreTag,
    fields: &[CoreExpr],
    span: crate::diagnostics::position::Span,
) -> Option<Vec<(CoreBinderId, CoreExpr)>> {
    match pat {
        CorePat::Wildcard => Some(Vec::new()),
        CorePat::Var(binder) => Some(vec![(
            binder.id,
            CoreExpr::Con {
                tag: tag.clone(),
                fields: fields.to_vec(),
                span,
            },
        )]),
        CorePat::Con {
            tag: pat_tag,
            fields: pat_fields,
        } if pat_tag == tag && pat_fields.len() == fields.len() => {
            let mut bindings = Vec::new();
            for (pat_field, field) in pat_fields.iter().zip(fields.iter()) {
                match pat_field {
                    CorePat::Wildcard => {}
                    CorePat::Var(binder) => bindings.push((binder.id, field.clone())),
                    _ => return None,
                }
            }
            Some(bindings)
        }
        CorePat::EmptyList if *tag == CoreTag::Nil && fields.is_empty() => Some(Vec::new()),
        _ => None,
    }
}
