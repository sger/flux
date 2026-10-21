//! Aether verification.
//!
//! This module has two responsibilities:
//! - `verify_contract`: fatal semantic-contract checks for emitted Aether Core
//! - `verify_diagnostics`: optional non-fatal optimization diagnostics

use std::collections::{HashMap, HashSet};

use crate::core::{CoreBinder, CoreBinderId, CoreExpr, CorePat, CorePrimOp, CoreTag};

use super::analysis::use_counts_aether;
use super::reuse_analysis::diagnose_drop_body;
use super::{AetherExpr, constructor_shape_for_tag_aether, is_heap_tag};

/// Fatal Aether verification error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AetherError {
    pub kind: AetherErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AetherErrorKind {
    UnresolvedAetherVar,
    UnsafeDrop,
    InvalidAetherCallModes,
    InvalidAetherCallGuards,
    InvalidReuseTag,
    ReuseTokenEscapesIntoFields,
    InvalidFieldMask,
    InvalidDropSpecializedScrutinee,
    InvalidDropSpecializedUse,
    /// A binder released twice on one path: dropped, reused or
    /// drop-specialized after it already was.
    DoubleDrop,
    /// A parameter the definition's borrow signature marks Borrowed is
    /// released (dropped, reused or drop-specialized): its caller still
    /// holds it.
    BorrowedParamReleased,
}

/// Non-fatal Aether diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AetherDiagnostic {
    pub kind: AetherDiagnosticKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AetherDiagnosticKind {
    /// Case arm destructures value and reconstructs same shape — could reuse.
    MissedReuse,
    /// A view (a value read out of something else that holds it, with no
    /// reference of its own) given away without a copy (0189 rule 3).
    ViewConsumed,
}

/// Verify the fatal Aether contract for an expression.
pub fn verify_contract(expr: &CoreExpr) -> Result<(), Vec<AetherError>> {
    verify_contract_aether(&AetherExpr::from_core(expr.clone()))
}

