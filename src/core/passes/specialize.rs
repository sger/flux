use crate::core::{CoreBinder, CoreExpr, CorePrimOp};

use super::{
    helpers::{is_pure, subst},
    inliner::{count_occurrences, occurs_as_callee},
};

/// Specialize trivial known wrappers into direct Core forms.
pub fn specialize_known_shapes(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            let rhs = specialize_known_shapes(*rhs);
            let body = specialize_known_shapes(*body);
            if count_occurrences(var.id, &body) == 1
                && occurs_as_callee(var.id, &body)
                && is_specializable_rhs(&rhs)
                && is_pure(&rhs)
            {
                specialize_known_shapes(subst(body, var.id, &rhs))
            } else {
                CoreExpr::Let {
                    var,
                    rhs: Box::new(rhs),
                    body: Box::new(body),
                    span,
                }
            }
        }
        other => super::helpers::map_children(other, specialize_known_shapes),
    }
}

fn is_specializable_rhs(expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Var { .. } => true,
        CoreExpr::Lam { params, body, .. } => is_specializable_wrapper(params, body),
        _ => false,
    }
}

fn is_specializable_wrapper(params: &[CoreBinder], body: &CoreExpr) -> bool {
    match body {
        CoreExpr::Con { fields, .. } => fields.iter().all(|field| wrapper_atom(params, field)),
        CoreExpr::PrimOp { op, args, .. } => {
            is_wrapper_primop(*op) && args.iter().all(|arg| wrapper_atom(params, arg))
        }
        CoreExpr::App { func, args, .. } => {
            wrapper_atom(params, func) && args.iter().all(|arg| wrapper_atom(params, arg))
        }
        _ => false,
    }
}

fn wrapper_atom(params: &[CoreBinder], expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Var { var, .. } => {
            var.binder
                .is_some_and(|binder| params.iter().any(|param| param.id == binder))
                || var.binder.is_none()
        }
        CoreExpr::Lit(_, _) => true,
        _ => false,
    }
}

fn is_wrapper_primop(op: CorePrimOp) -> bool {
    !matches!(
        op,
        CorePrimOp::Print
            | CorePrimOp::Println
            | CorePrimOp::DebugTrace
            | CorePrimOp::ReadFile
            | CorePrimOp::WriteFile
            | CorePrimOp::ReadStdin
            | CorePrimOp::ReadLines
            | CorePrimOp::ClockNow
            | CorePrimOp::Time
            | CorePrimOp::Panic
            | CorePrimOp::Try
            | CorePrimOp::AssertThrows
            | CorePrimOp::PerformDirect
            | CorePrimOp::YieldTo
            | CorePrimOp::YieldExtend
            | CorePrimOp::YieldPrompt
            | CorePrimOp::EvvSet
            | CorePrimOp::EvvInsert
    )
}
