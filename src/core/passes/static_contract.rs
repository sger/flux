//! Core-adjacent static typing contract validation (Proposal 0167 Part 7).
//!
//! This pass runs over a fully-lowered `CoreProgram` just before normalization
//! / Aether. It does **not** re-run HM inference. Instead, it inspects the
//! HM-resolved result types that `AST → Core` lowering already attached to
//! each [`CoreDef`] and verifies that those types do not carry illegal
//! unresolved residue — free type variables that are not accounted for by
//! a `Forall` quantifier enclosing them.
//!
//! The rationale is that by the time Core has been built, the static typing
//! contract should be a property of the maintained semantic pipeline, not
//! just a pre-Core AST check. An AST-level bug that silently leaks a free
//! var all the way through lowering now fails here with a clear message
//! instead of surfacing downstream as a representation surprise.
//!
//! The pass intentionally looks only at `CoreDef::result_ty`. Parameter
//! types and inner expressions are already covered by the AST-level
//! `static_type_validation` pass; duplicating that walk on Core would
//! over-report. This is a contract spot-check, not a re-validation.

use std::collections::HashSet;

use crate::{
    ast::type_infer::boundary::BoundaryKind,
    core::{CoreDef, CoreProgram, CoreType},
    diagnostics::{Diagnostic, DiagnosticBuilder, diagnostic_for, position::Span, types::Severity},
    syntax::interner::Interner,
    types::TypeVarId,
};

/// Result of the Core-adjacent static contract check. An empty vector means
/// every definition's result type is concrete (or only carries legitimately
/// quantified variables).
#[derive(Debug, Default)]
pub struct CoreContractReport {
    pub violations: Vec<Diagnostic>,
}

impl CoreContractReport {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Walk every definition's `result_ty` and collect diagnostics for any free
/// type variable that is not bound by an enclosing `Forall` quantifier.
///
/// `interner` is accepted (and currently unused) so a future refinement can
/// render the binder name alongside the offending variable without changing
/// callers.
pub fn validate_core_static_contract(
    program: &CoreProgram,
    _interner: &Interner,
) -> CoreContractReport {
    let mut report = CoreContractReport::default();
    for def in &program.defs {
        let Some(ty) = &def.result_ty else {
            // Missing result_ty means Core lowering could not attach an HM
            // type — unrelated to the residue contract. Earlier passes (AST
            // strict validation, quick_validate_function_body) are responsible
            // for surfacing that condition.
            continue;
        };
        let mut bound = HashSet::new();
        if let Some(span_diag) = find_unbound_var(ty, &mut bound, def, def.span) {
            report.violations.push(span_diag);
        }
    }
    report
}

/// Return the first illegal unresolved variable found in `ty`, rendered as
/// a diagnostic. `bound` accumulates variables introduced by `Forall`
/// quantifiers on the traversal path.
fn find_unbound_var(
    ty: &CoreType,
    bound: &mut HashSet<TypeVarId>,
    def: &CoreDef,
    span: Span,
) -> Option<Diagnostic> {
    match ty {
        CoreType::Int
        | CoreType::Float
        | CoreType::Bool
        | CoreType::String
        | CoreType::Unit
        | CoreType::Never => None,
        CoreType::Var(var) => {
            if bound.contains(var) {
                None
            } else {
                Some(build_violation(def, *var, span))
            }
        }
        CoreType::List(inner) | CoreType::Array(inner) | CoreType::Option(inner) => {
            find_unbound_var(inner, bound, def, span)
        }
        CoreType::Either(l, r) | CoreType::Map(l, r) => {
            find_unbound_var(l, bound, def, span).or_else(|| find_unbound_var(r, bound, def, span))
        }
        CoreType::Tuple(items) | CoreType::Adt(_, items) => items
            .iter()
            .find_map(|t| find_unbound_var(t, bound, def, span)),
        CoreType::Function(params, ret) => params
            .iter()
            .find_map(|t| find_unbound_var(t, bound, def, span))
            .or_else(|| find_unbound_var(ret, bound, def, span)),
        CoreType::Forall(vars, inner) => {
            // Bring quantified vars into scope for the inner traversal.
            let added: Vec<TypeVarId> = vars.iter().copied().filter(|v| bound.insert(*v)).collect();
            let result = find_unbound_var(inner, bound, def, span);
            for v in added {
                bound.remove(&v);
            }
            result
        }
        // Abstract constructor heads carry no inner vars we need to walk.
        CoreType::Abstract(_) => None,
    }
}

fn build_violation(def: &CoreDef, var: TypeVarId, span: Span) -> Diagnostic {
    // Reuse the existing "strict unresolved" error code so downstream users
    // (docs, error-code guides) see consistent surfacing. The message keeps
    // the internal Core detail secondary to the source-level problem.
    //
    // Tagged with [`BoundaryKind::BackendConcreteBoundary`] (Proposal 0167
    // Part 1): by the time Core lowering has run, any residue in a def's
    // result type is a representation-level failure — backends need every
    // Core-visible type concrete to pick a layout. The label surfaces in
    // the user-facing message so "why is this reported here?" reads
    // consistently with the AST-level boundary diagnostics.
    //
    // Severity is `Error`: by this point HM inference has finalized and
    // AST strict validation has already run. Residue surfacing here is a
    // real contract violation the AST layer missed, not observability
    // noise. The explicit `Severity::Error` assignment is retained
    // (rather than relying on the default from `diagnostic_for`) so the
    // intent is documented at the emission site and a future rollout
    // step can flip it in one place without hunting for the severity.
    let kind = BoundaryKind::BackendConcreteBoundary;
    let mut diag = diagnostic_for(&crate::diagnostics::compiler_errors::STRICT_TYPES_ANY_INFERRED)
        .with_span(span)
        .with_message(format!(
            "Flux could not determine a concrete result type for this definition \
             before code generation. Internal detail: definition `{}` still has \
             unresolved type variable #{} at the {boundary}.",
            def.name.as_u32(),
            var,
            boundary = kind.label(),
        ))
        .with_hint_text("Add a type annotation or make the expression's result type unambiguous.");
    diag.severity = Severity::Error;
    diag
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{CoreBinder, CoreBinderId, CoreExpr, CoreLit},
        syntax::Identifier,
        types::TypeVarId,
    };