/// Verify the fatal Aether contract for a backend-only Aether expression.
pub fn verify_contract_aether(expr: &AetherExpr) -> Result<(), Vec<AetherError>> {
    let mut errors = Vec::new();
    check_contract(expr, &mut errors);
    check_single_release(expr, &mut Vec::new(), &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Verify the fatal Aether contract for a definition: the expression's, and
/// that no parameter its borrow `signature` marks Borrowed is released.
pub fn verify_def_contract_aether(
    expr: &AetherExpr,
    signature: Option<&super::borrow_infer::BorrowSignature>,
) -> Result<(), Vec<AetherError>> {
    let mut errors = match verify_contract_aether(expr) {
        Ok(()) => Vec::new(),
        Err(errors) => errors,
    };
    if let (AetherExpr::Lam { params, body, .. }, Some(signature)) = (expr, signature) {
        let borrowed = borrowed_params(params, signature);
        if !borrowed.is_empty() {
            check_borrowed_kept(body, &borrowed, &mut errors);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// The parameters `signature` marks Borrowed.
fn borrowed_params(
    params: &[CoreBinder],
    signature: &super::borrow_infer::BorrowSignature,
) -> Vec<CoreBinderId> {
    params
        .iter()
        .zip(&signature.params)
        .filter(|(_, mode)| **mode == super::borrow_infer::BorrowMode::Borrowed)
        .map(|(param, _)| param.id)
        .collect()
}

/// Reports every view of a definition given away without a copy: proposal
/// 0189's rule 3, which the planner does not meet everywhere yet (0189 F3-2),
/// so this is a diagnostic to count, not part of the contract.
pub fn verify_def_views_aether(
    expr: &AetherExpr,
    signature: Option<&super::borrow_infer::BorrowSignature>,
) -> Vec<AetherDiagnostic> {
    let mut diags = Vec::new();
    let mut views = Views::default();
    match expr {
        AetherExpr::Lam { params, body, .. } => {
            let borrowed = signature
                .map(|signature| borrowed_params(params, signature))
                .unwrap_or_default();
            for param in params {
                views.bind(param, borrowed.contains(&param.id));
            }
            check_views(body, true, &mut views, &mut diags);
        }
        _ => check_views(expr, true, &mut views, &mut diags),
    }
    diags
}

/// What `check_views` knows on the path it walks.
#[derive(Clone, Default)]
struct Views {
    /// Binders bound in the definition, and whether each is counted: only
    /// these can be a lambda's or a handler's captures.
    locals: HashMap<CoreBinderId, bool>,
    /// The views on this path.
    binders: HashSet<CoreBinderId>,
    /// Copies made of a view and not yet given away on this path.
    copies: HashMap<CoreBinderId, usize>,
    /// The scrutinee each top-level pattern field was read from.
    parents: HashMap<CoreBinderId, CoreBinderId>,
}

impl Views {
    fn bind(&mut self, binder: &CoreBinder, view: bool) {
        let counted = binder.rep.needs_rc();
        self.locals.insert(binder.id, counted);
        if view && counted {
            self.binders.insert(binder.id);
        }
    }

    /// A lambda's or a handler clause's body: the closure holds its own
    /// reference to each capture, so inside the body a capture is a view.
    fn function(&self, body: &AetherExpr, own: &[CoreBinder]) -> Views {
        let mut inner = Views {
            locals: self.locals.clone(),
            ..Views::default()
        };
        for id in super::free_vars::collect_free_vars_aether(body) {
            if self.locals.get(&id) == Some(&true) && !own.iter().any(|b| b.id == id) {
                inner.binders.insert(id);
            }
        }
        inner
    }

    fn give_away(&mut self, binder: CoreBinderId, diags: &mut Vec<AetherDiagnostic>) {
        if !self.binders.contains(&binder) {
            return;
        }
        match self.copies.get_mut(&binder) {
            Some(copies) if *copies > 0 => *copies -= 1,
            _ => diags.push(AetherDiagnostic {
                kind: AetherDiagnosticKind::ViewConsumed,
                message: format!("view {binder:?} is given away without a copy"),
            }),
        }
    }
}

/// Walks `expr`, whose value is given away when `owned`, reporting each view
/// given away with no `Dup` of it before.
fn check_views(
    expr: &AetherExpr,
    owned: bool,
    views: &mut Views,
    diags: &mut Vec<AetherDiagnostic>,
) {
    match expr {
        AetherExpr::Var { var, .. } => {
            if owned && let Some(binder) = var.binder {
                views.give_away(binder, diags);
            }
        }
        AetherExpr::Lit(_, _) => {}
        AetherExpr::Lam { params, body, .. } => {
            let mut inner = views.function(body, params);
            for param in params {
                inner.bind(param, false);
            }
            check_views(body, true, &mut inner, diags);
        }
        AetherExpr::App { func, args, .. } => {
            check_views(func, false, views, diags);
            for arg in args {
                check_views(arg, true, views, diags);
            }
        }
        AetherExpr::AetherCall {
            func,
            args,
            arg_modes,
            ..
        } => {
            check_views(func, false, views, diags);
            for (i, arg) in args.iter().enumerate() {
                let lent = arg_modes.get(i) == Some(&super::borrow_infer::BorrowMode::Borrowed);
                check_views(arg, !lent, views, diags);
            }
        }
        AetherExpr::Let { var, rhs, body, .. } => {
            match rhs.as_ref() {
                // Another name for a view, or a field read out of a tuple.
                AetherExpr::Var { var: of, .. }
                    if of.binder.is_some_and(|b| views.binders.contains(&b)) =>
                {
                    views.bind(var, true);
                }
                AetherExpr::TupleField { object, .. }
                    if matches!(object.as_ref(), AetherExpr::Var { .. }) =>
                {
                    views.bind(var, true);
                }
                // Whether the bound value is given away depends on the body;
                // only what it gives away inside is checked.
                _ => {
                    check_views(rhs, false, views, diags);
                    views.bind(var, false);
                }
            }
            check_views(body, owned, views, diags);
        }
        AetherExpr::LetRec { var, rhs, body, .. } => {
            views.bind(var, false);
            check_views(rhs, false, views, diags);
            check_views(body, owned, views, diags);
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            for (var, _) in bindings {
                views.bind(var, false);
            }
            for (_, rhs) in bindings {
                check_views(rhs, false, views, diags);
            }
            check_views(body, owned, views, diags);
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            check_views(scrutinee, false, views, diags);
            let parent = match scrutinee.as_ref() {
                AetherExpr::Var { var, .. } => var.binder,
                _ => None,
            };
            let mut joined: Option<HashMap<CoreBinderId, usize>> = None;
            for alt in alts {
                let mut arm = views.clone();
                match &alt.pat {
                    // The matched value itself, under another name.
                    CorePat::Var(binder) => {
                        let view = parent.is_some_and(|p| views.binders.contains(&p));
                        arm.bind(binder, view);
                    }
                    pat => {
                        let mut binders = Vec::new();
                        pattern_binders(pat, &mut binders);
                        for binder in binders {
                            arm.bind(binder, true);
                        }
                        // A nested field is held by an inner cell, which the
                        // outer cell's unique path does not own.
                        if let (
                            Some(parent),
                            CorePat::Con { fields, .. } | CorePat::Tuple(fields),
                        ) = (parent, pat)
                        {
                            for field in fields {
                                if let CorePat::Var(binder) = field {
                                    arm.parents.insert(binder.id, parent);
                                }
                            }
                        }
                    }
                }
                if let Some(guard) = &alt.guard {
                    check_views(guard, false, &mut arm, diags);
                }
                check_views(&alt.rhs, owned, &mut arm, diags);
                joined = Some(match joined {
                    None => arm.copies,
                    Some(copies) => fewest(copies, &arm.copies),
                });
            }
            // What follows the match runs after any one arm.
            if let Some(copies) = joined {
                views.copies = copies;
            }
        }
        AetherExpr::Con { fields, .. } | AetherExpr::Reuse { fields, .. } => {
            for field in fields {
                check_views(field, true, views, diags);
            }
        }
        AetherExpr::PrimOp { op, args, .. } => {
            let stores = matches!(
                op,
                CorePrimOp::MakeTuple
                    | CorePrimOp::MakeList
                    | CorePrimOp::MakeArray
                    | CorePrimOp::MakeHash
            );
            for arg in args {
                check_views(arg, stores, views, diags);
            }
        }
        AetherExpr::Perform { args, .. } => {
            for arg in args {
                check_views(arg, true, views, diags);
            }
        }
        AetherExpr::Return { value, .. } => check_views(value, true, views, diags),
        AetherExpr::MemberAccess { object, .. } => check_views(object, false, views, diags),
        AetherExpr::TupleField { object, index, .. } => {
            check_views(object, false, views, diags);
            if owned
                && let AetherExpr::Var { var, .. } = object.as_ref()
                && let Some(binder) = var.binder
            {
                diags.push(AetherDiagnostic {
                    kind: AetherDiagnosticKind::ViewConsumed,
                    message: format!("field {index} of {binder:?} is given away without a copy"),
                });
            }
        }
        AetherExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            if let Some(parameter) = parameter {
                check_views(parameter, true, views, diags);
            }
            check_views(body, owned, views, diags);
            // The runtime lends a clause its arguments, state and `resume`,
            // and releases them after it returns (`runtime/c/effects.c`).
            for handler in handlers {
                let own: Vec<CoreBinder> = handler
                    .params
                    .iter()
                    .chain(&handler.state)
                    .chain([&handler.resume])
                    .copied()
                    .collect();
                let mut inner = views.function(&handler.body, &own);
                for binder in &own {
                    inner.bind(binder, true);
                }
                check_views(&handler.body, true, &mut inner, diags);
            }
        }
        AetherExpr::Dup { var, body, .. } => {
            if let Some(binder) = var.binder {
                *views.copies.entry(binder).or_default() += 1;
            }
            check_views(body, owned, views, diags);
        }
        AetherExpr::Drop { body, .. } => check_views(body, owned, views, diags),
        AetherExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            // On the unique side the cell is this path's alone: it hands the
            // arm its one reference to each field it holds, in place of the
            // copy drop_spec removed. A second use still needs a copy.
            let mut unique = views.clone();
            if let Some(cell) = scrutinee.binder {
                for (field, _) in views.parents.iter().filter(|(_, p)| **p == cell) {
                    *unique.copies.entry(*field).or_default() += 1;
                }
            }
            check_views(unique_body, owned, &mut unique, diags);
            let mut shared = views.clone();
            check_views(shared_body, owned, &mut shared, diags);
            views.copies = fewest(unique.copies, &shared.copies);
        }
    }
}

/// The copies left on both of two paths that join.
fn fewest(
    mut copies: HashMap<CoreBinderId, usize>,
    other: &HashMap<CoreBinderId, usize>,
) -> HashMap<CoreBinderId, usize> {
    copies.retain(|binder, n| {
        *n = (*n).min(other.get(binder).copied().unwrap_or(0));
        *n > 0
    });
    copies
}

fn pattern_binders<'a>(pat: &'a CorePat, out: &mut Vec<&'a CoreBinder>) {
    match pat {
        CorePat::Var(binder) => out.push(binder),
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for field in fields {
                pattern_binders(field, out);
            }
        }
        CorePat::Lit(_) | CorePat::Wildcard | CorePat::EmptyList => {}
    }
}

/// Reports every release of one of the `borrowed` parameters in `expr`.
fn check_borrowed_kept(
    expr: &AetherExpr,
    borrowed: &[crate::core::CoreBinderId],
    errors: &mut Vec<AetherError>,
) {
    let released = |var: &crate::core::CoreVarRef| {
        var.binder
            .is_some_and(|binder| borrowed.contains(&binder))
            .then(|| AetherError {
                kind: AetherErrorKind::BorrowedParamReleased,
                message: format!(
                    "borrowed parameter {:?} is released by the function that borrows it",
                    var.binder
                ),
            })
    };
    let mut walk = |child: &AetherExpr| check_borrowed_kept(child, borrowed, errors);
    match expr {
        AetherExpr::Var { .. } | AetherExpr::Lit(_, _) => {}
        AetherExpr::Lam { body, .. }
        | AetherExpr::Return { value: body, .. }
        | AetherExpr::Dup { body, .. } => walk(body),
        AetherExpr::Drop { var, body, .. } => {
            errors.extend(released(var));
            check_borrowed_kept(body, borrowed, errors);
        }
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            walk(func);
            args.iter().for_each(walk);
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            walk(rhs);
            walk(body);
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            bindings.iter().for_each(|(_, rhs)| walk(rhs));
            walk(body);
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            walk(scrutinee);
            for alt in alts {
                if let Some(guard) = &alt.guard {
                    walk(guard);
                }
                walk(&alt.rhs);
            }
        }
        AetherExpr::Con { fields: args, .. }
        | AetherExpr::PrimOp { args, .. }
        | AetherExpr::Perform { args, .. } => args.iter().for_each(walk),
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            walk(object)
        }
        AetherExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            if let Some(parameter) = parameter {
                walk(parameter);
            }
            walk(body);
            handlers.iter().for_each(|handler| walk(&handler.body));
        }
        AetherExpr::Reuse { token, fields, .. } => {
            errors.extend(released(token));
            fields
                .iter()
                .for_each(|field| check_borrowed_kept(field, borrowed, errors));
        }
        AetherExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            errors.extend(released(scrutinee));
            check_borrowed_kept(unique_body, borrowed, errors);
            check_borrowed_kept(shared_body, borrowed, errors);
        }
    }
}

