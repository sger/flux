//! What a function still holds right after each call and effect.
//!
//! Native code can suspend at a call or an effect: the frame becomes a
//! continuation closure, which takes a reference of its own to each value it
//! captures. The frame's own references have to go with it, and each resume
//! has to start with exactly those again, because the rest of the function
//! releases them. This pass records them on each `AetherCall` and `Perform`.
//!
//! The rest of a path releases a value it holds once more than it copies it,
//! so the count is taken backwards over the finished plan: a release or a
//! hand-over adds one, a `Dup` takes one away. A borrowed parameter or a
//! view is only read and nets zero.

use std::collections::{BTreeMap, HashSet};

use crate::core::{CoreBinder, CoreBinderId, CorePat, CoreVarRef};

use super::AetherExpr;
use super::borrow_infer::BorrowMode;

/// Releases still to come on the rest of the path, less its copies.
type Pending = BTreeMap<CoreBinderId, i64>;

/// Fills `owned_after` on every call and effect in `expr`.
pub fn annotate_owned_after(mut expr: AetherExpr) -> AetherExpr {
    let mut counted = HashSet::new();
    collect_counted(&expr, &mut counted);
    function_body(&mut expr, &counted);
    expr
}

/// A function body, a lambda's or a handler clause's: it ends by handing its
/// result over, and nothing it holds outlives it.
fn function_body(body: &mut AetherExpr, counted: &HashSet<CoreBinderId>) {
    walk(body, &mut Pending::new(), true, counted);
}

/// Each binder the function holds, once per reference. Only local binders
/// that need a count: lowering copies a top-level value at each use.
fn held(pending: &Pending, counted: &HashSet<CoreBinderId>) -> Vec<CoreBinderId> {
    pending
        .iter()
        .filter(|(binder, _)| counted.contains(binder))
        .flat_map(|(binder, count)| std::iter::repeat_n(*binder, (*count).max(0) as usize))
        .collect()
}

fn release(var: &CoreVarRef, pending: &mut Pending, by: i64) {
    if let Some(binder) = var.binder {
        *pending.entry(binder).or_default() += by;
    }
}

/// Walks `expr` from its end to its start. `given` says the value it
/// produces is handed over: a bare binder there gives its reference away.
fn walk(
    expr: &mut AetherExpr,
    pending: &mut Pending,
    given: bool,
    counted: &HashSet<CoreBinderId>,
) {
    match expr {
        AetherExpr::Var { var, .. } => {
            if given {
                release(var, pending, 1);
            }
        }
        AetherExpr::Lit(..) => {}
        AetherExpr::Lam { body, .. } => function_body(body, counted),
        // Only before planning, which lends its arguments.
        AetherExpr::App { func, args, .. } => {
            for arg in args.iter_mut().rev() {
                walk(arg, pending, false, counted);
            }
            walk(func, pending, false, counted);
        }
        AetherExpr::AetherCall {
            func,
            args,
            arg_modes,
            owned_after,
            ..
        } => {
            *owned_after = held(pending, counted);
            for (arg, mode) in args.iter_mut().zip(arg_modes.iter()).rev() {
                walk(arg, pending, *mode == BorrowMode::Owned, counted);
            }
            walk(func, pending, false, counted);
        }
        AetherExpr::Let { var, rhs, body, .. } => {
            walk(body, pending, given, counted);
            let own = pending.remove(&var.id).unwrap_or(0);
            // `let y = x` names the reference `x` holds, or the copy a `Dup`
            // in the right-hand side made: what `y` releases, `x` does.
            if let Some(source) = bare_binder(rhs) {
                *pending.entry(source).or_default() += own;
            }
            walk(rhs, pending, false, counted);
        }
        AetherExpr::LetRec { var, rhs, body, .. } => {
            walk(body, pending, given, counted);
            pending.remove(&var.id);
            walk(rhs, pending, false, counted);
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            walk(body, pending, given, counted);
            for (var, _) in bindings.iter() {
                pending.remove(&var.id);
            }
            for (_, rhs) in bindings.iter_mut().rev() {
                walk(rhs, pending, false, counted);
            }
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            let after = pending.clone();
            let mut arms = Vec::with_capacity(alts.len());
            for alt in alts.iter_mut() {
                let mut arm = after.clone();
                walk(&mut alt.rhs, &mut arm, given, counted);
                if let Some(guard) = &mut alt.guard {
                    walk(guard, &mut arm, false, counted);
                }
                for binder in super::analysis::pat_binders(&alt.pat) {
                    arm.remove(&binder);
                }
                arms.push(arm);
            }
            *pending = join(arms, after);
            walk(scrutinee, pending, false, counted);
        }
        AetherExpr::Con { fields, .. } => {
            for field in fields.iter_mut().rev() {
                walk(field, pending, true, counted);
            }
        }
        AetherExpr::PrimOp { op, args, .. } => {
            let op = *op;
            for (index, arg) in args.iter_mut().enumerate().rev() {
                walk(arg, pending, op.takes_arg(index), counted);
            }
        }
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            walk(object, pending, false, counted)
        }
        // Nothing after a return runs.
        AetherExpr::Return { value, .. } => {
            pending.clear();
            walk(value, pending, true, counted);
        }
        // The runtime takes the argument and releases it after the clause.
        AetherExpr::Perform {
            args, owned_after, ..
        } => {
            *owned_after = held(pending, counted);
            for arg in args.iter_mut().rev() {
                walk(arg, pending, true, counted);
            }
        }
        AetherExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            for handler in handlers.iter_mut() {
                function_body(&mut handler.body, counted);
            }
            walk(body, pending, given, counted);
            if let Some(parameter) = parameter {
                walk(parameter, pending, true, counted);
            }
        }
        AetherExpr::Dup { var, body, .. } => {
            walk(body, pending, given, counted);
            release(var, pending, -1);
        }
        AetherExpr::Drop { var, body, .. } => {
            walk(body, pending, given, counted);
            release(var, pending, 1);
        }
        AetherExpr::Reuse { token, fields, .. } => {
            for field in fields.iter_mut().rev() {
                walk(field, pending, true, counted);
            }
            release(token, pending, 1);
        }
        // Each side releases the scrutinee itself: reuse on the unique side,
        // a drop after the copies on the shared side.
        AetherExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            let after = pending.clone();
            let mut unique = after.clone();
            walk(unique_body, &mut unique, given, counted);
            let mut shared = after.clone();
            walk(shared_body, &mut shared, given, counted);
            *pending = join(vec![unique, shared], after);
        }
    }
}

