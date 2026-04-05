/// Beta reduction pass.
///
/// Reduces obvious `App(Lam(x, body), arg)` → `body[x := arg]` at the top level.
///
/// This eliminates the desugaring overhead introduced by lowering
/// (e.g. `|>` pipe always produces `App(f, x)` which may be immediately applied).
use crate::core::CoreExpr;

use super::helpers::{map_children, subst};

pub fn beta_reduce(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::App { func, args, span } => {
            let func = beta_reduce(*func);
            let args: Vec<_> = args.into_iter().map(beta_reduce).collect();
            if let CoreExpr::Lam { params, body, .. } = func {
                if params.len() == args.len() {
                    // Full application: substitute all params
                    let mut body = *body;
                    for (p, a) in params.into_iter().zip(args.into_iter()) {
                        body = subst(body, p.id, &a);
                    }
                    beta_reduce(body)
                } else if args.len() < params.len() {
                    // Partial application: substitute provided args, return Lam with remaining
                    let mut body = *body;
                    let remaining = params[args.len()..].to_vec();
                    for (p, a) in params.into_iter().zip(args.into_iter()) {
                        body = subst(body, p.id, &a);
                    }
                    beta_reduce(CoreExpr::Lam {
                        params: remaining,
                        body: Box::new(body),
                        span,
                    })
                } else {
                    // Over-application: apply all params, then apply remaining args
                    let extra_args = args[params.len()..].to_vec();
                    let mut body = *body;
                    for (p, a) in params.into_iter().zip(args.into_iter()) {
                        body = subst(body, p.id, &a);
                    }
                    let body = beta_reduce(body);
                    beta_reduce(CoreExpr::App {
                        func: Box::new(body),
                        args: extra_args,
                        span,
                    })
                }
            } else {
                CoreExpr::App {
                    func: Box::new(func),
                    args,
                    span,
                }
            }
        }
        CoreExpr::Lam { .. }
        | CoreExpr::Let { .. }
        | CoreExpr::LetRec { .. }
        | CoreExpr::LetRecGroup { .. }
        | CoreExpr::Case { .. }
        | CoreExpr::Con { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::Return { .. }
        | CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. } => map_children(expr, beta_reduce),
        // Atoms are already in normal form.
        other => other,
    }
}
