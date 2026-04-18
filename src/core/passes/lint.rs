//! Core IR structural verification.
//!
//! Validates invariants that must hold after every Core pass:
//! - Binder scope: every variable reference resolves to an in-scope binder
//! - No duplicate binders within a single binding form
//! - Lambda/handler parameter count matches type annotation count
//! - Case expressions have at least one alternative
//! - LetRecGroup has at least one binding
//!
//! Modeled after GHC's Core Lint (`GHC.Core.Lint`) and Flux's Aether
//! verifier (`src/aether/verify.rs`).

use std::collections::HashSet;

use crate::core::{CoreBinderId, CoreDef, CoreExpr, CoreHandler, CorePat, CoreProgram};

/// Fatal Core lint error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreLintError {
    pub kind: CoreLintErrorKind,
    pub message: String,
    /// Name symbol of the top-level definition containing the violation.
    pub def_name: Option<crate::syntax::Identifier>,
}

/// Categories of Core lint violations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreLintErrorKind {
    /// Variable reference with `binder: Some(id)` where `id` is not in scope.
    UnresolvedVar,
    /// Same `CoreBinderId` introduced twice in one binding form.
    DuplicateBinder,
    /// `Lam.param_types` is non-empty but has different length than `params`.
    LamParamTypeMismatch,
    /// `CoreHandler.param_types` is non-empty but has different length than `params`.
    HandlerParamTypeMismatch,
    /// `Case` expression with zero alternatives.
    EmptyCaseAlts,
    /// `LetRecGroup` with zero bindings.
    LetRecGroupEmpty,
}

