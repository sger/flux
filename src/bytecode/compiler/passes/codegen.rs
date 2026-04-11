use crate::bytecode::op_code::OpCode;
use crate::cfg::IrProgram;
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

        // Emit dict tuple construction bytecode after all functions are compiled.
        // Dict values must be initialized at module load time before user code
        // can call constrained functions.
        self.emit_dict_globals(ir_program);
    }

    /// Emit bytecode to construct and store dictionary globals.
    ///
    /// For each `__dict_{Class}_{Type}` CoreDef (a `MakeTuple` of method
    /// references), emit bytecode that loads the method functions onto the
    /// stack, creates a tuple, and stores it in the global symbol table.
    fn emit_dict_globals(&mut self, ir_program: &IrProgram) {
        let core = match ir_program.core.as_ref() {
            Some(c) => c,
            None => return,
        };

        // Collect dict construction info first to avoid borrow conflicts.
        let mut dict_entries: Vec<(crate::syntax::Identifier, Vec<crate::syntax::Identifier>)> =
            Vec::new();

        for def in &core.defs {
            let name_str = match self.interner.try_resolve(def.name) {
                Some(s) => s,
                None => continue,
            };
            if !name_str.starts_with("__dict_") {
                continue;
            }

            // Extract method references from MakeTuple(...).
            if let crate::core::CoreExpr::PrimOp {
                op: crate::core::CorePrimOp::MakeTuple,
                args,
                ..
            } = &def.expr
            {
                let mut method_names = Vec::new();
                let mut ok = true;
                for arg in args {
                    if let crate::core::CoreExpr::Var { var, .. } = arg {
                        method_names.push(var.name);
                    } else {
                        ok = false;
                        break;
                    }
                }
                if ok && !method_names.is_empty() {
                    dict_entries.push((def.name, method_names));
                }
            }
        }

        // Now emit bytecode for each dict.
        for (dict_name, method_names) in &dict_entries {
            let dict_binding = match self.symbol_table.resolve(*dict_name) {
                Some(b) => b,
                None => continue,
            };

            let mut all_resolved = true;
            for &method_name in method_names {
                if let Some(binding) = self.symbol_table.resolve(method_name) {
                    self.load_symbol(&binding);
                } else {
                    all_resolved = false;
                    break;
                }
            }

            if !all_resolved {
                continue;
            }

            let count = method_names.len();
            if count <= 255 {
                self.emit(OpCode::OpTuple, &[count, 0]);
            } else {
                self.emit(OpCode::OpTupleLong, &[count]);
            }
            self.emit(OpCode::OpSetGlobal, &[dict_binding.index]);
        }
    }
}