/// Verify optional non-fatal diagnostics for an expression.
pub fn verify_diagnostics(expr: &CoreExpr) -> Vec<AetherDiagnostic> {
    verify_diagnostics_aether(&AetherExpr::from_core(expr.clone()))
}

/// Verify optional non-fatal diagnostics for a backend-only Aether expression.
pub fn verify_diagnostics_aether(expr: &AetherExpr) -> Vec<AetherDiagnostic> {
    let mut diags = Vec::new();
    check_diagnostics(expr, &mut diags);
    diags
}

/// Checks that no path releases a binder twice. `released` holds the
/// binders released so far on the path being walked, in evaluation order;
/// a case's arms each continue from it, and what follows the case continues
/// from every arm.
fn check_single_release(
    expr: &AetherExpr,
    released: &mut Vec<crate::core::CoreBinderId>,
    errors: &mut Vec<AetherError>,
) {
    let release = |var: &crate::core::CoreVarRef,
                   released: &mut Vec<crate::core::CoreBinderId>,
                   errors: &mut Vec<AetherError>| {
        let Some(binder) = var.binder else {
            return;
        };
        if released.contains(&binder) {
            errors.push(AetherError {
                kind: AetherErrorKind::DoubleDrop,
                message: format!("binder {binder:?} is released twice on one path"),
            });
        } else {
            released.push(binder);
        }
    };
    match expr {
        AetherExpr::Var { .. } | AetherExpr::Lit(_, _) => {}
        // Another function: its own paths.
        AetherExpr::Lam { body, .. } => check_single_release(body, &mut Vec::new(), errors),
        AetherExpr::Return { value, .. } => check_single_release(value, released, errors),
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            check_single_release(func, released, errors);
            for arg in args {
                check_single_release(arg, released, errors);
            }
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            check_single_release(rhs, released, errors);
            check_single_release(body, released, errors);
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            for (_, rhs) in bindings {
                check_single_release(rhs, released, errors);
            }
            check_single_release(body, released, errors);
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            check_single_release(scrutinee, released, errors);
            let entry = released.clone();
            for alt in alts {
                let mut path = entry.clone();
                if let Some(guard) = &alt.guard {
                    check_single_release(guard, &mut path, errors);
                }
                check_single_release(&alt.rhs, &mut path, errors);
                for binder in path {
                    if !released.contains(&binder) {
                        released.push(binder);
                    }
                }
            }
        }
        AetherExpr::Con { fields: args, .. }
        | AetherExpr::PrimOp { args, .. }
        | AetherExpr::Perform { args, .. } => {
            for arg in args {
                check_single_release(arg, released, errors);
            }
        }
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            check_single_release(object, released, errors)
        }
        AetherExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            if let Some(parameter) = parameter {
                check_single_release(parameter, released, errors);
            }
            check_single_release(body, released, errors);
            // Each clause runs as a function of its own.
            for handler in handlers {
                check_single_release(&handler.body, &mut Vec::new(), errors);
            }
        }
        AetherExpr::Dup { body, .. } => check_single_release(body, released, errors),
        AetherExpr::Drop { var, body, .. } => {
            release(var, released, errors);
            check_single_release(body, released, errors);
        }
        AetherExpr::Reuse { token, fields, .. } => {
            for field in fields {
                check_single_release(field, released, errors);
            }
            release(token, released, errors);
        }
        AetherExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            // The scrutinee is released once on each side: the unique side
            // may reuse it (its `Reuse` is that release) or frees it, the
            // shared side gives up its reference on entry.
            let entry = released.clone();
            if scrutinee
                .binder
                .is_some_and(|binder| entry.contains(&binder))
            {
                release(scrutinee, &mut entry.clone(), errors);
            }
            let mut unique = entry.clone();
            check_single_release(unique_body, &mut unique, errors);
            let mut shared = entry.clone();
            release(scrutinee, &mut shared, errors);
            check_single_release(shared_body, &mut shared, errors);
            for binder in unique.into_iter().chain(shared).chain(scrutinee.binder) {
                if !released.contains(&binder) {
                    released.push(binder);
                }
            }
        }
    }
}

