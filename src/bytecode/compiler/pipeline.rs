use crate::diagnostics::Diagnostic;
use crate::syntax::program::Program;

use super::{Compiler, MainValidationState};

/// Result of Phase 1 (collection + validation), consumed by later phases.
pub(super) struct CollectionResult {
    pub(super) main_state: MainValidationState,
}

/// Result of Phase 3 (type inference), consumed by Phases 4–6.
pub(super) struct TypeInferenceResult {
    pub(super) type_optimized_program: Option<Program>,
    pub(super) hm_diagnostics: Vec<Diagnostic>,
}

impl Compiler {
    /// Orchestrates the full compile pipeline.
    ///
    /// Phases are executed in order; inter-phase data flows through
    /// [`CollectionResult`] and [`TypeInferenceResult`] structs.
    pub(super) fn run_pipeline(&mut self, program: &Program) -> Result<(), Vec<Diagnostic>> {
        // Phase 0: Reset per-file state
        self.phase_reset();

        // Phase 1: Collect definitions + validate structure
        let collection = self.phase_collection(program);

        // Phase 2: Predeclare function names
        self.phase_predeclaration(program, &collection);

        // Phase 3: Type inference
        let ti = self.phase_type_inference(program);

        // Phase 4: IR lowering (uses the possibly-optimized program)
        let effective_program = ti.type_optimized_program.as_ref().unwrap_or(program);
        let ir_program = self.phase_ir_lowering(effective_program)?;

        // Phase 5: Code generation
        self.phase_codegen(effective_program, &ir_program);

        // Phase 6: Finalization
        self.phase_finalization(program, &collection, ti.hm_diagnostics)
    }
}
