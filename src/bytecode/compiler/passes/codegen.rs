use crate::backend_ir::IrProgram;
use crate::diagnostics::DiagnosticPhase;
use crate::syntax::{
    pattern_validate::validate_program_patterns, program::Program, statement::Statement,
};

use super::super::{Compiler, tag_diagnostics};

impl Compiler {
    /// Phase 5: Pattern validation and compile all statements to bytecode.
    pub(in crate::bytecode::compiler) fn phase_codegen(
        &mut self,
        program: &Program,
        ir_program: &IrProgram,
    ) {
        let mut pattern_diags = validate_program_patterns(program, &self.file_path, &self.interner);
        tag_diagnostics(&mut pattern_diags, DiagnosticPhase::Validation);
        self.errors.extend(pattern_diags);

        for statement in &program.statements {
            // Continue compilation even if there are errors
            let compile_result = match statement {
                Statement::Function {
                    name,
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    body,
                    span,
                    ..
                } => {
                    let effective_effects: Vec<crate::syntax::effect_expr::EffectExpr> =
                        if effects.is_empty() {
                            self.lookup_unqualified_contract(*name, parameters.len())
                                .map(|contract| contract.effects.clone())
                                .unwrap_or_default()
                        } else {
                            effects.clone()
                        };
                    let ir_function = self.find_ir_function_by_symbol(ir_program, *name);
                    let result = self.compile_function_statement(
                        *name,
                        parameters,
                        parameter_types,
                        return_type,
                        &effective_effects,
                        body,
                        ir_function,
                        *span,
                    );
                    if self.scope_index == 0 {
                        self.file_scope_symbols.insert(*name);
                    }
                    result
                }
                Statement::Module { name, body, span } => {
                    self.compile_module_statement(*name, body, span.start, Some(ir_program))
                }
                _ => self.compile_statement(statement),
            };
            if let Err(err) = compile_result {
                let mut diag = *err;
                if diag.phase().is_none() {
                    diag.phase = Some(DiagnosticPhase::TypeCheck);
                }
                self.errors.push(diag);
            }
        }
    }
}