/// The binder a right-hand side names, under any copies of it.
fn bare_binder(rhs: &AetherExpr) -> Option<CoreBinderId> {
    match rhs {
        AetherExpr::Var { var, .. } => var.binder,
        AetherExpr::Dup { body, .. } => bare_binder(body),
        _ => None,
    }
}

/// Paths that meet: each releases what the value holds once, so they agree
/// on everything held before them. A path that keeps a value for a tail call
/// releases nothing, and the others say what the frame holds.
fn join(paths: Vec<Pending>, fallback: Pending) -> Pending {
    let mut paths = paths.into_iter();
    let Some(mut joined) = paths.next() else {
        return fallback;
    };
    for path in paths {
        for (binder, count) in path {
            let entry = joined.entry(binder).or_insert(count);
            *entry = (*entry).max(count);
        }
    }
    joined
}

/// Binders bound in `expr` that need a count.
fn collect_counted(expr: &AetherExpr, out: &mut HashSet<CoreBinderId>) {
    let bind = |binder: &CoreBinder, out: &mut HashSet<CoreBinderId>| {
        if binder.rep.needs_rc() {
            out.insert(binder.id);
        }
    };
    match expr {
        AetherExpr::Var { .. } | AetherExpr::Lit(..) => {}
        AetherExpr::Lam { params, body, .. } => {
            for param in params {
                bind(param, out);
            }
            collect_counted(body, out);
        }
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            collect_counted(func, out);
            for arg in args {
                collect_counted(arg, out);
            }
        }
        AetherExpr::Let { var, rhs, body, .. } | AetherExpr::LetRec { var, rhs, body, .. } => {
            bind(var, out);
            collect_counted(rhs, out);
            collect_counted(body, out);
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            for (var, rhs) in bindings {
                bind(var, out);
                collect_counted(rhs, out);
            }
            collect_counted(body, out);
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_counted(scrutinee, out);
            for alt in alts {
                collect_pattern(&alt.pat, out);
                if let Some(guard) = &alt.guard {
                    collect_counted(guard, out);
                }
                collect_counted(&alt.rhs, out);
            }
        }
        AetherExpr::Con { fields: args, .. }
        | AetherExpr::PrimOp { args, .. }
        | AetherExpr::Perform { args, .. }
        | AetherExpr::Reuse { fields: args, .. } => {
            for arg in args {
                collect_counted(arg, out);
            }
        }
        AetherExpr::MemberAccess { object, .. }
        | AetherExpr::TupleField { object, .. }
        | AetherExpr::Return { value: object, .. } => collect_counted(object, out),
        AetherExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            collect_counted(body, out);
            if let Some(parameter) = parameter {
                collect_counted(parameter, out);
            }
            for handler in handlers {
                for binder in handler
                    .params
                    .iter()
                    .chain(handler.state.iter())
                    .chain(std::iter::once(&handler.resume))
                {
                    bind(binder, out);
                }
                collect_counted(&handler.body, out);
            }
        }
        AetherExpr::Dup { body, .. } | AetherExpr::Drop { body, .. } => collect_counted(body, out),
        AetherExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            collect_counted(unique_body, out);
            collect_counted(shared_body, out);
        }
    }
}

