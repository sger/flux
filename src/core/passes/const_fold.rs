use crate::core::{CoreAlt, CoreExpr, CoreLit, CorePrimOp};
use crate::shared::{int_arith, opt_bisect};

use super::helpers::{bind_pattern, select_known_arm};

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
            // A bisected run past its limit (0190 §13) keeps the operation.
            fold_primop(op, &args, span)
                .filter(|_| opt_bisect::allow("constant_fold", || format!("fold {op:?}")))
                .unwrap_or(CoreExpr::PrimOp { op, args, span })
        }
        CoreExpr::Case { scrutinee, alts, join_ty, span } => {
            let scrutinee = constant_fold(*scrutinee);
            let alts: Vec<CoreAlt> = alts
                .into_iter()
                .map(|mut alt| {
                    alt.guard = alt.guard.map(constant_fold);
                    alt.rhs = constant_fold(alt.rhs);
                    alt
                })
                .collect();
            fold_case(&scrutinee, &alts)
                .filter(|_| {
                    opt_bisect::allow("constant_fold", || {
                        "select the arm a known value matches".to_string()
                    })
                })
                .unwrap_or(CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, join_ty, span })
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
        // @KI-040: a folded Int means what it means at run time: it wraps,
        // and MIN / -1 is MIN.
        (IAdd | Add, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(int_arith::add(*a, *b)), span)
        }
        (ISub | Sub, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(int_arith::sub(*a, *b)), span)
        }
        (IMul | Mul, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) => {
            CoreExpr::Lit(Int(int_arith::mul(*a, *b)), span)
        }
        (IDiv | Div, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) if *b != 0 => {
            CoreExpr::Lit(Int(int_arith::div(*a, *b)), span)
        }
        (IMod | Mod, [CoreExpr::Lit(Int(a), _), CoreExpr::Lit(Int(b), _)]) if *b != 0 => {
            CoreExpr::Lit(Int(int_arith::rem(*a, *b)), span)
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
    let (alt, bindings) = select_known_arm(scrutinee, alts)?;
    Some(constant_fold(bind_pattern(alt.rhs.clone(), bindings, alt.span)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::position::Span;

    fn lit(value: i64) -> CoreExpr {
        CoreExpr::Lit(CoreLit::Int(value), Span::default())
    }

    fn folded(op: CorePrimOp, a: i64, b: i64) -> Option<i64> {
        match fold_primop(op, &[lit(a), lit(b)], Span::default())? {
            CoreExpr::Lit(CoreLit::Int(value), _) => Some(value),
            _ => None,
        }
    }

    #[test]
    fn folds_min_divided_by_minus_one_ki040() {
        assert_eq!(folded(CorePrimOp::IDiv, i64::MIN, -1), Some(i64::MIN));
        assert_eq!(folded(CorePrimOp::IMod, i64::MIN, -1), Some(0));
    }
}
