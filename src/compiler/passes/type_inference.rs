use crate::ast::desugar_named_fields::{
    NamedFieldDesugarCtx, collect_named_field_metadata, collect_named_field_metadata_in_statements,
    desugar_named_fields_in_program,
};
use crate::ast::type_infer::constraint::WantedConstraints;
use crate::ast::type_infer::static_type_validation::{
    StaticTypeValidationCtx, validate_static_types,
};
use crate::diagnostics::DiagnosticPhase;
use crate::syntax::program::Program;
use crate::types::class_disposition::SolveScope;
use crate::types::class_solver::solve_wanted_tree;

use super::super::{Compiler, pipeline::TypeInferenceResult, tag_diagnostics};

impl Compiler {
    /// Phase 3: HM type inference (single or two-phase with type_optimize).
    ///
    /// Two-phase model (when type_optimize=true, proposal 0077):
    ///   Phase 1: infer on the syntactically-optimized AST → TypeEnv for optimization
    ///   type_informed_fold: rewrite AST using TypeEnv (dead branch, const prop, inlining)
    ///   Phase 2: infer on the type-optimized AST → pointer-stable maps for codegen
    ///
    /// Single-phase model (when type_optimize=false):
    ///   Standard single inference pass.
    ///
    /// Invariant: codegen must use the same Program allocation as the final
    /// inference pass so pointer-keyed expression IDs remain stable.
    pub(in crate::compiler) fn phase_type_inference<'a>(
        &mut self,
        program: &'a Program,
    ) -> TypeInferenceResult<'a> {
        let final_inference = self.infer_final_program(program);
        let mut final_program = final_inference.effective_program;
        let hm_final = final_inference.hm_final;
        self.type_env = hm_final.type_env;
        self.hm_expr_types = hm_final.expr_types;
        let module_member_schemes = hm_final.module_member_schemes;
        self.cached_member_schemes
            .extend(module_member_schemes.clone());
        let class_constraints: WantedConstraints = hm_final.class_constraints;
        let instantiated_expr_vars = hm_final.instantiated_expr_vars;
        let resolved_binding_schemes = hm_final.resolved_binding_schemes;

        // REPL mode (proposal 0176): remember this line's top-level binding
        // schemes so a later line's inference can resolve the types of earlier
        // session globals. A failed line is rolled back wholesale by the engine
        // (a pre-line clone of the compiler), which discards these too.
        if self.repl_mode {
            self.repl_session_schemes.extend(
                resolved_binding_schemes
                    .iter()
                    .map(|(k, v)| (*k, v.clone())),
            );
        }

        let mut hm_diagnostics = hm_final.diagnostics;
        tag_diagnostics(&mut hm_diagnostics, DiagnosticPhase::TypeInference);

        // Authoritative static-typing gate: reject any binding whose
        // resolved scheme still contains unresolved fallback type variables.
        let mut strict_diags = validate_static_types(
            final_program.as_ref(),
            &StaticTypeValidationCtx {
                resolved_schemes: &resolved_binding_schemes,
                resolved_binding_schemes_by_span: &hm_final.resolved_binding_schemes_by_span,
                expr_types: &self.hm_expr_types,
                module_member_schemes: &module_member_schemes,
                fallback_vars: &hm_final.fallback_vars,
                instantiated_expr_vars: &instantiated_expr_vars,
                existing_diagnostics: &hm_diagnostics,
                interner: &self.interner,
            },
        );
        tag_diagnostics(&mut strict_diags, DiagnosticPhase::TypeInference);
        hm_diagnostics.extend(strict_diags);

        // Type class constraint solving: verify that concrete-type constraints
        // have matching instances in the ClassEnv (Proposal 0145, Step 4).
        if !class_constraints.is_solved() && !self.class_env.classes.is_empty() {
            // Whole-program scope: generalization has already had its chance,
            // so nothing here is generalizable (Proposal 0179 Stage 3). Each
            // definition's scope is solved with the context its signature
            // promises, which is why the tree is passed rather than a list.
            let outcome = solve_wanted_tree(
                &class_constraints,
                SolveScope::WholeProgram,
                &self.class_env,
                &self.interner,
            );
            outcome.trace_stuck(&self.interner);
            self.evidence_map = harvest_evidence(&outcome);
            let mut solver_diags: Vec<_> = outcome.into_diagnostics().collect();
            tag_diagnostics(&mut solver_diags, DiagnosticPhase::TypeInference);
            hm_diagnostics.extend(solver_diags);
        }

        self.has_hm_diagnostics = hm_diagnostics
            .iter()
            .any(|d| d.severity() == crate::diagnostics::Severity::Error);

        // Proposal 0152, Phase 3: desugar named-field AST nodes into their
        // positional equivalents so every downstream phase (AST-fallback
        // bytecode, Core lowering, LLVM) sees only classic AST forms.
        {
            let (mut ctor_field_names, mut adt_variants) =
                collect_named_field_metadata(final_program.as_ref());
            // Proposal 0176: fold in named-field metadata from earlier REPL
            // lines' `data` declarations so a later line's `Point { x: .. }` /
            // `{ ...p, .. }` / `p.x` desugars to its positional form. Empty
            // outside the REPL.
            if !self.repl_session_adt_data.is_empty() {
                let (preloaded_field_names, preloaded_variants) =
                    collect_named_field_metadata_in_statements(&self.repl_session_adt_data);
                ctor_field_names.extend(preloaded_field_names);
                adt_variants.extend(preloaded_variants);
            }
            // fold in field order for constructors declared in
            // *imported* modules, which have no `data` statement in this
            // program. Inserted first-wins order: a locally declared
            // constructor of the same name keeps its own field order, matching
            // the shadowing behaviour everywhere else.
            for (ctor, fields) in &self.preloaded_ctor_field_names {
                ctor_field_names
                    .entry(*ctor)
                    .or_insert_with(|| fields.clone());
            }
            let mut ctx = NamedFieldDesugarCtx {
                ctor_field_names: &ctor_field_names,
                adt_variants: &adt_variants,
                hm_expr_types: &self.hm_expr_types,
            };
            let owned = final_program.to_mut();
            desugar_named_fields_in_program(owned, &mut ctx);
        }

        TypeInferenceResult {
            final_program,
            hm_diagnostics,
        }
    }
}

