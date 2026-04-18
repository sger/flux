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
        let class_env_ref = if self.class_env.classes.is_empty() {
            None
        } else {
            Some(&self.class_env)
        };
        let (mut ir_program, fbip_warnings) = match lower_program_to_ir_typed(
            program,
            &self.hm_expr_types,
            Some(&self.interner),
            self.type_optimize,
            Some(&self.type_env),
            class_env_ref,
        ) {
            Ok(program) => program,
            Err(diag) => {
                self.errors.push(diag);
                return Err(std::mem::take(&mut self.errors));
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

        // Register dict globals (__dict_*) injected by dictionary elaboration.
        // These CoreDefs are created during Core-to-Core passes and weren't
        // predeclared during Phase 2 (which only sees AST function names).
        for &global_name in &ir_program.globals {
            let Some(name_str) = self.interner.try_resolve(global_name) else {
                continue;
            };
            if name_str.starts_with("__dict_") && self.symbol_table.resolve(global_name).is_none() {
                self.symbol_table
                    .define(global_name, crate::diagnostics::position::Span::default());
            }
        }

        Ok(ir_program)
    }
}