fn check_contract(expr: &AetherExpr, errors: &mut Vec<AetherError>) {
    match expr {
        AetherExpr::Var { .. } | AetherExpr::Lit(_, _) => {}
        AetherExpr::Lam { body, .. } | AetherExpr::Return { value: body, .. } => {
            check_contract(body, errors)
        }
        AetherExpr::App { func, args, .. } => {
            check_contract(func, errors);
            for arg in args {
                check_contract(arg, errors);
            }
        }
        AetherExpr::AetherCall {
            func,
            args,
            arg_modes,
            guarded_borrowed_args,
            ..
        } => {
            if args.len() != arg_modes.len() {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidAetherCallModes,
                    message: format!(
                        "aether_call argument count {} does not match mode count {}",
                        args.len(),
                        arg_modes.len()
                    ),
                });
            }
            if args.len() != guarded_borrowed_args.len() {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidAetherCallGuards,
                    message: format!(
                        "aether_call argument count {} does not match guard count {}",
                        args.len(),
                        guarded_borrowed_args.len()
                    ),
                });
            }
            check_contract(func, errors);
            for arg in args {
                check_contract(arg, errors);
            }
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            check_contract(rhs, errors);
            check_contract(body, errors);
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            for (_, rhs) in bindings {
                check_contract(rhs, errors);
            }
            check_contract(body, errors);
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            check_contract(scrutinee, errors);
            for alt in alts {
                if let Some(guard) = &alt.guard {
                    check_contract(guard, errors);
                }
                check_contract(&alt.rhs, errors);
            }
        }
        AetherExpr::Con { fields, .. } => {
            for field in fields {
                check_contract(field, errors);
            }
        }
        AetherExpr::PrimOp { args, .. } | AetherExpr::Perform { args, .. } => {
            for arg in args {
                check_contract(arg, errors);
            }
        }
        AetherExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            if let Some(parameter) = parameter {
                check_contract(parameter, errors);
            }
            check_contract(body, errors);
            for handler in handlers {
                check_contract(&handler.body, errors);
            }
        }
        AetherExpr::Dup { var, body, .. } => {
            if var.binder.is_none() {
                errors.push(AetherError {
                    kind: AetherErrorKind::UnresolvedAetherVar,
                    message: format!("dup uses unresolved variable `{}`", var.name.as_u32()),
                });
            }
            check_contract(body, errors);
        }
        AetherExpr::Drop { var, body, .. } => {
            if let Some(id) = var.binder {
                if let Some(&count) = use_counts_aether(body).get(&id)
                    && count > 0
                {
                    errors.push(AetherError {
                        kind: AetherErrorKind::UnsafeDrop,
                        message: format!(
                            "drop of `{}` is unsafe: variable still has {} use(s) in body",
                            var.name.as_u32(),
                            count
                        ),
                    });
                }
            } else {
                errors.push(AetherError {
                    kind: AetherErrorKind::UnresolvedAetherVar,
                    message: format!("drop uses unresolved variable `{}`", var.name.as_u32()),
                });
            }
            check_contract(body, errors);
        }
        AetherExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            ..
        } => {
            if token.binder.is_none() {
                errors.push(AetherError {
                    kind: AetherErrorKind::UnresolvedAetherVar,
                    message: format!("reuse uses unresolved token `{}`", token.name.as_u32()),
                });
            }
            if !is_heap_tag(tag) {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidReuseTag,
                    message: format!("reuse uses non-heap constructor tag `{:?}`", tag),
                });
            }
            if let Some(token_id) = token.binder
                && fields
                    .iter()
                    .any(|field| use_counts_aether(field).contains_key(&token_id))
            {
                errors.push(AetherError {
                    kind: AetherErrorKind::ReuseTokenEscapesIntoFields,
                    message: format!(
                        "reuse token `{}` escapes into constructor fields",
                        token.name.as_u32()
                    ),
                });
            }
            if let Some(mask) = field_mask
                && !field_mask_fits(*mask, fields.len())
            {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidFieldMask,
                    message: format!(
                        "reuse field_mask=0b{:b} exceeds constructor arity {}",
                        mask,
                        fields.len()
                    ),
                });
            }
            for field in fields {
                check_contract(field, errors);
            }
        }
        AetherExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            let Some(scrutinee_id) = scrutinee.binder else {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidDropSpecializedScrutinee,
                    message: format!(
                        "drop_spec uses unresolved scrutinee `{}`",
                        scrutinee.name.as_u32()
                    ),
                });
                check_contract(unique_body, errors);
                check_contract(shared_body, errors);
                return;
            };

            let unique_count =
                invalid_drop_specialized_uses_aether(unique_body, scrutinee_id, false);
            if unique_count > 0 {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidDropSpecializedUse,
                    message: format!(
                        "drop_spec unique branch still uses scrutinee `{}` {} time(s)",
                        scrutinee.name.as_u32(),
                        unique_count
                    ),
                });
            }
            let shared_count =
                invalid_drop_specialized_uses_aether(shared_body, scrutinee_id, true);
            if shared_count > 0 {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidDropSpecializedUse,
                    message: format!(
                        "drop_spec shared branch still uses scrutinee `{}` {} time(s)",
                        scrutinee.name.as_u32(),
                        shared_count
                    ),
                });
            }
            check_contract(unique_body, errors);
            check_contract(shared_body, errors);
        }
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            check_contract(object, errors);
        }
    }
}

fn check_diagnostics(expr: &AetherExpr, diags: &mut Vec<AetherDiagnostic>) {
    match expr {
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            check_diagnostics(scrutinee, diags);
            for alt in alts {
                let destr_tag = pat_constructor_tag(&alt.pat);
                if let Some(destr_tag) = destr_tag
                    && let Some(con_tag) = find_con_in_body(&alt.rhs, Some(&destr_tag))
                    && tags_compatible(&destr_tag, &con_tag)
                    && !has_reuse_for_tag(&alt.rhs, &con_tag)
                {
                    let scrutinee_var = match scrutinee.as_ref() {
                        AetherExpr::Var { var, .. } => Some(*var),
                        _ => None,
                    };
                    let reason = scrutinee_var
                        .as_ref()
                        .and_then(|var| {
                            diagnose_drop_body(
                                var,
                                &alt.rhs,
                                pat_field_binder_ids(&alt.pat).as_deref(),
                                Some(&destr_tag),
                                None,
                            )
                        })
                        .map(|reason| reason.as_str());
                    diags.push(AetherDiagnostic {
                        kind: AetherDiagnosticKind::MissedReuse,
                        message: if let Some(reason) = reason {
                            format!(
                                "MISSED REUSE ({reason}): Case arm destructures {:?} and constructs {:?}",
                                destr_tag, con_tag
                            )
                        } else {
                            format!(
                                "MISSED REUSE: Case arm destructures {:?} and constructs {:?} — could reuse allocation",
                                destr_tag, con_tag
                            )
                        },
                    });
                }
                check_diagnostics(&alt.rhs, diags);
                if let Some(g) = &alt.guard {
                    check_diagnostics(g, diags);
                }
            }
        }
        AetherExpr::Dup { body, .. } | AetherExpr::Drop { body, .. } => {
            check_diagnostics(body, diags)
        }
        AetherExpr::Reuse { fields, .. } | AetherExpr::Con { fields, .. } => {
            for f in fields {
                check_diagnostics(f, diags);
            }
        }
        AetherExpr::Lam { body, .. } | AetherExpr::Return { value: body, .. } => {
            check_diagnostics(body, diags)
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            check_diagnostics(rhs, diags);
            check_diagnostics(body, diags);
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            for (_, rhs) in bindings {
                check_diagnostics(rhs, diags);
            }
            check_diagnostics(body, diags);
        }
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            check_diagnostics(func, diags);
            for a in args {
                check_diagnostics(a, diags);
            }
        }
        AetherExpr::PrimOp { args, .. } | AetherExpr::Perform { args, .. } => {
            for a in args {
                check_diagnostics(a, diags);
            }
        }
        AetherExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            if let Some(parameter) = parameter {
                check_diagnostics(parameter, diags);
            }
            check_diagnostics(body, diags);
            for h in handlers {
                check_diagnostics(&h.body, diags);
            }
        }
        AetherExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            check_diagnostics(unique_body, diags);
            check_diagnostics(shared_body, diags);
        }
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            check_diagnostics(object, diags);
        }
        AetherExpr::Var { .. } | AetherExpr::Lit(_, _) => {}
    }
}

fn field_mask_fits(mask: u64, arity: usize) -> bool {
    if arity >= u64::BITS as usize {
        true
    } else {
        let valid_bits = if arity == 0 { 0 } else { (1u64 << arity) - 1 };
        mask & !valid_bits == 0
    }
}

