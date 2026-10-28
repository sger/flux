/// A handle's body runs in a frame of its own.
///
/// Both backends delimit a handled computation by the frame the handle runs
/// in: a perform captures the frames above it, and the clause's value comes
/// back to that frame. Work the body does in that frame itself is outside
/// the delimited part. Natively a perform there skips the handle's prompt;
/// on the VM the clause's value lands where the frame stopped, so the rest
/// of the body runs again with it, or the perform repeats (KI-135). So, as
/// Koka defines `handle`, the body is an action the prompt calls, and the
/// frame does nothing else:
///
/// ```text
/// handle E { … } with body   →   handle E { … } with (λ. body)()
/// ```
///
/// Tail-resumptive handles become direct calls (`evidence_pass`), a body
/// that is already one call needs no action, and neither does one that
/// cannot perform: no call, perform or handle in its frame. A body with a `return`
/// keeps its frame, since the `return` would leave the action instead of
/// the function.
use crate::core::CoreExpr;

use super::helpers::map_children;
use super::tail_resumptive::is_core_handler_tail_resumptive;

pub fn handle_body_frames(expr: CoreExpr) -> CoreExpr {
    match map_children(expr, handle_body_frames) {
        CoreExpr::Handle { body, effect, parameter, handlers, span }
            if !is_core_handler_tail_resumptive(&handlers)
                && !is_one_call(&body)
                && in_frame(&body, &|e| {
                    matches!(
                        e,
                        CoreExpr::App { .. } | CoreExpr::Perform { .. } | CoreExpr::Handle { .. }
                    )
                })
                && !in_frame(&body, &|e| matches!(e, CoreExpr::Return { .. })) =>
        {
            // No source position: the debugger hides what the compiler made.
            let action = CoreExpr::Lam {
                params: Vec::new(),
                param_types: Vec::new(),
                result_ty: None,
                body,
                span: crate::diagnostics::position::Span::default(),
            };
            let body = CoreExpr::App { func: Box::new(action), args: Vec::new(), span };
            CoreExpr::Handle { body: Box::new(body), effect, parameter, handlers, span }
        }
        other => other,
    }
}

/// `f(a, b)`, with a variable for `f` and nothing to compute in the
/// arguments.
fn is_one_call(expr: &CoreExpr) -> bool {
    matches!(expr, CoreExpr::App { func, args, .. }
        if matches!(func.as_ref(), CoreExpr::Var { .. })
            && args.iter().all(|arg| matches!(arg, CoreExpr::Var { .. } | CoreExpr::Lit(..))))
}

/// Whether `found` holds for `expr` or a part of it that runs in the same
/// frame: not inside a lambda, and not in a handler's clauses, which run as
/// closures of their own.
fn in_frame(expr: &CoreExpr, found: &dyn Fn(&CoreExpr) -> bool) -> bool {
    if found(expr) {
        return true;
    }
    match expr {
        CoreExpr::Lam { .. } => false,
        CoreExpr::App { func, args, .. } => in_frame(func, found) || any_in_frame(args, found),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            in_frame(rhs, found) || in_frame(body, found)
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            any_in_frame(bindings.iter().map(|(_, rhs)| rhs.as_ref()), found)
                || in_frame(body, found)
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            in_frame(scrutinee, found)
                || alts.iter().any(|alt| {
                    in_frame(&alt.rhs, found)
                        || alt.guard.as_ref().is_some_and(|g| in_frame(g, found))
                })
        }
        CoreExpr::Con { fields: args, .. }
        | CoreExpr::PrimOp { args, .. }
        | CoreExpr::Perform { args, .. } => any_in_frame(args, found),
        CoreExpr::Return { value: inner, .. }
        | CoreExpr::MemberAccess { object: inner, .. }
        | CoreExpr::TupleField { object: inner, .. } => in_frame(inner, found),
        CoreExpr::Handle { body, parameter, .. } => {
            in_frame(body, found) || parameter.as_ref().is_some_and(|p| in_frame(p, found))
        }
        _ => false,
    }
}

