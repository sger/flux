//! What the simplifier keeps in a debug build (proposal 0188 S9c).
//!
//! A `-g` build runs every Core pass, as a plain build does, except that a
//! binder the program wrote is never substituted away: where a pass would
//! inline a `let`, a lambda's parameter or a pattern variable the program
//! named, it binds it with a `let` instead, so a native debugger shows each
//! variable and stops on each line, as the VM debugger does. Lambdas the
//! simplifier still inlines are recorded, so DWARF can describe their code
//! as inlined.
//!
//! The passes walk with plain `fn` pointers, so the policy for the run in
//! progress is this thread's, set for the run by [`keeping_source_binders`].

use std::cell::RefCell;
use std::collections::HashSet;

use crate::core::{CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CorePat, CoreProgram, InlineSite};
use crate::diagnostics::position::Span;
use crate::syntax::interner::Interner;

thread_local! {
    static KEPT: RefCell<Option<HashSet<CoreBinderId>>> = const { RefCell::new(None) };
    static INLINED: RefCell<Vec<InlineSite>> = const { RefCell::new(Vec::new()) };
}

/// Whether the run in progress keeps this binder: the program wrote it, and
/// this is a debug build.
pub(super) fn keeps(binder: CoreBinderId) -> bool {
    KEPT.with(|kept| kept.borrow().as_ref().is_some_and(|kept| kept.contains(&binder)))
}

/// Whether this run keeps source binders at all.
pub(super) fn is_keeping() -> bool {
    KEPT.with(|kept| kept.borrow().is_some())
}

/// Notes that the lambda at `lambda` was inlined where it was applied, at
/// `call`.
pub(super) fn note_inlined(lambda: Span, call: Span) {
    if is_keeping() && lambda != Span::default() && call != Span::default() {
        INLINED.with(|sites| {
            let site = InlineSite { lambda, call };
            let mut sites = sites.borrow_mut();
            if !sites.contains(&site) {
                sites.push(site);
            }
        });
    }
}

/// The binders `program` wrote.
pub(super) fn source_binders(program: &CoreProgram, interner: &Interner) -> HashSet<CoreBinderId> {
    let mut kept = HashSet::new();
    for def in &program.defs {
        collect(&def.expr, interner, &mut kept);
    }
    kept
}

/// Runs `run` keeping the binders in `kept`, and returns what it returned
/// with the lambdas it inlined.
pub(super) fn keeping<R>(
    kept: HashSet<CoreBinderId>,
    run: impl FnOnce() -> R,
) -> (R, Vec<InlineSite>) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            KEPT.with(|kept| kept.borrow_mut().take());
            INLINED.with(|sites| sites.borrow_mut().clear());
        }
    }
    KEPT.with(|slot| *slot.borrow_mut() = Some(kept));
    let _reset = Reset;
    let result = run();
    let sites = INLINED.with(|sites| std::mem::take(&mut *sites.borrow_mut()));
    (result, sites)
}

/// A binder the program wrote: one with a name of its own, bound at a place
/// in the source. Names starting `__` are the compiler's (as `emit_debug`
/// marks them artificial).
fn is_source(binder: &CoreBinder, span: Span, interner: &Interner) -> bool {
    span != Span::default()
        && interner
            .try_resolve(binder.name)
            .is_some_and(|name| !name.starts_with("__") && !name.starts_with('%'))
}

fn collect(expr: &CoreExpr, interner: &Interner, kept: &mut HashSet<CoreBinderId>) {
    let keep = |binder: &CoreBinder, span: Span, kept: &mut HashSet<CoreBinderId>| {
        if is_source(binder, span, interner) {
            kept.insert(binder.id);
        }
    };
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(..) => {}
        CoreExpr::Lam { params, body, span, .. } => {
            for param in params {
                keep(param, *span, kept);
            }
            collect(body, interner, kept);
        }
        CoreExpr::App { func, args, .. } => {
            collect(func, interner, kept);
            for arg in args {
                collect(arg, interner, kept);
            }
        }
        CoreExpr::Let { var, rhs, body, span } | CoreExpr::LetRec { var, rhs, body, span } => {
            keep(var, *span, kept);
            collect(rhs, interner, kept);
            collect(body, interner, kept);
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            for (_, rhs) in bindings {
                collect(rhs, interner, kept);
            }
            collect(body, interner, kept);
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            collect(scrutinee, interner, kept);
            for CoreAlt { pat, guard, rhs, span } in alts {
                collect_pat(pat, *span, interner, kept);
                if let Some(guard) = guard {
                    collect(guard, interner, kept);
                }
                collect(rhs, interner, kept);
            }
        }
        CoreExpr::Con { fields: args, .. }
        | CoreExpr::PrimOp { args, .. }
        | CoreExpr::Perform { args, .. } => {
            for arg in args {
                collect(arg, interner, kept);
            }
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            collect(object, interner, kept);
        }
        CoreExpr::Return { value, .. } => collect(value, interner, kept),
        CoreExpr::Handle { body, parameter, handlers, .. } => {
            collect(body, interner, kept);
            if let Some(parameter) = parameter {
                collect(parameter, interner, kept);
            }
            for handler in handlers {
                collect(&handler.body, interner, kept);
            }
        }
    }
}

fn collect_pat(pat: &CorePat, span: Span, interner: &Interner, kept: &mut HashSet<CoreBinderId>) {
    match pat {
        CorePat::Var(binder) => {
            if is_source(binder, span, interner) {
                kept.insert(binder.id);
            }
        }
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for field in fields {
                collect_pat(field, span, interner, kept);
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
}