fn invalid_drop_specialized_uses_aether(
    expr: &AetherExpr,
    scrutinee_id: crate::core::CoreBinderId,
    count_reuse_token: bool,
) -> usize {
    match expr {
        AetherExpr::Var { var, .. } => usize::from(var.binder == Some(scrutinee_id)),
        AetherExpr::Lit(_, _) => 0,
        AetherExpr::Lam { body, .. } | AetherExpr::Return { value: body, .. } => {
            invalid_drop_specialized_uses_aether(body, scrutinee_id, count_reuse_token)
        }
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            invalid_drop_specialized_uses_aether(func, scrutinee_id, count_reuse_token)
                + args
                    .iter()
                    .map(|arg| {
                        invalid_drop_specialized_uses_aether(arg, scrutinee_id, count_reuse_token)
                    })
                    .sum::<usize>()
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            invalid_drop_specialized_uses_aether(rhs, scrutinee_id, count_reuse_token)
                + invalid_drop_specialized_uses_aether(body, scrutinee_id, count_reuse_token)
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            bindings
                .iter()
                .map(|(_, rhs)| {
                    invalid_drop_specialized_uses_aether(rhs, scrutinee_id, count_reuse_token)
                })
                .sum::<usize>()
                + invalid_drop_specialized_uses_aether(body, scrutinee_id, count_reuse_token)
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            invalid_drop_specialized_uses_aether(scrutinee, scrutinee_id, count_reuse_token)
                + alts
                    .iter()
                    .map(|alt| {
                        invalid_drop_specialized_uses_aether(
                            &alt.rhs,
                            scrutinee_id,
                            count_reuse_token,
                        ) + alt
                            .guard
                            .as_ref()
                            .map(|g| {
                                invalid_drop_specialized_uses_aether(
                                    g,
                                    scrutinee_id,
                                    count_reuse_token,
                                )
                            })
                            .unwrap_or(0)
                    })
                    .sum::<usize>()
        }
        AetherExpr::Con { fields, .. } | AetherExpr::PrimOp { args: fields, .. } => fields
            .iter()
            .map(|field| {
                invalid_drop_specialized_uses_aether(field, scrutinee_id, count_reuse_token)
            })
            .sum(),
        AetherExpr::Perform { args, .. } => args
            .iter()
            .map(|arg| invalid_drop_specialized_uses_aether(arg, scrutinee_id, count_reuse_token))
            .sum(),
        AetherExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            parameter.as_ref().map_or(0, |parameter| {
                invalid_drop_specialized_uses_aether(parameter, scrutinee_id, count_reuse_token)
            }) + invalid_drop_specialized_uses_aether(body, scrutinee_id, count_reuse_token)
                + handlers
                    .iter()
                    .map(|h| {
                        invalid_drop_specialized_uses_aether(
                            &h.body,
                            scrutinee_id,
                            count_reuse_token,
                        )
                    })
                    .sum::<usize>()
        }
        AetherExpr::Dup { var, body, .. } | AetherExpr::Drop { var, body, .. } => {
            usize::from(var.binder == Some(scrutinee_id))
                + invalid_drop_specialized_uses_aether(body, scrutinee_id, count_reuse_token)
        }
        AetherExpr::Reuse { token, fields, .. } => {
            let token_uses = usize::from(count_reuse_token && token.binder == Some(scrutinee_id));
            token_uses
                + fields
                    .iter()
                    .map(|field| {
                        invalid_drop_specialized_uses_aether(field, scrutinee_id, count_reuse_token)
                    })
                    .sum::<usize>()
        }
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            invalid_drop_specialized_uses_aether(object, scrutinee_id, count_reuse_token)
        }
        AetherExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            usize::from(scrutinee.binder == Some(scrutinee_id))
                + invalid_drop_specialized_uses_aether(unique_body, scrutinee_id, count_reuse_token)
                + invalid_drop_specialized_uses_aether(shared_body, scrutinee_id, count_reuse_token)
        }
    }
}

fn pat_constructor_tag(pat: &CorePat) -> Option<CoreTag> {
    match pat {
        CorePat::Con { tag, .. } => Some(tag.clone()),
        _ => None,
    }
}

fn pat_field_binder_ids(pat: &CorePat) -> Option<Vec<Option<crate::core::CoreBinderId>>> {
    match pat {
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => Some(
            fields
                .iter()
                .map(|field| match field {
                    CorePat::Var(binder) => Some(binder.id),
                    _ => None,
                })
                .collect(),
        ),
        _ => None,
    }
}

fn find_con_in_body(expr: &AetherExpr, expected_tag: Option<&CoreTag>) -> Option<CoreTag> {
    match expr {
        AetherExpr::Reuse { tag, .. } => Some(tag.clone()),
        _ if constructor_shape_for_tag_aether(expr, expected_tag).is_some() => {
            constructor_shape_for_tag_aether(expr, expected_tag).map(|(tag, _, _)| tag)
        }
        AetherExpr::Let { body, .. }
        | AetherExpr::Drop { body, .. }
        | AetherExpr::Dup { body, .. } => find_con_in_body(body, expected_tag),
        _ => None,
    }
}

fn tags_compatible(a: &CoreTag, b: &CoreTag) -> bool {
    match (a, b) {
        (CoreTag::Cons, CoreTag::Cons) => true,
        (CoreTag::Some, CoreTag::Some) => true,
        (CoreTag::Left, CoreTag::Left) => true,
        (CoreTag::Right, CoreTag::Right) => true,
        (CoreTag::Named(a), CoreTag::Named(b)) => a == b,
        _ => false,
    }
}

