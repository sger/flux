use crate::cfg::{IrPassContext, IrProgram, lower_program_to_ir_typed, run_ir_pass_pipeline};
use crate::diagnostics::Diagnostic;
use crate::syntax::program::Program;

use super::super::Compiler;

impl Compiler {
    /// Phase 4: Lower AST to backend IR and run IR pass pipeline.
    ///
    /// Returns the lowered IR program, or drains accumulated errors on failure.
    pub(in crate::compiler) fn phase_ir_lowering(
        &mut self,
        program: &Program,
    ) -> Result<IrProgram, Vec<Diagnostic>> {
        self.deferred_lowering_error = None;
        let mut program = program.clone();
        self.apply_named_field_desugar(&mut program);
        self.pre_intern_specialization_names(&program);
        let class_env_ref = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(&self.class_env)
        };
        let (mut ir_program, fbip_warnings) = match lower_program_to_ir_typed(
            &program,
            &self.hm_expr_types,
            Some(&self.interner),
            self.type_optimize,
            Some(&self.type_env),
            class_env_ref,
            Some(&self.evidence_map),
        ) {
            Ok(program) => program,
            // Codegen compiles every function on the AST path without IR,
            // which reports the unit's own errors. Finalization reports this
            // one only if there are none, so the unit still never runs.
            Err(mut diag) => {
                if diag.file().is_none() {
                    diag.set_file(self.file_path.clone());
                }
                self.deferred_lowering_error = Some(diag);
                return Ok(IrProgram {
                    top_level_items: Vec::new(),
                    functions: Vec::new(),
                    entry: crate::cfg::FunctionId(0),
                    globals: Vec::new(),
                    global_bindings: Vec::new(),
                    hm_expr_types: std::collections::HashMap::new(),
                    core: None,
                });
            }
        };
        self.warnings.extend(fbip_warnings);
        if let Err(diag) = run_ir_pass_pipeline(&mut ir_program, &IrPassContext) {
            self.errors.push(diag);
            return Err(std::mem::take(&mut self.errors));
        }
        if self.analyze_enabled {
            self.tail_calls = super::super::collect_tail_calls_from_ir(&ir_program);
        }
        self.ir_function_symbols.clear();
        self.register_ir_function_symbols_from_backend(ir_program.functions());

        // Register dict globals (__dict_*) injected by dictionary elaboration,
        // and the clones lowering emitted (0.0.8 plan step 2a). Neither was
        // predeclared during Phase 2, which only sees AST function names.
        for &global_name in &ir_program.globals {
            let Some(name_str) = self.interner.try_resolve(global_name) else {
                continue;
            };
            let is_clone = self
                .specialization_clones
                .values()
                .any(|clones| clones.contains(&global_name));
            if (is_clone || name_str.starts_with("__dict_"))
                && self.symbol_table.resolve(global_name).is_none()
            {
                self.symbol_table
                    .define(global_name, crate::diagnostics::position::Span::default());
            }
        }

        Ok(ir_program)
    }
}
