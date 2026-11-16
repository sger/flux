use super::helpers::is_pure;
use crate::core::{CoreExpr, CoreLit, CorePrimOp};
use crate::shared::opt_bisect;

/// The bits of `+0.0` and `-0.0`, which compare equal as floats.
const POS_ZERO: u64 = 0;
const NEG_ZERO: u64 = 0x8000_0000_0000_0000;

/// Apply small backend-neutral algebraic rewrites over Core expressions.
///
/// Scope: **identity-driven simplifications** where one operand is a literal
/// but the other can be an arbitrary subexpression — `x + 0 → x`, `x * 1 → x`,
/// `x * 0 → 0`, `x mod 1 → 0`, and the Float identities IEEE 754 makes exact.
/// These fire in situations `constant_fold` cannot reach.
///
/// The pass runs before ANF, so `x` can be a call: a rewrite that discards
/// `x` applies only when `x` is pure. A Float rewrite must give the same bits
/// for every `x`, `-0.0`, the infinities and NaN included.
///
/// Companion pass: [`super::constant_fold`] handles the complementary case
/// where **every** operand is a literal — that pass computes the result at
/// compile time. When in doubt: if your rewrite needs both sides known, it
/// belongs in `constant_fold`; if only the shape of one side matters, it
/// belongs here.
pub fn algebraic_simplify(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::PrimOp { op, args, span } => {
            let args: Vec<CoreExpr> = args.into_iter().map(algebraic_simplify).collect();
            let simplified = match (op, args.as_slice()) {
                (CorePrimOp::IAdd, [lhs, CoreExpr::Lit(CoreLit::Int(0), _)])
                | (CorePrimOp::ISub, [lhs, CoreExpr::Lit(CoreLit::Int(0), _)])
                | (CorePrimOp::IMul, [lhs, CoreExpr::Lit(CoreLit::Int(1), _)])
                | (CorePrimOp::IDiv, [lhs, CoreExpr::Lit(CoreLit::Int(1), _)]) => lhs.clone(),
                (CorePrimOp::IAdd, [CoreExpr::Lit(CoreLit::Int(0), _), rhs])
                | (CorePrimOp::IMul, [CoreExpr::Lit(CoreLit::Int(1), _), rhs]) => rhs.clone(),
                // @KI-200: these discard an operand, which may be a call.
                (CorePrimOp::IMul, [x, CoreExpr::Lit(CoreLit::Int(0), _)])
                | (CorePrimOp::IMul, [CoreExpr::Lit(CoreLit::Int(0), _), x])
                | (CorePrimOp::IMod, [x, CoreExpr::Lit(CoreLit::Int(1), _)])
                    if is_pure(x) =>
                {
                    CoreExpr::Lit(CoreLit::Int(0), span)
                }

                // @KI-200: `-0.0 + 0.0` is `+0.0`, so adding `+0.0` is not an
                // identity, and `x * 0.0` is `-0.0` or NaN for some `x`. Only
                // adding `-0.0` and subtracting `+0.0` keep every `x`.
                (CorePrimOp::FAdd, [lhs, CoreExpr::Lit(CoreLit::Float(v), _)])
                    if v.to_bits() == NEG_ZERO =>
                {
                    lhs.clone()
                }
                (CorePrimOp::FAdd, [CoreExpr::Lit(CoreLit::Float(v), _), rhs])
                    if v.to_bits() == NEG_ZERO =>
                {
                    rhs.clone()
                }
                (CorePrimOp::FSub, [lhs, CoreExpr::Lit(CoreLit::Float(v), _)])
                    if v.to_bits() == POS_ZERO =>
                {
                    lhs.clone()
                }
                (CorePrimOp::FMul, [lhs, CoreExpr::Lit(CoreLit::Float(v), _)]) if *v == 1.0 => {
                    lhs.clone()
                }
                (CorePrimOp::FDiv, [lhs, CoreExpr::Lit(CoreLit::Float(v), _)]) if *v == 1.0 => {
                    lhs.clone()
                }
                (CorePrimOp::FMul, [CoreExpr::Lit(CoreLit::Float(v), _), rhs]) if *v == 1.0 => {
                    rhs.clone()
                }

                (CorePrimOp::Not, [CoreExpr::PrimOp { op: CorePrimOp::Not, args: inner, .. }])
                    if inner.len() == 1 && expr_is_known_bool(&inner[0]) =>
                {
                    inner[0].clone()
                }
                (CorePrimOp::And, [lhs, CoreExpr::Lit(CoreLit::Bool(true), _)])
                    if expr_is_known_bool(lhs) =>
                {
                    lhs.clone()
                }
                (CorePrimOp::And, [CoreExpr::Lit(CoreLit::Bool(true), _), rhs])
                    if expr_is_known_bool(rhs) =>
                {
                    rhs.clone()
                }
                // The left operand runs first, so the right one may be skipped;
                // the left one is discarded only when pure (@KI-200).
                (CorePrimOp::And, [CoreExpr::Lit(CoreLit::Bool(false), _), _]) => {
                    CoreExpr::Lit(CoreLit::Bool(false), span)
                }
                (CorePrimOp::And, [lhs, CoreExpr::Lit(CoreLit::Bool(false), _)])
                    if is_pure(lhs) =>
                {
                    CoreExpr::Lit(CoreLit::Bool(false), span)
                }
                (CorePrimOp::Or, [lhs, CoreExpr::Lit(CoreLit::Bool(false), _)])
                    if expr_is_known_bool(lhs) =>
                {
                    lhs.clone()
                }
                (CorePrimOp::Or, [CoreExpr::Lit(CoreLit::Bool(false), _), rhs])
                    if expr_is_known_bool(rhs) =>
                {
                    rhs.clone()
                }
                (CorePrimOp::Or, [CoreExpr::Lit(CoreLit::Bool(true), _), _]) => {
                    CoreExpr::Lit(CoreLit::Bool(true), span)
                }
                (CorePrimOp::Or, [lhs, CoreExpr::Lit(CoreLit::Bool(true), _)]) if is_pure(lhs) => {
                    CoreExpr::Lit(CoreLit::Bool(true), span)
                }
                _ => return CoreExpr::PrimOp { op, args, span },
            };
            // A bisected run past its limit (0190 §13) keeps the operation.
            if opt_bisect::allow("algebraic_simplify", || format!("simplify {op:?}")) {
                simplified
            } else {
                CoreExpr::PrimOp { op, args, span }
            }
        }
        other => super::helpers::map_children(other, algebraic_simplify),
    }
}

fn expr_is_known_bool(expr: &CoreExpr) -> bool {
    matches!(
        expr,
        CoreExpr::Lit(CoreLit::Bool(_), _)
            | CoreExpr::PrimOp {
                op: CorePrimOp::Not
                    | CorePrimOp::And
                    | CorePrimOp::Or
                    | CorePrimOp::Eq
                    | CorePrimOp::NEq
                    | CorePrimOp::Lt
                    | CorePrimOp::Le
                    | CorePrimOp::Gt
                    | CorePrimOp::Ge
                    | CorePrimOp::ICmpEq
                    | CorePrimOp::ICmpNe
                    | CorePrimOp::ICmpLt
                    | CorePrimOp::ICmpLe
                    | CorePrimOp::ICmpGt
                    | CorePrimOp::ICmpGe
                    | CorePrimOp::FCmpEq
                    | CorePrimOp::FCmpNe
                    | CorePrimOp::FCmpLt
                    | CorePrimOp::FCmpLe
                    | CorePrimOp::FCmpGt
                    | CorePrimOp::FCmpGe
                    | CorePrimOp::CmpEq
                    | CorePrimOp::CmpNe,
                ..
            }
    )
}