fn has_reuse_for_tag(expr: &AetherExpr, tag: &CoreTag) -> bool {
    match expr {
        AetherExpr::Reuse { tag: t, .. } => tags_compatible(t, tag),
        AetherExpr::Let { body, .. }
        | AetherExpr::Drop { body, .. }
        | AetherExpr::Dup { body, .. } => has_reuse_for_tag(body, tag),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::aether::{AetherAlt as CoreAlt, AetherExpr as CoreExpr};
    use crate::core::{CoreBinder, CoreBinderId, CoreLit, CorePat, CoreTag};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    use super::{
        AetherDiagnosticKind, AetherErrorKind, verify_contract_aether as verify_contract,
        verify_diagnostics_aether as verify_diagnostics,
    };
    use std::borrow::Borrow;

    fn binder(raw: u32, name: crate::syntax::Identifier) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), name)
    }

    fn s() -> Span {
        Span::default()
    }

    fn v<B: Borrow<CoreBinder>>(binder: B) -> CoreExpr {
        CoreExpr::bound_var(*binder.borrow(), s())
    }

    #[test]
    fn contract_rejects_drop_with_remaining_use() {
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let expr = CoreExpr::Drop {
            var: crate::core::CoreVarRef::resolved(&x),
            body: Box::new(v(x)),
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected unsafe drop");
        assert!(err.iter().any(|e| e.kind == AetherErrorKind::UnsafeDrop));
    }

    /// `case flag of true -> <then>, _ -> <other>`.
    fn on_flag(flag: CoreBinder, then: CoreExpr, other: CoreExpr) -> CoreExpr {
        CoreExpr::Case {
            scrutinee: Box::new(v(flag)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Lit(CoreLit::Bool(true)),
                    guard: None,
                    rhs: then,
                    span: s(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: other,
                    span: s(),
                },
            ],
            join_ty: None,
            span: s(),
        }
    }

    fn drop_in(x: CoreBinder, body: CoreExpr) -> CoreExpr {
        CoreExpr::Drop {
            var: crate::core::CoreVarRef::resolved(&x),
            body: Box::new(body),
            span: s(),
        }
    }

    #[test]
    fn contract_rejects_a_double_drop_on_one_path() {
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let flag = binder(2, interner.intern("flag"));
        let lit = |n| CoreExpr::Lit(CoreLit::Int(n), s());

        // Dropped on entry and again in one arm: twice on that path.
        let twice = drop_in(x, on_flag(flag, drop_in(x, lit(1)), lit(0)));
        let err = verify_contract(&twice).expect_err("expected a double drop");
        assert!(err.iter().any(|e| e.kind == AetherErrorKind::DoubleDrop));

        // Once in each arm: once on every path.
        let once = on_flag(flag, drop_in(x, lit(1)), drop_in(x, lit(0)));
        assert!(verify_contract(&once).is_ok());

        // Once in an arm, again after the case: twice on that arm's path.
        let after = CoreExpr::Let {
            var: binder(3, interner.intern("y")),
            rhs: Box::new(on_flag(flag, drop_in(x, lit(1)), lit(0))),
            body: Box::new(drop_in(x, lit(2))),
            span: s(),
        };
        let err = verify_contract(&after).expect_err("expected a double drop");
        assert!(err.iter().any(|e| e.kind == AetherErrorKind::DoubleDrop));
    }

    #[test]
    fn contract_rejects_a_released_borrowed_param() {
        use crate::aether::borrow_infer::{BorrowMode, BorrowProvenance, BorrowSignature};
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let y = binder(2, interner.intern("y"));
        let lit = CoreExpr::Lit(CoreLit::Int(0), s());
        let f = CoreExpr::Lam {
            params: vec![x, y],
            param_types: vec![],
            result_ty: None,
            body: Box::new(drop_in(y, drop_in(x, lit))),
            span: s(),
        };
        let signature = |modes| BorrowSignature::new(modes, BorrowProvenance::Inferred);

        // `y` owned, `x` lent: dropping `x` releases what the caller holds.
        let err = super::verify_def_contract_aether(
            &f,
            Some(&signature(vec![BorrowMode::Borrowed, BorrowMode::Owned])),
        )
        .expect_err("expected a released borrowed parameter");
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].kind, AetherErrorKind::BorrowedParamReleased);

        assert!(
            super::verify_def_contract_aether(
                &f,
                Some(&signature(vec![BorrowMode::Owned, BorrowMode::Owned]))
            )
            .is_ok()
        );
    }

    /// A call that takes its one argument owned.
    fn give(f: CoreBinder, arg: CoreExpr) -> CoreExpr {
        CoreExpr::AetherCall {
            func: Box::new(v(f)),
            args: vec![arg],
            arg_modes: vec![crate::aether::borrow_infer::BorrowMode::Owned],
            guarded_borrowed_args: vec![false],
            span: s(),
        }
    }

    fn lam(params: Vec<CoreBinder>, body: CoreExpr) -> CoreExpr {
        CoreExpr::Lam {
            params,
            param_types: vec![],
            result_ty: None,
            body: Box::new(body),
            span: s(),
        }
    }

    fn dup(x: CoreBinder, body: CoreExpr) -> CoreExpr {
        CoreExpr::Dup {
            var: crate::core::CoreVarRef::resolved(&x),
            body: Box::new(body),
            span: s(),
        }
    }

    /// `case xs of [h | t] -> <rhs>, _ -> 0`.
    fn on_cons(xs: CoreBinder, h: CoreBinder, t: CoreBinder, rhs: CoreExpr) -> CoreExpr {
        CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Con {
                        tag: CoreTag::Cons,
                        fields: vec![CorePat::Var(h), CorePat::Var(t)],
                    },
                    guard: None,
                    rhs,
                    span: s(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
                    span: s(),
                },
            ],
            join_ty: None,
            span: s(),
        }
    }

    fn views_consumed(
        def: &CoreExpr,
        signature: Option<&crate::aether::borrow_infer::BorrowSignature>,
    ) -> usize {
        super::verify_def_views_aether(def, signature)
            .iter()
            .filter(|d| d.kind == AetherDiagnosticKind::ViewConsumed)
            .count()
    }

    #[test]
    fn a_pattern_field_given_away_needs_a_copy() {
        let mut interner = Interner::new();
        let [f, xs, h, t] = ["f", "xs", "h", "t"].map(|name| interner.intern(name));
        let (f, xs, h, t) = (binder(1, f), binder(2, xs), binder(3, h), binder(4, t));
        let bare = lam(vec![f, xs], on_cons(xs, h, t, give(f, v(t))));
        assert_eq!(views_consumed(&bare, None), 1);
        let copied = lam(vec![f, xs], on_cons(xs, h, t, give(f, dup(t, v(t)))));
        assert_eq!(views_consumed(&copied, None), 0);
        // Read, not given away.
        let lent = lam(vec![f, xs], on_cons(xs, h, t, v(f)));
        assert_eq!(views_consumed(&lent, None), 0);
    }

    #[test]
    fn a_field_of_a_unique_cell_is_its_own() {
        let mut interner = Interner::new();
        let [xs, h, t] = ["xs", "h", "t"].map(|name| interner.intern(name));
        let (xs, h, t) = (binder(1, xs), binder(2, h), binder(3, t));
        let rebuild = || CoreExpr::Con {
            tag: CoreTag::Cons,
            fields: vec![v(h), v(t)],
            span: s(),
        };
        let specialized = CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::resolved(&xs),
            unique_body: Box::new(rebuild()),
            shared_body: Box::new(dup(h, dup(t, rebuild()))),
            span: s(),
        };
        let def = lam(vec![xs], on_cons(xs, h, t, specialized));
        assert_eq!(views_consumed(&def, None), 0);
    }

    #[test]
    fn a_capture_returned_from_its_lambda_needs_a_copy() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let bare = lam(vec![xs], lam(vec![], v(xs)));
        assert_eq!(views_consumed(&bare, None), 1);
        let copied = lam(vec![xs], lam(vec![], dup(xs, v(xs))));
        assert_eq!(views_consumed(&copied, None), 0);
        // The lambda's own parameter is its own.
        let own = lam(vec![xs], v(xs));
        assert_eq!(views_consumed(&own, None), 0);
    }

    #[test]
    fn a_handler_argument_returned_needs_a_copy() {
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let resume = binder(2, interner.intern("resume"));
        let (effect, operation) = (interner.intern("Ask"), interner.intern("ask"));
        let handle = |body| CoreExpr::Handle {
            body: Box::new(CoreExpr::Lit(CoreLit::Int(0), s())),
            effect,
            parameter: None,
            handlers: vec![crate::aether::AetherHandler {
                operation,
                params: vec![x],
                param_types: vec![None],
                state: None,
                state_ty: None,
                resume,
                resume_ty: None,
                body,
                span: s(),
            }],
            span: s(),
        };
        assert_eq!(views_consumed(&lam(vec![], handle(v(x))), None), 1);
        assert_eq!(views_consumed(&lam(vec![], handle(dup(x, v(x)))), None), 0);
    }

    #[test]
    fn a_borrowed_parameter_given_away_needs_a_copy() {
        use crate::aether::borrow_infer::{BorrowMode, BorrowProvenance, BorrowSignature};
        let mut interner = Interner::new();
        let f = binder(1, interner.intern("f"));
        let xs = binder(2, interner.intern("xs"));
        let def = lam(vec![f, xs], give(f, v(xs)));
        let signature = |modes| BorrowSignature::new(modes, BorrowProvenance::Inferred);
        let lent = signature(vec![BorrowMode::Borrowed, BorrowMode::Borrowed]);
        assert_eq!(views_consumed(&def, Some(&lent)), 1);
        let owned = signature(vec![BorrowMode::Borrowed, BorrowMode::Owned]);
        assert_eq!(views_consumed(&def, Some(&owned)), 0);
    }

    #[test]
    fn a_tuple_field_given_away_needs_a_copy() {
        let mut interner = Interner::new();
        let f = binder(1, interner.intern("f"));
        let pair = binder(2, interner.intern("pair"));
        let x = binder(3, interner.intern("x"));
        let field = || CoreExpr::TupleField {
            object: Box::new(v(pair)),
            index: 0,
            span: s(),
        };
        // `f(pair.0)`.
        assert_eq!(
            views_consumed(&lam(vec![f, pair], give(f, field())), None),
            1
        );
        // `let x = pair.0 in f(x)`, and the same with a copy.
        let named = |given| CoreExpr::Let {
            var: x,
            rhs: Box::new(field()),
            body: Box::new(give(f, given)),
            span: s(),
        };
        assert_eq!(views_consumed(&lam(vec![f, pair], named(v(x))), None), 1);
        assert_eq!(
            views_consumed(&lam(vec![f, pair], named(dup(x, v(x)))), None),
            0
        );
    }

    #[test]
    fn a_copy_used_in_one_arm_is_gone_after_the_match() {
        let mut interner = Interner::new();
        let [f, xs, h, t, c, h2, t2, a] =
            ["f", "xs", "h", "t", "c", "h2", "t2", "a"].map(|name| interner.intern(name));
        let (f, xs, h, t, c) = (
            binder(1, f),
            binder(2, xs),
            binder(3, h),
            binder(4, t),
            binder(5, c),
        );
        let (h2, t2, a) = (binder(6, h2), binder(7, t2), binder(8, a));
        // `dup t in let a = (case c of [_ | _] -> f(t), _ -> 0) in f(t)`.
        let body = dup(
            t,
            CoreExpr::Let {
                var: a,
                rhs: Box::new(on_cons(c, h2, t2, give(f, v(t)))),
                body: Box::new(give(f, v(t))),
                span: s(),
            },
        );
        assert_eq!(
            views_consumed(&lam(vec![f, xs, c], on_cons(xs, h, t, body)), None),
            1
        );
    }

    #[test]
    fn a_unique_cell_hands_over_one_reference_per_field_it_holds() {
        let mut interner = Interner::new();
        let [xs, h, t, a] = ["xs", "h", "t", "a"].map(|name| interner.intern(name));
        let (xs, h, t, a) = (binder(1, xs), binder(2, h), binder(3, t), binder(4, a));
        let cons = |fields| CoreExpr::Con {
            tag: CoreTag::Cons,
            fields,
            span: s(),
        };
        let specialized = |unique| CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::resolved(&xs),
            unique_body: Box::new(unique),
            shared_body: Box::new(CoreExpr::Lit(CoreLit::Int(0), s())),
            span: s(),
        };
        // A field used twice needs a copy for the second use.
        let twice = on_cons(xs, h, t, specialized(cons(vec![v(t), v(t)])));
        assert_eq!(views_consumed(&lam(vec![xs], twice), None), 1);
        // `a` belongs to the inner cell, which this path does not own.
        let nested = CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![
                        CorePat::Con {
                            tag: CoreTag::Cons,
                            fields: vec![CorePat::Var(a), CorePat::Wildcard],
                        },
                        CorePat::Var(t),
                    ],
                },
                guard: None,
                rhs: specialized(cons(vec![v(a), v(t)])),
                span: s(),
            }],
            join_ty: None,
            span: s(),
        };
        assert_eq!(views_consumed(&lam(vec![xs], nested), None), 1);
    }

    #[test]
    fn a_name_for_the_whole_matched_value_is_a_view_only_if_the_value_is() {
        use crate::aether::borrow_infer::{BorrowMode, BorrowProvenance, BorrowSignature};
        let mut interner = Interner::new();
        let [f, xs, ys] = ["f", "xs", "ys"].map(|name| interner.intern(name));
        let (f, xs, ys) = (binder(1, f), binder(2, xs), binder(3, ys));
        let def = lam(
            vec![f, xs],
            CoreExpr::Case {
                scrutinee: Box::new(v(xs)),
                alts: vec![CoreAlt {
                    pat: CorePat::Var(ys),
                    guard: None,
                    rhs: give(f, v(ys)),
                    span: s(),
                }],
                join_ty: None,
                span: s(),
            },
        );
        assert_eq!(views_consumed(&def, None), 0);
        let lent = BorrowSignature::new(
            vec![BorrowMode::Borrowed, BorrowMode::Borrowed],
            BorrowProvenance::Inferred,
        );
        assert_eq!(views_consumed(&def, Some(&lent)), 1);
    }

    #[test]
    fn an_unboxed_field_is_not_a_view() {
        let mut interner = Interner::new();
        let [f, xs, h, t] = ["f", "xs", "h", "t"].map(|name| interner.intern(name));
        let (f, xs, t) = (binder(1, f), binder(2, xs), binder(4, t));
        let h = CoreBinder::with_rep(CoreBinderId(3), h, crate::core::FluxRep::IntRep);
        let def = lam(vec![f, xs], on_cons(xs, h, t, give(f, v(h))));
        assert_eq!(views_consumed(&def, None), 0);
    }

    #[test]
    fn contract_rejects_reuse_when_token_escapes_into_fields() {
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let expr = CoreExpr::Reuse {
            token: crate::core::CoreVarRef::resolved(&x),
            tag: CoreTag::Cons,
            fields: vec![v(x), CoreExpr::Lit(CoreLit::Int(0), s())],
            field_mask: None,
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected token-escape error");
        assert!(
            err.iter()
                .any(|e| e.kind == AetherErrorKind::ReuseTokenEscapesIntoFields)
        );
    }

    #[test]
    fn contract_rejects_reuse_on_non_heap_tag() {
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let expr = CoreExpr::Reuse {
            token: crate::core::CoreVarRef::resolved(&x),
            tag: CoreTag::Nil,
            fields: vec![],
            field_mask: None,
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected invalid reuse tag");
        assert!(
            err.iter()
                .any(|e| e.kind == AetherErrorKind::InvalidReuseTag)
        );
    }

    #[test]
    fn contract_rejects_out_of_range_field_mask() {
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));
        let expr = CoreExpr::Reuse {
            token: crate::core::CoreVarRef::resolved(&x),
            tag: CoreTag::Cons,
            fields: vec![v(h), v(t)],
            field_mask: Some(0b100),
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected invalid field mask");
        assert!(
            err.iter()
                .any(|e| e.kind == AetherErrorKind::InvalidFieldMask)
        );
    }

    #[test]
    fn contract_rejects_drop_specialized_with_unresolved_scrutinee() {
        let mut interner = Interner::new();
        let name = interner.intern("xs");
        let expr = CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::unresolved(name),
            unique_body: Box::new(CoreExpr::Lit(CoreLit::Int(1), s())),
            shared_body: Box::new(CoreExpr::Lit(CoreLit::Int(2), s())),
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected invalid drop_spec scrutinee");
        assert!(
            err.iter()
                .any(|e| e.kind == AetherErrorKind::InvalidDropSpecializedScrutinee)
        );
    }

    #[test]
    fn contract_rejects_shared_branch_reusing_drop_specialized_scrutinee() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));

        let expr = CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::resolved(&xs),
            unique_body: Box::new(CoreExpr::Con {
                tag: CoreTag::Cons,
                fields: vec![v(h), v(t)],
                span: s(),
            }),
            shared_body: Box::new(CoreExpr::Reuse {
                token: crate::core::CoreVarRef::resolved(&xs),
                tag: CoreTag::Cons,
                fields: vec![v(h), v(t)],
                field_mask: None,
                span: s(),
            }),
            span: s(),
        };

        let err = verify_contract(&expr).expect_err("expected invalid shared drop_spec reuse");
        assert!(
            err.iter()
                .any(|e| e.kind == AetherErrorKind::InvalidDropSpecializedUse),
            "shared branch must not reuse the drop-specialized scrutinee"
        );
    }

    #[test]
    fn contract_rejects_aether_call_with_mismatched_mode_count() {
        let mut interner = Interner::new();
        let f = binder(1, interner.intern("f"));
        let x = binder(2, interner.intern("x"));
        let expr = CoreExpr::AetherCall {
            func: Box::new(v(f)),
            args: vec![v(x)],
            arg_modes: vec![],
            guarded_borrowed_args: vec![false],
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected invalid aether_call mode count");
        assert!(
            err.iter()
                .any(|e| e.kind == AetherErrorKind::InvalidAetherCallModes)
        );
    }

    #[test]
    fn contract_accepts_valid_current_shapes() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));
        let color = binder(4, interner.intern("color"));
        let left = binder(5, interner.intern("left"));
        let key = binder(6, interner.intern("key"));
        let right = binder(7, interner.intern("right"));
        let node = CoreTag::Named(interner.intern("Node"));

        let list_reuse = CoreExpr::Reuse {
            token: crate::core::CoreVarRef::resolved(&xs),
            tag: CoreTag::Cons,
            fields: vec![v(h), v(t)],
            field_mask: Some(0),
            span: s(),
        };
        assert!(verify_contract(&list_reuse).is_ok());

        let named_adt_reuse = CoreExpr::Reuse {
            token: crate::core::CoreVarRef::resolved(&xs),
            tag: node.clone(),
            fields: vec![v(color), v(left), v(key), v(right)],
            field_mask: Some(0b1),
            span: s(),
        };
        assert!(verify_contract(&named_adt_reuse).is_ok());

        let list_drop_spec = CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::resolved(&xs),
            unique_body: Box::new(CoreExpr::Reuse {
                token: crate::core::CoreVarRef::resolved(&xs),
                tag: CoreTag::Cons,
                fields: vec![v(h), v(t)],
                field_mask: Some(0b10),
                span: s(),
            }),
            shared_body: Box::new(CoreExpr::Con {
                tag: CoreTag::Cons,
                fields: vec![v(h), v(t)],
                span: s(),
            }),
            span: s(),
        };
        assert!(verify_contract(&list_drop_spec).is_ok());

        let named_adt_drop_spec = CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::resolved(&xs),
            unique_body: Box::new(CoreExpr::Drop {
                var: crate::core::CoreVarRef::resolved(&right),
                body: Box::new(CoreExpr::Reuse {
                    token: crate::core::CoreVarRef::resolved(&xs),
                    tag: node.clone(),
                    fields: vec![v(color), v(left), v(key), v(left)],
                    field_mask: Some(0b1000),
                    span: s(),
                }),
                span: s(),
            }),
            shared_body: Box::new(CoreExpr::Drop {
                var: crate::core::CoreVarRef::resolved(&right),
                body: Box::new(CoreExpr::Con {
                    tag: node.clone(),
                    fields: vec![v(color), v(left), v(key), v(left)],
                    span: s(),
                }),
                span: s(),
            }),
            span: s(),
        };
        assert!(verify_contract(&named_adt_drop_spec).is_ok());

        let keep = binder(8, interner.intern("keep"));
        let branchy_named = CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::resolved(&xs),
            unique_body: Box::new(CoreExpr::Case {
                scrutinee: Box::new(v(keep)),
                alts: vec![
                    CoreAlt {
                        pat: CorePat::Lit(CoreLit::Bool(true)),
                        guard: None,
                        rhs: CoreExpr::Reuse {
                            token: crate::core::CoreVarRef::resolved(&xs),
                            tag: node.clone(),
                            fields: vec![v(color), v(left), v(key), v(right)],
                            field_mask: Some(0),
                            span: s(),
                        },
                        span: s(),
                    },
                    CoreAlt {
                        pat: CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::Reuse {
                            token: crate::core::CoreVarRef::resolved(&xs),
                            tag: node,
                            fields: vec![v(color), v(left), v(key), v(left)],
                            field_mask: Some(0b1000),
                            span: s(),
                        },
                        span: s(),
                    },
                ],
                join_ty: None,
                span: s(),
            }),
            shared_body: Box::new(CoreExpr::Case {
                scrutinee: Box::new(v(keep)),
                alts: vec![
                    CoreAlt {
                        pat: CorePat::Lit(CoreLit::Bool(true)),
                        guard: None,
                        rhs: CoreExpr::Con {
                            tag: CoreTag::Named(interner.intern("Node")),
                            fields: vec![v(color), v(left), v(key), v(right)],
                            span: s(),
                        },
                        span: s(),
                    },
                    CoreAlt {
                        pat: CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::Con {
                            tag: CoreTag::Named(interner.intern("Node")),
                            fields: vec![v(color), v(left), v(key), v(left)],
                            span: s(),
                        },
                        span: s(),
                    },
                ],
                join_ty: None,
                span: s(),
            }),
            span: s(),
        };
        assert!(verify_contract(&branchy_named).is_ok());
    }

    #[test]
    fn diagnostics_report_specific_missed_reuse_reason() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let left = binder(2, interner.intern("left"));
        let key = binder(3, interner.intern("key"));
        let right = binder(4, interner.intern("right"));
        let tmp = binder(5, interner.intern("tmp"));
        let node = CoreTag::Named(interner.intern("Node"));

        let io = crate::syntax::builtin_effects::io_effect_symbol(&mut interner);
        let print = interner.intern("print");
        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: node.clone(),
                    fields: vec![
                        CorePat::Wildcard,
                        CorePat::Var(left),
                        CorePat::Var(key),
                        CorePat::Var(right),
                    ],
                },
                guard: None,
                rhs: CoreExpr::Let {
                    var: tmp,
                    rhs: Box::new(CoreExpr::Perform {
                        effect: io,
                        operation: print,
                        args: vec![v(key)],
                        span: s(),
                    }),
                    body: Box::new(CoreExpr::Con {
                        tag: node,
                        fields: vec![
                            CoreExpr::Lit(CoreLit::Int(0), s()),
                            v(left),
                            v(tmp),
                            v(right),
                        ],
                        span: s(),
                    }),
                    span: s(),
                },
                span: s(),
            }],
            join_ty: None,
            span: s(),
        };

        let diags = verify_diagnostics(&expr);
        assert!(diags.iter().any(|diag| {
            diag.kind == AetherDiagnosticKind::MissedReuse
                && diag.message.contains("EffectfulBoundary")
        }));
    }
}
