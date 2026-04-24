use std::collections::HashMap;

use super::super::Compiler;

impl Compiler {
    /// Phase 0: Clear per-file tracking state for each compile pass.
    pub(in crate::compiler) fn phase_reset(&mut self) {
        self.warnings.clear();
        self.file_scope_symbols.clear();
        self.imported_modules.clear();
        self.import_aliases.clear();
        self.imported_module_exclusions.clear();
        self.current_module_prefix = None;
        self.function_effects.clear();
        self.handled_effects.clear();
        self.effect_ops_registry = self.preloaded_effect_ops_registry.clone();
        self.effect_op_signatures = self.preloaded_effect_op_signatures.clone();
        self.effect_row_aliases.clear();
        self.seed_builtin_effect_aliases();
        self.seed_builtin_effect_operations();
        self.static_type_scopes.clear();
        self.static_type_scopes.push(HashMap::new());
        self.effect_alias_scopes.clear();
        self.effect_alias_scopes.push(HashMap::new());
        #[cfg(test)]
        {
            self.hm_infer_runs = 0;
        }
    }
}