/// Collect the solver's evidence, keyed by the site that raised each predicate.
///
/// Only `Solved` predicates carry evidence: a `Generalized` one becomes a
/// dictionary *parameter* of the enclosing definition rather than an argument
/// at this site, and a `Stuck` or `Diagnosed` one has no instance to name.
///
/// The index within a site is the emission order of that site's predicates,
/// which is also the order its dictionaries are passed — so it doubles as the
/// argument position.
fn harvest_evidence(
    outcome: &crate::types::class_disposition::SolveOutcome,
) -> crate::types::evidence::EvidenceMap {
    use crate::types::class_disposition::Disposition;
    use crate::types::evidence::{EvidenceMap, EvidenceSite};

    let mut map = EvidenceMap::new();
    let mut next_index: std::collections::HashMap<crate::syntax::expression::ExprId, u16> =
        std::collections::HashMap::new();
    for entry in &outcome.dispositions {
        let Some(expr) = entry.wanted.expr else {
            continue;
        };
        // Count every predicate the expression raised, not just the solved
        // ones. `EvidenceSite.index` is the argument position, so skipping an
        // unsolved predicate would slide every later dictionary one slot left.
        // Leaving the gap is also what lets `EvidenceMap::args_for` refuse to
        // build a partial argument list: a dense short list would be a call
        // with the wrong arity that still type-checks.
        let index = next_index.entry(expr).or_insert(0);
        let position = *index;
        *index += 1;
        let Disposition::Solved { evidence } = &entry.disposition else {
            continue;
        };
        map.insert(EvidenceSite::new(expr, position), evidence.clone());
    }
    map
}

#[cfg(test)]
mod tests {
    use super::harvest_evidence;
    use crate::ast::type_infer::constraint::{WantedClassConstraint, WantedClassConstraintOrigin};
    use crate::diagnostics::position::Span;
    use crate::syntax::expression::ExprId;
    use crate::syntax::symbol::Symbol;
    use crate::types::class_disposition::{
        Disposition, DispositionedConstraint, Evidence, SolveOutcome, StuckReason,
    };
    use crate::types::class_id::ClassId;
    use crate::types::evidence::EvidenceSite;

    /// Symbols are only ever compared here, never resolved, so a raw index is
    /// safe — see `Symbol::new`.
    fn wanted(expr: ExprId) -> WantedClassConstraint {
        let name = Symbol::new(0);
        WantedClassConstraint {
            class_name: name,
            class_id: ClassId::from_local_name(name),
            type_args: vec![],
            span: Span::default(),
            expr: Some(expr),
            origin: WantedClassConstraintOrigin::MethodCall,
        }
    }

    fn outcome(dispositions: Vec<Disposition>, expr: ExprId) -> SolveOutcome {
        SolveOutcome {
            dispositions: dispositions
                .into_iter()
                .map(|disposition| DispositionedConstraint {
                    wanted: wanted(expr),
                    disposition,
                })
                .collect(),
        }
    }

    fn solved() -> Disposition {
        Disposition::Solved {
            evidence: Evidence::Structural { components: vec![] },
        }
    }

    fn stuck() -> Disposition {
        Disposition::Stuck {
            reason: StuckReason::OuterScopeVariable,
        }
    }

    #[test]
    fn an_unsolved_predicate_does_not_shift_the_ones_after_it() {
        // `index` is the argument position. Counting only solved predicates
        // would put this evidence at slot 0, which is the slot the *stuck*
        // predicate's dictionary occupies.
        let expr = ExprId::UNSET;
        let map = harvest_evidence(&outcome(vec![stuck(), solved()], expr));

        assert!(map.get(&EvidenceSite::new(expr, 0)).is_none());
        assert!(map.get(&EvidenceSite::new(expr, 1)).is_some());
    }

    #[test]
    fn a_site_with_a_hole_refuses_to_build_an_argument_list() {
        // `args_for` returns None rather than a short list, so a caller cannot
        // emit a call with the wrong arity. Compacting the indices would defeat
        // that guard by making the list dense.
        let expr = ExprId::UNSET;
        let map = harvest_evidence(&outcome(vec![stuck(), solved()], expr));

        assert!(map.args_for(expr).is_none());
    }

    #[test]
    fn a_fully_solved_site_still_yields_every_argument_in_order() {
        let expr = ExprId::UNSET;
        let map = harvest_evidence(&outcome(vec![solved(), solved()], expr));

        assert_eq!(map.args_for(expr).map(|args| args.len()), Some(2));
    }
}
