//! The largest values the backends can build. A native heap object counts
//! the fields its release walks in one byte, and the VM's instructions count
//! a constructor's fields and a closure's captures in one byte too, so a
//! constructor, tuple or closure with more than 255 is rejected here, before
//! either backend sees it.

use std::collections::HashSet;

use crate::core::to_ir::collect_free_vars_core;
use crate::core::{CoreBinderId, CoreExpr, CorePat, CorePrimOp, CoreProgram, CoreTopLevelItem};
use crate::diagnostics::Diagnostic;
use crate::diagnostics::compiler_errors::too_many_fields;

const MAX_FIELDS: usize = 255;

#[allow(clippy::result_large_err)]
pub(super) fn check_field_counts(program: &CoreProgram) -> Result<(), Diagnostic> {
    check_declarations(&program.top_level_items)?;
    for def in &program.defs {
        let mut local = HashSet::new();
        bound_in(&def.expr, &mut local);
        check(&def.expr, &local)?;
    }
    Ok(())
}

/// A user constructor is a call until the backend builds it, so its field
/// count is checked where it is declared.
#[allow(clippy::result_large_err)]
fn check_declarations(items: &[CoreTopLevelItem]) -> Result<(), Diagnostic> {
    for item in items {
        match item {
            CoreTopLevelItem::Data { variants, .. } => {
                for variant in variants {
                    if variant.fields.len() > MAX_FIELDS {
                        return Err(too_many_fields(
                            variant.span,
                            "constructor",
                            "fields",
                            variant.fields.len(),
                        ));
                    }
                }
            }
            CoreTopLevelItem::Module { body, .. } => check_declarations(body)?,
            _ => {}
        }
    }
    Ok(())
}

/// `local` holds the binders the definition binds itself: only those are
/// captured. A top-level or imported function a closure names is not.
#[allow(clippy::result_large_err)]
fn check(expr: &CoreExpr, local: &HashSet<CoreBinderId>) -> Result<(), Diagnostic> {
    let at_most = |count: usize, what: &str, items: &str, span| {
        if count > MAX_FIELDS {
            Err(too_many_fields(span, what, items, count))
        } else {
            Ok(())
        }
    };
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(..) => Ok(()),
        CoreExpr::Lam { body, span, .. } => {
            let captures = collect_free_vars_core(expr)
                .into_iter()
                .filter(|id| local.contains(id))
                .count();
            at_most(captures, "closure", "captured values", *span)?;
            check(body, local)
        }
        CoreExpr::Con { fields, span, .. } => {
            at_most(fields.len(), "constructor", "fields", *span)?;
            fields.iter().try_for_each(|field| check(field, local))
        }
        CoreExpr::PrimOp { op, args, span } => {
            if *op == CorePrimOp::MakeTuple {
                at_most(args.len(), "tuple", "elements", *span)?;
            }
            args.iter().try_for_each(|arg| check(arg, local))
        }
        CoreExpr::App { func, args, .. } => {
            check(func, local)?;
            args.iter().try_for_each(|arg| check(arg, local))
        }
        CoreExpr::Perform { args, .. } => args.iter().try_for_each(|arg| check(arg, local)),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            check(rhs, local)?;
            check(body, local)
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            bindings.iter().try_for_each(|(_, rhs)| check(rhs, local))?;
            check(body, local)
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            check(scrutinee, local)?;
            alts.iter().try_for_each(|alt| {
                if let Some(guard) = &alt.guard {
                    check(guard, local)?;
                }
                check(&alt.rhs, local)
            })
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            check(object, local)
        }
        CoreExpr::Return { value, .. } => check(value, local),
        CoreExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            check(body, local)?;
            if let Some(parameter) = parameter {
                check(parameter, local)?;
            }
            handlers
                .iter()
                .try_for_each(|handler| check(&handler.body, local))
        }
    }
}

fn bound_in(expr: &CoreExpr, out: &mut HashSet<CoreBinderId>) {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(..) => {}
        CoreExpr::Lam { params, body, .. } => {
            out.extend(params.iter().map(|param| param.id));
            bound_in(body, out);
        }
        CoreExpr::Con { fields: args, .. }
        | CoreExpr::PrimOp { args, .. }
        | CoreExpr::Perform { args, .. } => {
            for arg in args {
                bound_in(arg, out);
            }
        }
        CoreExpr::App { func, args, .. } => {
            bound_in(func, out);
            for arg in args {
                bound_in(arg, out);
            }
        }
        CoreExpr::Let { var, rhs, body, .. } | CoreExpr::LetRec { var, rhs, body, .. } => {
            out.insert(var.id);
            bound_in(rhs, out);
            bound_in(body, out);
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            for (var, rhs) in bindings {
                out.insert(var.id);
                bound_in(rhs, out);
            }
            bound_in(body, out);
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            bound_in(scrutinee, out);
            for alt in alts {
                pattern_binders(&alt.pat, out);
                if let Some(guard) = &alt.guard {
                    bound_in(guard, out);
                }
                bound_in(&alt.rhs, out);
            }
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            bound_in(object, out)
        }
        CoreExpr::Return { value, .. } => bound_in(value, out),
        CoreExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            bound_in(body, out);
            if let Some(parameter) = parameter {
                bound_in(parameter, out);
            }
            for handler in handlers {
                out.extend(handler.params.iter().map(|param| param.id));
                out.extend(handler.state.iter().map(|state| state.id));
                out.insert(handler.resume.id);
                bound_in(&handler.body, out);
            }
        }
    }
}

fn pattern_binders(pat: &CorePat, out: &mut HashSet<CoreBinderId>) {
    match pat {
        CorePat::Var(binder) => {
            out.insert(binder.id);
        }
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for field in fields {
                pattern_binders(field, out);
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
}
