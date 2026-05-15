use std::collections::HashMap;

use super::super::Compiler;
use crate::syntax::program::Program;

impl Compiler {
    /// LSP-facing wrapper around `phase_reset`. The LSP shares one `Compiler`
    /// across prelude loading and every buffer's inference pass; this clears
    /// per-file scratch between buffers while leaving preloaded prelude state
    /// (`cached_member_schemes`, etc.) intact.
    pub fn phase_reset_for_lsp(&mut self) {
        self.phase_reset();
    }

    /// LSP-facing wrapper that runs class-declaration collection for a
    /// single program. The LSP's inference path doesn't go through the
    /// full `phase_collection`, but class-method dispatch resolution
    /// inside `infer_program` requires `class_env` to be populated
    /// (`lookup_class_method` in `ast/type_infer/mod.rs` iterates
    /// `class_env.classes`). Call this on the snapshot's program before
    /// `build_infer_config_for_program` so buffer-declared classes are
    /// visible to inference.
    ///
    /// Diagnostics produced by collection are folded into the compiler's
    /// `errors` / `warnings` — they surface through the next `take` on
    /// those lists. The LSP currently doesn't display class-collection
    /// diagnostics separately from inference diagnostics.
    pub fn collect_classes_for_lsp(&mut self, program: &Program) {
        self.collect_class_declarations(program);
    }

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
        self.transparent_type_aliases.clear();
        self.seed_builtin_effect_aliases();
        self.seed_builtin_effect_operations();
        self.static_type_scopes.clear();
        self.static_type_scopes.push(HashMap::new());
        self.effect_alias_scopes.clear();
        self.effect_alias_scopes.push(HashMap::new());
        self.routed_call_perform_ids.clear();
        #[cfg(test)]
        {
            self.hm_infer_runs = 0;
        }
    }
}
