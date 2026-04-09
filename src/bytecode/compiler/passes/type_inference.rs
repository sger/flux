use crate::ast::type_infer::constraint::WantedClassConstraint;
use crate::ast::type_infer::strict_types::validate_strict_types;
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
    pub(in crate::bytecode::compiler) fn phase_type_inference<'a>(
        &mut self,
        program: &'a Program,
    ) -> TypeInferenceResult<'a> {
        let final_inference = self.infer_final_program(program);
        let final_program = final_inference.final_program;
        let hm_final = final_inference.hm_final;
        self.type_env = hm_final.type_env;
        self.hm_expr_types = hm_final.expr_types;
        self.cached_member_schemes
            .extend(hm_final.module_member_schemes);
        let class_constraints: Vec<WantedClassConstraint> = hm_final.class_constraints;

        let mut hm_diagnostics = hm_final.diagnostics;
        tag_diagnostics(&mut hm_diagnostics, DiagnosticPhase::TypeInference);

        // Strict-types validation: reject any binding whose inferred type
        // still contains `Any`. Runs after inference on the final program.
        if self.strict_types {
            let mut strict_diags =
                validate_strict_types(final_program.as_ref(), &self.type_env, &self.interner);
            tag_diagnostics(&mut strict_diags, DiagnosticPhase::TypeInference);
            hm_diagnostics.extend(strict_diags);
        }

        // Type class constraint solving: verify that concrete-type constraints
        // have matching instances in the ClassEnv (Proposal 0145, Step 4).
        if !class_constraints.is_empty() && !self.class_env.classes.is_empty() {
            let mut solver_diags =
                solve_class_constraints(&class_constraints, &self.class_env, &self.interner);
            tag_diagnostics(&mut solver_diags, DiagnosticPhase::TypeInference);
            hm_diagnostics.extend(solver_diags);
        }

        self.has_hm_diagnostics = !hm_diagnostics.is_empty();

        TypeInferenceResult {
            final_program,
            hm_diagnostics,
        }
    }
}