    fn make_def(name: Identifier, ty: Option<CoreType>) -> CoreDef {
        CoreDef {
            name,
            binder: CoreBinder::new(CoreBinderId(0), name),
            expr: CoreExpr::Lit(CoreLit::Unit, Default::default()),
            borrow_signature: None,
            result_ty: ty,
            is_anonymous: false,
            is_recursive: false,
            fip: None,
            span: Span::default(),
        }
    }

    fn make_program(defs: Vec<CoreDef>) -> CoreProgram {
        CoreProgram {
            defs,
            top_level_items: Vec::new(),
        }
    }

    #[test]
    fn concrete_result_type_passes() {
        let interner = Interner::new();
        let name = Identifier::new(1);
        let prog = make_program(vec![make_def(name, Some(CoreType::Int))]);
        let report = validate_core_static_contract(&prog, &interner);
        assert!(report.is_clean());
    }

    #[test]
    fn forall_quantified_var_passes() {
        let interner = Interner::new();
        let name = Identifier::new(1);
        let v: TypeVarId = 42;
        let ty = CoreType::Forall(
            vec![v],
            Box::new(CoreType::Function(
                vec![CoreType::Var(v)],
                Box::new(CoreType::Var(v)),
            )),
        );
        let prog = make_program(vec![make_def(name, Some(ty))]);
        let report = validate_core_static_contract(&prog, &interner);
        assert!(report.is_clean(), "polymorphic identity must pass");
    }

    #[test]
    fn unbound_var_fails() {
        let interner = Interner::new();
        let name = Identifier::new(1);
        let v: TypeVarId = 7;
        // No enclosing Forall — var is illegal residue.
        let ty = CoreType::Function(vec![CoreType::Int], Box::new(CoreType::Var(v)));
        let prog = make_program(vec![make_def(name, Some(ty))]);
        let report = validate_core_static_contract(&prog, &interner);
        assert_eq!(report.violations.len(), 1);
    }

    #[test]
    fn missing_result_ty_is_skipped() {
        // result_ty = None means an earlier pass already should have flagged
        // the missing HM type. The Core contract pass is only responsible for
        // residue, not for the absence itself.
        let interner = Interner::new();
        let name = Identifier::new(1);
        let prog = make_program(vec![make_def(name, None)]);
        let report = validate_core_static_contract(&prog, &interner);
        assert!(report.is_clean());
    }
}
