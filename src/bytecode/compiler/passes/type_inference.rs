use crate::ast::type_infer::constraint::WantedClassConstraint;
use crate::ast::type_infer::infer_program;
use crate::ast::type_infer::strict_types::validate_strict_types;
use crate::ast::type_informed_fold::type_informed_fold;
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
    pub(in crate::bytecode::compiler) fn phase_type_inference(
        &mut self,
        program: &Program,
    ) -> TypeInferenceResult {
        let type_optimized_program;
        let class_constraints: Vec<WantedClassConstraint>;
        let hm_diagnostics = {
            let hm_config = self.build_infer_config(program);
            let hm = infer_program(program, &self.interner, hm_config);

            if self.type_optimize {
                // Phase 1 complete: use TypeEnv for type-informed fold.
                let optimized = type_informed_fold(program, &hm.type_env, &self.interner);

                // Phase 2: re-infer on the optimized AST for stable expr-id maps.
                let hm_config2 = self.build_infer_config(&optimized);
                let hm2 = infer_program(&optimized, &self.interner, hm_config2);
                self.type_env = hm2.type_env;
                self.hm_expr_types = hm2.expr_types;
                // Cache HM-inferred module member schemes for downstream modules.
                self.cached_member_schemes.extend(hm2.module_member_schemes);
                class_constraints = hm2.class_constraints;
                type_optimized_program = Some(optimized);

                let mut diags = hm2.diagnostics;
                tag_diagnostics(&mut diags, DiagnosticPhase::TypeInference);
                diags
            } else {
                self.type_env = hm.type_env;
                self.hm_expr_types = hm.expr_types;
                // Cache HM-inferred module member schemes for downstream modules.
                self.cached_member_schemes.extend(hm.module_member_schemes);
                class_constraints = hm.class_constraints;
                type_optimized_program = None;

                let mut diags = hm.diagnostics;
                tag_diagnostics(&mut diags, DiagnosticPhase::TypeInference);
                diags
            }
        };

        // Strict-types validation: reject any binding whose inferred type
        // still contains `Any`. Runs after inference on the final program.
        let mut hm_diagnostics = hm_diagnostics;
        if self.strict_types {
            let final_program = type_optimized_program.as_ref().unwrap_or(program);
            let mut strict_diags =
                validate_strict_types(final_program, &self.type_env, &self.interner);
            tag_diagnostics(&mut strict_diags, DiagnosticPhase::TypeInference);
            hm_diagnostics.extend(strict_diags);
        }

        // Type class constraint solving: verify that concrete-type constraints
        // have matching instances in the ClassEnv (Proposal 0145, Step 4).
        // Only enforced when strict_types is active (Flow stdlib is excluded).
        if self.strict_types && !class_constraints.is_empty() && !self.class_env.classes.is_empty()
        {
            let mut solver_diags =
                solve_class_constraints(&class_constraints, &self.class_env, &self.interner);
            tag_diagnostics(&mut solver_diags, DiagnosticPhase::TypeInference);
            hm_diagnostics.extend(solver_diags);
        }

        self.has_hm_diagnostics = !hm_diagnostics.is_empty();

        TypeInferenceResult {
            type_optimized_program,
            hm_diagnostics,
        }
    }
}