fn any_in_frame<'a>(
    exprs: impl IntoIterator<Item = &'a CoreExpr>,
    found: &dyn Fn(&CoreExpr) -> bool,
) -> bool {
    exprs.into_iter().any(|e| in_frame(e, found))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{CoreBinder, CoreBinderId, CoreHandler, CoreLit};
    use crate::diagnostics::position::Span;
    use crate::syntax::symbol::Symbol;

    fn binder(id: u32) -> CoreBinder {
        CoreBinder::new(CoreBinderId(id), Symbol::new(id))
    }

    fn int(n: i64) -> CoreExpr {
        CoreExpr::Lit(CoreLit::Int(n), Span::default())
    }

    fn perform() -> CoreExpr {
        CoreExpr::Perform {
            effect: Symbol::new(900),
            operation: Symbol::new(901),
            args: vec![int(1)],
            span: Span::default(),
        }
    }

    /// `handle Ask { ask(resume, x) -> clause }` around `body`.
    fn handle(body: CoreExpr, clause: CoreExpr) -> CoreExpr {
        CoreExpr::Handle {
            body: Box::new(body),
            effect: Symbol::new(900),
            parameter: None,
            handlers: vec![CoreHandler {
                operation: Symbol::new(901),
                params: vec![binder(2)],
                param_types: vec![None],
                state: None,
                state_ty: None,
                resume: binder(1),
                resume_ty: None,
                body: clause,
                span: Span::default(),
            }],
            span: Span::default(),
        }
    }

    fn resume_x() -> CoreExpr {
        CoreExpr::App {
            func: Box::new(CoreExpr::bound_var(&binder(1), Span::default())),
            args: vec![CoreExpr::bound_var(&binder(2), Span::default())],
            span: Span::default(),
        }
    }

    fn body_of(expr: &CoreExpr) -> &CoreExpr {
        match expr {
            CoreExpr::Handle { body, .. } => body,
            other => panic!("expected a handle, got {other:?}"),
        }
    }

    fn is_action_call(expr: &CoreExpr) -> bool {
        matches!(expr, CoreExpr::App { func, args, .. }
            if args.is_empty() && matches!(func.as_ref(), CoreExpr::Lam { params, .. } if params.is_empty()))
    }

    /// `f(1)`: one call with nothing to compute in its arguments.
    fn call_f() -> CoreExpr {
        CoreExpr::App {
            func: Box::new(CoreExpr::bound_var(&binder(3), Span::default())),
            args: vec![int(1)],
            span: Span::default(),
        }
    }

    #[test]
    fn a_body_that_performs_or_computes_after_a_call_gets_a_frame() {
        let performs = handle_body_frames(handle(perform(), int(100)));
        assert!(is_action_call(body_of(&performs)), "{performs:?}");

        let after_call = CoreExpr::PrimOp {
            op: crate::core::CorePrimOp::IAdd,
            args: vec![call_f(), int(1)],
            span: Span::default(),
        };
        let computes = handle_body_frames(handle(after_call, int(100)));
        assert!(is_action_call(body_of(&computes)), "{computes:?}");
    }

    #[test]
    fn a_tail_resumptive_handle_one_call_or_a_return_keeps_its_body() {
        let tail_resumptive = handle_body_frames(handle(perform(), resume_x()));
        assert!(matches!(body_of(&tail_resumptive), CoreExpr::Perform { .. }));

        let one_call = handle_body_frames(handle(call_f(), int(100)));
        assert!(matches!(body_of(&one_call), CoreExpr::App { func, .. }
            if matches!(func.as_ref(), CoreExpr::Var { .. })));

        let with_return = CoreExpr::Let {
            var: binder(4),
            rhs: Box::new(perform()),
            body: Box::new(CoreExpr::Return { value: Box::new(int(0)), span: Span::default() }),
            span: Span::default(),
        };
        let returns = handle_body_frames(handle(with_return, int(100)));
        assert!(matches!(body_of(&returns), CoreExpr::Let { .. }));
    }
}
