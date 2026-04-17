use crate::core::{CoreExpr, CoreLit, CorePrimOp};

/// Apply small backend-neutral algebraic rewrites over Core expressions.
pub fn algebraic_simplify(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::PrimOp { op, args, span } => {
            let args: Vec<CoreExpr> = args.into_iter().map(algebraic_simplify).collect();
            match (op, args.as_slice()) {
                (CorePrimOp::IAdd, [lhs, CoreExpr::Lit(CoreLit::Int(0), _)])
                | (CorePrimOp::ISub, [lhs, CoreExpr::Lit(CoreLit::Int(0), _)])
                | (CorePrimOp::IMul, [lhs, CoreExpr::Lit(CoreLit::Int(1), _)])
                | (CorePrimOp::IDiv, [lhs, CoreExpr::Lit(CoreLit::Int(1), _)]) => lhs.clone(),
                (CorePrimOp::IAdd, [CoreExpr::Lit(CoreLit::Int(0), _), rhs])
                | (CorePrimOp::IMul, [CoreExpr::Lit(CoreLit::Int(1), _), rhs]) => rhs.clone(),
                (CorePrimOp::IMul, [_, CoreExpr::Lit(CoreLit::Int(0), _)])
                | (CorePrimOp::IMul, [CoreExpr::Lit(CoreLit::Int(0), _), _]) => {
                    CoreExpr::Lit(CoreLit::Int(0), span)
                }
                (CorePrimOp::IMod, [_, CoreExpr::Lit(CoreLit::Int(1), _)]) => {
                    CoreExpr::Lit(CoreLit::Int(0), span)
                }

                (CorePrimOp::FAdd, [lhs, CoreExpr::Lit(CoreLit::Float(v), _)]) if *v == 0.0 => {
                    lhs.clone()
                }
                (CorePrimOp::FSub, [lhs, CoreExpr::Lit(CoreLit::Float(v), _)]) if *v == 0.0 => {
                    lhs.clone()
                }
                (CorePrimOp::FMul, [lhs, CoreExpr::Lit(CoreLit::Float(v), _)]) if *v == 1.0 => {
                    lhs.clone()
                }
                (CorePrimOp::FDiv, [lhs, CoreExpr::Lit(CoreLit::Float(v), _)]) if *v == 1.0 => {
                    lhs.clone()
                }
                (CorePrimOp::FAdd, [CoreExpr::Lit(CoreLit::Float(v), _), rhs]) if *v == 0.0 => {
                    rhs.clone()
                }
                (CorePrimOp::FMul, [CoreExpr::Lit(CoreLit::Float(v), _), rhs]) if *v == 1.0 => {
                    rhs.clone()
                }
                (CorePrimOp::FMul, [_, CoreExpr::Lit(CoreLit::Float(v), _)]) if *v == 0.0 => {
                    CoreExpr::Lit(CoreLit::Float(0.0), span)
                }
                (CorePrimOp::FMul, [CoreExpr::Lit(CoreLit::Float(v), _), _]) if *v == 0.0 => {
                    CoreExpr::Lit(CoreLit::Float(0.0), span)
                }

                (
                    CorePrimOp::Not,
                    [
                        CoreExpr::PrimOp {
                            op: CorePrimOp::Not,
                            args: inner,
                            ..
                        },
                    ],
                ) if inner.len() == 1 && expr_is_known_bool(&inner[0]) => inner[0].clone(),
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
                (CorePrimOp::And, [_, CoreExpr::Lit(CoreLit::Bool(false), _)])
                | (CorePrimOp::And, [CoreExpr::Lit(CoreLit::Bool(false), _), _]) => {
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
                (CorePrimOp::Or, [_, CoreExpr::Lit(CoreLit::Bool(true), _)])
                | (CorePrimOp::Or, [CoreExpr::Lit(CoreLit::Bool(true), _), _]) => {
                    CoreExpr::Lit(CoreLit::Bool(true), span)
                }
                _ => CoreExpr::PrimOp { op, args, span },
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
