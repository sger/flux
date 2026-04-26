use crate::ast::desugar_named_fields::{
    NamedFieldDesugarCtx, collect_named_field_metadata, desugar_named_fields_in_program,
};
use crate::ast::type_infer::constraint::WantedClassConstraint;
use crate::ast::type_infer::static_type_validation::{
    StaticTypeValidationCtx, validate_static_types,
};
use crate::diagnostics::DiagnosticPhase;
use crate::syntax::program::Program;
use crate::types::class_solver::solve_class_constraints;

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
        let class_constraints: Vec<WantedClassConstraint> = hm_final.class_constraints;
        let instantiated_expr_vars = hm_final.instantiated_expr_vars;
        let resolved_binding_schemes = hm_final.resolved_binding_schemes;

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
        if !class_constraints.is_empty() && !self.class_env.classes.is_empty() {
            let mut solver_diags =
                solve_class_constraints(&class_constraints, &self.class_env, &self.interner);
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
            let (ctor_field_names, adt_variants) =
                collect_named_field_metadata(final_program.as_ref());
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