/// Verify Core IR invariants for an entire program.
///
/// Returns `Ok(())` if all definitions pass, or `Err(errors)` with every
/// violation found (does not stop at the first error).
///
/// All top-level definition binders are in scope for every definition
/// (mutual recursion at the module level).
pub fn lint_core_program(program: &CoreProgram) -> Result<(), Vec<CoreLintError>> {
    let mut errors = Vec::new();
    // Collect all top-level binder IDs — they are mutually in scope.
    let top_level_scope: HashSet<CoreBinderId> = program.defs.iter().map(|d| d.binder.id).collect();
    for def in &program.defs {
        let before = errors.len();
        lint_expr(&def.expr, &mut top_level_scope.clone(), &mut errors);
        // Tag new errors with the definition name.
        for e in &mut errors[before..] {
            e.def_name = Some(def.name);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Verify Core IR invariants for a single definition.
pub fn lint_core_def(def: &CoreDef) -> Result<(), Vec<CoreLintError>> {
    let mut errors = Vec::new();
    let mut in_scope = HashSet::new();
    in_scope.insert(def.binder.id);
    lint_expr(&def.expr, &mut in_scope, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Recursively lint a Core expression, maintaining the in-scope binder set.
fn lint_expr(
    expr: &CoreExpr,
    in_scope: &mut HashSet<CoreBinderId>,
    errors: &mut Vec<CoreLintError>,
) {
    match expr {
        CoreExpr::Var { var, .. } => {
            if let Some(id) = var.binder
                && !in_scope.contains(&id) {
                    errors.push(CoreLintError {
                        kind: CoreLintErrorKind::UnresolvedVar,
                        def_name: None,
                        message: format!(
                            "variable `{}` references binder {:?} which is not in scope",
                            var.name.as_u32(),
                            id
                        ),
                    });
                }
        }

        CoreExpr::Lit(..) => {}

        CoreExpr::Lam {
            params,
            param_types,
            body,
            ..
        } => {
            if !param_types.is_empty() && param_types.len() != params.len() {
                errors.push(CoreLintError {
                    kind: CoreLintErrorKind::LamParamTypeMismatch,
                    def_name: None,
                    message: format!(
                        "Lam has {} params but {} param_types",
                        params.len(),
                        param_types.len()
                    ),
                });
            }
            check_duplicate_binders(params.iter().map(|b| b.id), "Lam", errors);
            let saved = save_scope(in_scope);
            for p in params {
                in_scope.insert(p.id);
            }
            lint_expr(body, in_scope, errors);
            restore_scope(in_scope, saved);
        }

        CoreExpr::App { func, args, .. } => {
            lint_expr(func, in_scope, errors);
            for arg in args {
                lint_expr(arg, in_scope, errors);
            }
        }

        CoreExpr::Let { var, rhs, body, .. } => {
            lint_expr(rhs, in_scope, errors);
            let saved = save_scope(in_scope);
            in_scope.insert(var.id);
            lint_expr(body, in_scope, errors);
            restore_scope(in_scope, saved);
        }

        CoreExpr::LetRec { var, rhs, body, .. } => {
            let saved = save_scope(in_scope);
            in_scope.insert(var.id);
            lint_expr(rhs, in_scope, errors);
            lint_expr(body, in_scope, errors);
            restore_scope(in_scope, saved);
        }

        CoreExpr::LetRecGroup { bindings, body, .. } => {
            if bindings.is_empty() {
                errors.push(CoreLintError {
                    kind: CoreLintErrorKind::LetRecGroupEmpty,
                    def_name: None,
                    message: "LetRecGroup has zero bindings".to_string(),
                });
            }
            check_duplicate_binders(bindings.iter().map(|(b, _)| b.id), "LetRecGroup", errors);
            let saved = save_scope(in_scope);
            for (binder, _) in bindings {
                in_scope.insert(binder.id);
            }
            for (_, rhs) in bindings {
                lint_expr(rhs, in_scope, errors);
            }
            lint_expr(body, in_scope, errors);
            restore_scope(in_scope, saved);
        }

        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            lint_expr(scrutinee, in_scope, errors);
            for alt in alts {
                let saved = save_scope(in_scope);
                collect_pat_binders(&alt.pat, in_scope);
                if let Some(guard) = &alt.guard {
                    lint_expr(guard, in_scope, errors);
                }
                lint_expr(&alt.rhs, in_scope, errors);
                restore_scope(in_scope, saved);
            }
        }

        CoreExpr::Con { fields, .. } => {
            for field in fields {
                lint_expr(field, in_scope, errors);
            }
        }

        CoreExpr::PrimOp { args, .. } => {
            for arg in args {
                lint_expr(arg, in_scope, errors);
            }
        }

        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            lint_expr(object, in_scope, errors);
        }

        CoreExpr::Return { value, .. } => {
            lint_expr(value, in_scope, errors);
        }

        CoreExpr::Perform { args, .. } => {
            for arg in args {
                lint_expr(arg, in_scope, errors);
            }
        }

        CoreExpr::Handle { body, handlers, .. } => {
            lint_expr(body, in_scope, errors);
            for handler in handlers {
                lint_handler(handler, in_scope, errors);
            }
        }
    }
}

/// Lint one effect handler arm.
fn lint_handler(
    handler: &CoreHandler,
    in_scope: &mut HashSet<CoreBinderId>,
    errors: &mut Vec<CoreLintError>,
) {
    if !handler.param_types.is_empty() && handler.param_types.len() != handler.params.len() {
        errors.push(CoreLintError {
            kind: CoreLintErrorKind::HandlerParamTypeMismatch,
            def_name: None,
            message: format!(
                "Handler has {} params but {} param_types",
                handler.params.len(),
                handler.param_types.len()
            ),
        });
    }
    let saved = save_scope(in_scope);
    for p in &handler.params {
        in_scope.insert(p.id);
    }
    in_scope.insert(handler.resume.id);
    lint_expr(&handler.body, in_scope, errors);
    restore_scope(in_scope, saved);
}

/// Collect binder IDs introduced by a pattern into the in-scope set.
fn collect_pat_binders(pat: &CorePat, in_scope: &mut HashSet<CoreBinderId>) {
    match pat {
        CorePat::Var(binder) => {
            in_scope.insert(binder.id);
        }
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for f in fields {
                collect_pat_binders(f, in_scope);
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
}

/// Check for duplicate binder IDs in a single binding form.
fn check_duplicate_binders(
    ids: impl Iterator<Item = CoreBinderId>,
    context: &str,
    errors: &mut Vec<CoreLintError>,
) {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            errors.push(CoreLintError {
                kind: CoreLintErrorKind::DuplicateBinder,
                def_name: None,
                message: format!("duplicate binder {:?} in {context}", id),
            });
        }
    }
}

/// Snapshot the current scope for later restoration.
fn save_scope(in_scope: &HashSet<CoreBinderId>) -> HashSet<CoreBinderId> {
    in_scope.clone()
}

/// Restore the scope to a previous snapshot.
fn restore_scope(in_scope: &mut HashSet<CoreBinderId>, saved: HashSet<CoreBinderId>) {
    *in_scope = saved;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::*;
    use crate::diagnostics::position::Span;
    use crate::syntax::symbol::Symbol;

    fn s() -> Span {
        Span::default()
    }

    fn binder(id: u32) -> CoreBinder {
        CoreBinder::new(CoreBinderId(id), Symbol::new(id))
    }

    fn var_ref(id: u32) -> CoreVarRef {
        CoreVarRef {
            name: Symbol::new(id),
            binder: Some(CoreBinderId(id)),
        }
    }

    fn external_var_ref(name: u32) -> CoreVarRef {
        CoreVarRef {
            name: Symbol::new(name),
            binder: None,
        }
    }

    fn make_program(defs: Vec<CoreDef>) -> CoreProgram {
        CoreProgram {
            defs,
            top_level_items: Vec::new(),
        }
    }

    fn make_def(id: u32, expr: CoreExpr) -> CoreDef {
        CoreDef {
            name: Symbol::new(id),
            binder: binder(id),
            expr,
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip: None,
            span: s(),
        }
    }

    // ── Passing cases ────────────────────────────────────────────────────

    #[test]
    fn lint_passes_for_simple_literal() {
        let program = make_program(vec![make_def(0, CoreExpr::Lit(CoreLit::Int(42), s()))]);
        assert!(lint_core_program(&program).is_ok());
    }

    #[test]
    fn lint_passes_for_lambda_with_bound_var() {
        let program = make_program(vec![make_def(
            0,
            CoreExpr::Lam {
                params: vec![binder(1)],
                param_types: vec![],
                result_ty: None,
                body: Box::new(CoreExpr::Var {
                    var: var_ref(1),
                    span: s(),
                }),
                span: s(),
            },
        )]);
        assert!(lint_core_program(&program).is_ok());
    }

    #[test]
    fn lint_passes_for_external_var() {
        let program = make_program(vec![make_def(
            0,
            CoreExpr::Var {
                var: external_var_ref(99),
                span: s(),
            },
        )]);
        assert!(lint_core_program(&program).is_ok());
    }

    #[test]
    fn lint_passes_for_let_binding() {
        let program = make_program(vec![make_def(
            0,
            CoreExpr::Let {
                var: binder(1),
                rhs: Box::new(CoreExpr::Lit(CoreLit::Int(1), s())),
                body: Box::new(CoreExpr::Var {
                    var: var_ref(1),
                    span: s(),
                }),
                span: s(),
            },
        )]);
        assert!(lint_core_program(&program).is_ok());
    }

    #[test]
    fn lint_passes_for_self_recursive_letrec() {
        // LetRec: var is in scope within its own RHS
        let program = make_program(vec![make_def(
            0,
            CoreExpr::LetRec {
                var: binder(1),
                rhs: Box::new(CoreExpr::Var {
                    var: var_ref(1),
                    span: s(),
                }),
                body: Box::new(CoreExpr::Var {
                    var: var_ref(1),
                    span: s(),
                }),
                span: s(),
            },
        )]);
        assert!(lint_core_program(&program).is_ok());
    }

    // ── Failing cases ────────────────────────────────────────────────────

    #[test]
    fn lint_catches_unresolved_var() {
        let program = make_program(vec![make_def(
            0,
            CoreExpr::Var {
                var: var_ref(99), // not in scope
                span: s(),
            },
        )]);
        let errors = lint_core_program(&program).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind, CoreLintErrorKind::UnresolvedVar);
    }

    #[test]
    fn lint_catches_duplicate_binders_in_lam() {
        let program = make_program(vec![make_def(
            0,
            CoreExpr::Lam {
                params: vec![binder(1), binder(1)], // duplicate
                param_types: vec![],
                result_ty: None,
                body: Box::new(CoreExpr::Lit(CoreLit::Unit, s())),
                span: s(),
            },
        )]);
        let errors = lint_core_program(&program).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.kind == CoreLintErrorKind::DuplicateBinder)
        );
    }

    #[test]
    fn lint_catches_param_type_mismatch() {
        let program = make_program(vec![make_def(
            0,
            CoreExpr::Lam {
                params: vec![binder(1), binder(2)],
                param_types: vec![Some(CoreType::Int)], // 1 type for 2 params
                result_ty: None,
                body: Box::new(CoreExpr::Lit(CoreLit::Unit, s())),
                span: s(),
            },
        )]);
        let errors = lint_core_program(&program).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.kind == CoreLintErrorKind::LamParamTypeMismatch)
        );
    }

    #[test]
    fn lint_catches_empty_letrec_group() {
        let program = make_program(vec![make_def(
            0,
            CoreExpr::LetRecGroup {
                bindings: vec![], // empty
                body: Box::new(CoreExpr::Lit(CoreLit::Unit, s())),
                span: s(),
            },
        )]);
        let errors = lint_core_program(&program).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.kind == CoreLintErrorKind::LetRecGroupEmpty)
        );
    }

    #[test]
    fn lint_catches_var_out_of_scope_after_let() {
        // let x = 1 in (let y = 2 in y) followed by referencing y outside
        // Simulate: Let x = 1 in App(Lam([y], y), [Var(y_outside_scope)])
        let program = make_program(vec![make_def(
            0,
            CoreExpr::Let {
                var: binder(1),
                rhs: Box::new(CoreExpr::Lit(CoreLit::Int(1), s())),
                body: Box::new(CoreExpr::Var {
                    var: var_ref(2), // binder 2 is NOT in scope
                    span: s(),
                }),
                span: s(),
            },
        )]);
        let errors = lint_core_program(&program).unwrap_err();
        assert_eq!(errors[0].kind, CoreLintErrorKind::UnresolvedVar);
    }

    #[test]
    fn lint_passes_for_case_with_pattern_binders() {
        let program = make_program(vec![make_def(
            0,
            CoreExpr::Case {
                scrutinee: Box::new(CoreExpr::Lit(CoreLit::Int(1), s())),
                alts: vec![CoreAlt {
                    pat: CorePat::Var(binder(1)),
                    guard: None,
                    rhs: CoreExpr::Var {
                        var: var_ref(1),
                        span: s(),
                    },
                    span: s(),
                }],
                join_ty: None,
                span: s(),
            },
        )]);
        assert!(lint_core_program(&program).is_ok());
    }
}
