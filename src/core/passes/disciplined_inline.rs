use crate::core::{CoreBinder, CoreExpr, CorePrimOp};

use super::{
    helpers::{expr_size, is_pure, subst},
    inliner::{count_occurrences, occurs_as_callee, occurs_under_lambda},
};

const INLINE_THRESHOLD: usize = 8;

/// Centralized non-trivial Core inlining policy.
pub fn disciplined_inline(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            let rhs = disciplined_inline(*rhs);
            let body = disciplined_inline(*body);
            let count = count_occurrences(var.id, &body);
            let preserves_call_boundary = occurs_as_callee(var.id, &body) && !is_wrapper_rhs(&rhs);

            if count == 0 && is_pure(&rhs) {
                body
            } else if count == 0 {
                CoreExpr::Let {
                    var,
                    rhs: Box::new(rhs),
                    body: Box::new(body),
                    span,
                }
            } else if is_pure(&rhs)
                && !preserves_call_boundary
                && ((count == 1 && !occurs_under_lambda(var.id, &body))
                    || expr_size(&rhs) <= INLINE_THRESHOLD)
            {
                disciplined_inline(subst(body, var.id, &rhs))
            } else {
                CoreExpr::Let {
                    var,
                    rhs: Box::new(rhs),
                    body: Box::new(body),
                    span,
                }
            }
        }
        other => super::helpers::map_children(other, disciplined_inline),
    }
}

fn is_wrapper_rhs(expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Var { .. } => true,
        CoreExpr::Lam { params, body, .. } => forwards_params(params, body),
        _ => false,
    }
}

fn forwards_params(params: &[CoreBinder], body: &CoreExpr) -> bool {
    match body {
        CoreExpr::Con { fields, .. } => fields.iter().all(|field| forwards_params(params, field)),
        CoreExpr::PrimOp { args, op, .. } => {
            is_wrapper_primop(*op) && args.iter().all(|arg| forwards_params(params, arg))
        }
        CoreExpr::App { func, args, .. } => {
            forwards_params(params, func) && args.iter().all(|arg| forwards_params(params, arg))
        }
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
            | CorePrimOp::TaskSpawn
            | CorePrimOp::TaskBlockingJoin
            | CorePrimOp::TaskCancel
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
