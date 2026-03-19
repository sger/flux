use crate::ast::type_infer::infer_program;
use crate::ast::type_informed_fold::type_informed_fold;
use crate::diagnostics::DiagnosticPhase;
use crate::syntax::program::Program;

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
                type_optimized_program = Some(optimized);

                let mut diags = hm2.diagnostics;
                tag_diagnostics(&mut diags, DiagnosticPhase::TypeInference);
                diags
            } else {
                self.type_env = hm.type_env;
                self.hm_expr_types = hm.expr_types;
                type_optimized_program = None;

                let mut diags = hm.diagnostics;
                tag_diagnostics(&mut diags, DiagnosticPhase::TypeInference);
                diags
            }
        };

        TypeInferenceResult {
            type_optimized_program,
            hm_diagnostics,
        }
    }
}
