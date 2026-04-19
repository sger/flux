use crate::core::CoreExpr;

/// Normalize Core call and case administrative shapes before later passes.
pub fn call_and_case_canonicalize(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::App { func, args, span } => {
            let func = call_and_case_canonicalize(*func);
            let args: Vec<CoreExpr> = args.into_iter().map(call_and_case_canonicalize).collect();
            match func {
                CoreExpr::App {
                    func: inner_func,
                    args: mut inner_args,
                    ..
                } => {
                    inner_args.extend(args);
                    CoreExpr::App {
                        func: inner_func,
                        args: inner_args,
                        span,
                    }
                }
                CoreExpr::Let {
                    var,
                    rhs,
                    body,
                    span: let_span,
                } => CoreExpr::Let {
                    var,
                    rhs,
                    body: Box::new(CoreExpr::App {
                        func: body,
                        args,
                        span,
                    }),
                    span: let_span,
                },
                CoreExpr::LetRec {
                    var,
                    rhs,
                    body,
                    span: let_span,
                } => CoreExpr::LetRec {
                    var,
                    rhs,
                    body: Box::new(CoreExpr::App {
                        func: body,
                        args,
                        span,
                    }),
                    span: let_span,
                },
                CoreExpr::LetRecGroup {
                    bindings,
                    body,
                    span: let_span,
                } => CoreExpr::LetRecGroup {
                    bindings,
                    body: Box::new(CoreExpr::App {
                        func: body,
                        args,
                        span,
                    }),
                    span: let_span,
                },
                func => CoreExpr::App {
                    func: Box::new(func),
                    args,
                    span,
                },
            }
        }
        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => {
            let scrutinee = call_and_case_canonicalize(*scrutinee);
            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    alt.guard = alt.guard.map(call_and_case_canonicalize);
                    alt.rhs = call_and_case_canonicalize(alt.rhs);
                    alt
                })
                .collect();
            match scrutinee {
                CoreExpr::Let {
                    var,
                    rhs,
                    body,
                    span: let_span,
                } => CoreExpr::Let {
                    var,
                    rhs,
                    body: Box::new(CoreExpr::Case {
                        scrutinee: body,
                        alts,
                        join_ty,
                        span,
                    }),
                    span: let_span,
                },
                CoreExpr::LetRec {
                    var,
                    rhs,
                    body,
                    span: let_span,
                } => CoreExpr::LetRec {
                    var,
                    rhs,
                    body: Box::new(CoreExpr::Case {
                        scrutinee: body,
                        alts,
                        join_ty,
                        span,
                    }),
                    span: let_span,
                },
                CoreExpr::LetRecGroup {
                    bindings,
                    body,
                    span: let_span,
                } => CoreExpr::LetRecGroup {
                    bindings,
                    body: Box::new(CoreExpr::Case {
                        scrutinee: body,
                        alts,
                        join_ty,
                        span,
                    }),
                    span: let_span,
                },
                scrutinee => CoreExpr::Case {
                    scrutinee: Box::new(scrutinee),
                    alts,
                    join_ty,
                    span,
                },
            }
        }
        other => super::helpers::map_children(other, call_and_case_canonicalize),
    }
}
