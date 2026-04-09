use crate::syntax::program::Program;

use super::super::{Compiler, pipeline::CollectionResult};

impl Compiler {
    /// Phase 1: Collect definitions and validate program structure.
    pub(in crate::bytecode::compiler) fn phase_collection(
        &mut self,
        program: &Program,
    ) -> CollectionResult {
        self.collect_module_function_visibility(program);
        self.collect_module_adt_constructors(program);
        self.collect_module_contracts(program);
        // Re-apply Flow stdlib auto-exposure after refreshing module visibility
        // for the current compilation unit. This keeps VM/import resolution in
        // sync with the latest collected Flow members.
        self.auto_expose_flow_modules();
        self.infer_unannotated_function_effects(program);
        self.collect_adt_definitions(program);
        self.collect_effect_declarations(program);
        self.collect_class_declarations(program);
        // Proposal 0151, Phase 3: catch import-collision diagnostics
        // (E457, E458) introduced by `exposing (...)` clauses.
        self.validate_import_collisions(program);
        let main_state = self.validate_main_entrypoint(program);
        self.validate_top_level_effectful_code(program, main_state.has_main);
        self.validate_main_root_effect_discharge(program, main_state);
        self.validate_strict_mode(program, main_state.has_main);

        CollectionResult { main_state }
    }
}
