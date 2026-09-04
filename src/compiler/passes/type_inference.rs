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