fn collect_pattern(pat: &CorePat, out: &mut HashSet<CoreBinderId>) {
    match pat {
        CorePat::Var(binder) => {
            if binder.rep.needs_rc() {
                out.insert(binder.id);
            }
        }
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for field in fields {
                collect_pattern(field, out);
            }
        }
        CorePat::Lit(_) | CorePat::Wildcard | CorePat::EmptyList => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::aether::AetherExpr;
    use crate::aether::borrow_infer::BorrowMode;
    use crate::core::{CoreBinder, CoreBinderId, CoreLit, CorePrimOp, CoreVarRef, FluxRep};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    use super::annotate_owned_after;

    struct Names(Interner);

    impl Names {
        fn binder(&mut self, raw: u32, name: &str) -> CoreBinder {
            CoreBinder::new(CoreBinderId(raw), self.0.intern(name))
        }

        /// `name(args)`, each argument given away.
        fn call(&mut self, name: &str, args: Vec<AetherExpr>) -> AetherExpr {
            let arg_modes = vec![BorrowMode::Owned; args.len()];
            self.call_with(name, args, arg_modes)
        }

        fn call_with(
            &mut self,
            name: &str,
            args: Vec<AetherExpr>,
            arg_modes: Vec<BorrowMode>,
        ) -> AetherExpr {
            AetherExpr::AetherCall {
                func: Box::new(AetherExpr::Var {
                    var: CoreVarRef::unresolved(self.0.intern(name)),
                    span: Span::default(),
                }),
                guarded_borrowed_args: vec![false; args.len()],
                args,
                arg_modes,
                owned_after: Vec::new(),
                span: Span::default(),
            }
        }

        fn perform(&mut self) -> AetherExpr {
            AetherExpr::Perform {
                effect: self.0.intern("Ask"),
                operation: self.0.intern("ask"),
                args: vec![AetherExpr::Lit(CoreLit::Int(1), Span::default())],
                owned_after: Vec::new(),
                span: Span::default(),
            }
        }
    }

    fn var(binder: CoreBinder) -> AetherExpr {
        AetherExpr::Var {
            var: CoreVarRef::resolved(&binder),
            span: Span::default(),
        }
    }

    fn let_in(var: CoreBinder, rhs: AetherExpr, body: AetherExpr) -> AetherExpr {
        AetherExpr::Let {
            var,
            rhs: Box::new(rhs),
            body: Box::new(body),
            span: Span::default(),
        }
    }

    fn dup(binder: CoreBinder, body: AetherExpr) -> AetherExpr {
        AetherExpr::Dup {
            var: CoreVarRef::resolved(&binder),
            body: Box::new(body),
            span: Span::default(),
        }
    }

    fn drop(binder: CoreBinder, body: AetherExpr) -> AetherExpr {
        AetherExpr::Drop {
            var: CoreVarRef::resolved(&binder),
            body: Box::new(body),
            span: Span::default(),
        }
    }

    fn len(binder: CoreBinder) -> AetherExpr {
        AetherExpr::PrimOp {
            op: CorePrimOp::Len,
            args: vec![var(binder)],
            span: Span::default(),
        }
    }

    fn lam(params: Vec<CoreBinder>, body: AetherExpr) -> AetherExpr {
        AetherExpr::Lam {
            params,
            param_types: vec![],
            result_ty: None,
            body: Box::new(body),
            span: Span::default(),
        }
    }

    /// `owned_after` of the call or effect bound to `k`.
    fn held_at(expr: &AetherExpr, k: CoreBinderId) -> Vec<CoreBinderId> {
        match expr {
            AetherExpr::Let { var, rhs, .. } if var.id == k => match rhs.as_ref() {
                AetherExpr::AetherCall { owned_after, .. }
                | AetherExpr::Perform { owned_after, .. } => owned_after.clone(),
                other => panic!("`k` is bound to {other:?}"),
            },
            AetherExpr::Let { body, .. }
            | AetherExpr::Lam { body, .. }
            | AetherExpr::Dup { body, .. }
            | AetherExpr::Drop { body, .. } => held_at(body, k),
            other => panic!("no `k` in {other:?}"),
        }
    }

    /// `fn(p) let xs = g() in let k = f() in (let n = len(xs) in drop xs in
    /// n + len(p))`: the frame made `xs` and releases it after the call;
    /// it only reads the borrowed `p`.
    #[test]
    fn a_value_released_after_a_call_is_held_there() {
        let mut names = Names(Interner::new());
        let [p, xs, n] = [(1, "p"), (2, "xs"), (4, "n")].map(|(id, name)| names.binder(id, name));
        let k = CoreBinder::with_rep(CoreBinderId(3), names.0.intern("k"), FluxRep::IntRep);
        let g = names.call("g", vec![]);
        let f = names.call("f", vec![]);
        let sum = AetherExpr::PrimOp {
            op: CorePrimOp::Add,
            args: vec![var(n), len(p)],
            span: Span::default(),
        };
        let expr = lam(
            vec![p],
            let_in(xs, g, let_in(k, f, let_in(n, len(xs), drop(xs, sum)))),
        );

        let annotated = annotate_owned_after(expr);
        assert_eq!(held_at(&annotated, k.id), vec![xs.id]);
    }

    /// `let xs = g() in dup xs in let k = f() in h(xs, xs)`: two references
    /// are given away after the call, so the frame holds both.
    #[test]
    fn each_reference_given_away_after_a_call_is_held_there() {
        let mut names = Names(Interner::new());
        let xs = names.binder(1, "xs");
        let k = CoreBinder::with_rep(CoreBinderId(2), names.0.intern("k"), FluxRep::IntRep);
        let g = names.call("g", vec![]);
        let f = names.call("f", vec![]);
        let h = names.call("h", vec![var(xs), var(xs)]);
        let expr = lam(vec![], let_in(xs, g, dup(xs, let_in(k, f, h))));

        let annotated = annotate_owned_after(expr);
        assert_eq!(held_at(&annotated, k.id), vec![xs.id, xs.id]);
    }

    /// `let xs = g() in let k = f() in h(dup xs in xs) ; drop xs`: the copy
    /// made after the call is the one given away, so the frame holds one.
    /// A value only lent after the call, `y = xs` read by `l`, holds none.
    #[test]
    fn a_copy_made_after_a_call_is_not_held_there() {
        let mut names = Names(Interner::new());
        let [xs, r] = [(1, "xs"), (3, "r")].map(|(id, name)| names.binder(id, name));
        let k = CoreBinder::with_rep(CoreBinderId(2), names.0.intern("k"), FluxRep::IntRep);
        let g = names.call("g", vec![]);
        let f = names.call("f", vec![]);
        let h = names.call("h", vec![dup(xs, var(xs))]);
        let expr = lam(
            vec![],
            let_in(xs, g, let_in(k, f, let_in(r, h, drop(xs, var(r))))),
        );

        let annotated = annotate_owned_after(expr);
        assert_eq!(held_at(&annotated, k.id), vec![xs.id]);

        let mut names = Names(Interner::new());
        let [p, y] = [(1, "p"), (3, "y")].map(|(id, name)| names.binder(id, name));
        let k = CoreBinder::with_rep(CoreBinderId(2), names.0.intern("k"), FluxRep::IntRep);
        let f = names.call("f", vec![]);
        let l = names.call_with("l", vec![var(y)], vec![BorrowMode::Borrowed]);
        let expr = lam(vec![p], let_in(k, f, let_in(y, var(p), l)));

        let annotated = annotate_owned_after(expr);
        assert_eq!(held_at(&annotated, k.id), Vec::<CoreBinderId>::new());
    }

    /// `let xs = g() in let k = perform Ask.ask(1) in drop xs in k`.
    #[test]
    fn a_value_released_after_an_effect_is_held_there() {
        let mut names = Names(Interner::new());
        let xs = names.binder(1, "xs");
        let k = CoreBinder::with_rep(CoreBinderId(2), names.0.intern("k"), FluxRep::IntRep);
        let g = names.call("g", vec![]);
        let ask = names.perform();
        let expr = lam(vec![], let_in(xs, g, let_in(k, ask, drop(xs, var(k)))));

        let annotated = annotate_owned_after(expr);
        assert_eq!(held_at(&annotated, k.id), vec![xs.id]);
    }
}
