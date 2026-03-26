use crate::cfg::{
    IrPassContext, IrProgram, lower_program_to_ir_with_optimize, run_ir_pass_pipeline,
};
use crate::diagnostics::Diagnostic;
use crate::syntax::program::Program;

use super::super::Compiler;

impl Compiler {
    /// Phase 4: Lower AST to backend IR and run IR pass pipeline.
    ///
    /// Returns the lowered IR program, or drains accumulated errors on failure.
    pub(in crate::bytecode::compiler) fn phase_ir_lowering(
        &mut self,
        program: &Program,
    ) -> Result<IrProgram, Vec<Diagnostic>> {
        let (mut ir_program, fbip_warnings) = match lower_program_to_ir_with_optimize(
            program,
            &self.hm_expr_types,
            Some(&self.interner),
            self.type_optimize,
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

        Ok(ir_program)
    }
}
