use std::borrow::Cow;

use crate::ast::expand_effect_aliases::expand_effect_aliases_in_program;
use crate::diagnostics::Diagnostic;
use crate::syntax::program::Program;
use crate::types::class_dispatch::generate_dispatch_functions;

use super::{Compiler, MainValidationState};

/// Result of Phase 1 (collection + validation), consumed by later phases.
pub(super) struct CollectionResult {
    pub(super) main_state: MainValidationState,
}

/// Result of Phase 3 (type inference), consumed by Phases 4–6.
pub(super) struct TypeInferenceResult<'a> {
    pub(super) final_program: Cow<'a, Program>,
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

        // Phase 1b: Generate dispatch functions for type class instances.
        // This injects mangled instance methods + dispatch functions into the
        // program AST so they compile through the normal pipeline.
        let class_augmented;
        let program = if !self.class_env.classes.is_empty() && !self.is_flow_library_file() {
            let additional_reserved_names = self
                .symbol_table
                .all_symbol_names()
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
            let extra = generate_dispatch_functions(
                &program.statements,
                &self.class_env,
                &mut self.interner,
                &additional_reserved_names,
            );
            if !extra.is_empty() {
                class_augmented = self.inject_generated_dispatch_functions(program, extra);
                &class_augmented
            } else {
                program
            }
        } else {
            program
        };

        // Phase 1c (Proposal 0161 B1): expand effect-row aliases in place.
        // After this pass, every EffectExpr in the AST has any
        // `alias Name = <...>` reference replaced by its decomposed body, so
        // downstream phases (predeclaration, inference, codegen) never see
        // unexpanded aliases.
        let alias_expanded;
        let program: &Program = if !self.effect_row_aliases.is_empty() {
            alias_expanded = {
                let mut owned: Program = program.clone();
                expand_effect_aliases_in_program(&mut owned, &self.effect_row_aliases);
                owned
            };
            &alias_expanded
        } else {
            program
        };

        // Phase 2: Predeclare function names
        self.phase_predeclaration(program, &collection);

        // Phase 3: Type inference
        let ti = self.phase_type_inference(program);

        // Phase 4: IR lowering (uses the possibly-optimized program)
        let effective_program = ti.final_program.as_ref();
        let ir_program = self.phase_ir_lowering(effective_program)?;

        // Phase 5: Code generation
        self.phase_codegen(effective_program, &ir_program);

        // Phase 6: Finalization
        self.phase_finalization(program, &collection, ti.hm_diagnostics)
    }
}
