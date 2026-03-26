use crate::diagnostics::DiagnosticPhase;
use crate::syntax::{program::Program, statement::Statement};

use super::super::{Compiler, pipeline::CollectionResult, tag_diagnostics};

impl Compiler {
    /// Phase 2: Forward-declare function names in symbol table.
    ///
    /// Enables forward references and mutual recursion by predeclaring all
    /// module-level function names before code generation.
    pub(in crate::bytecode::compiler) fn phase_predeclaration(
        &mut self,
        program: &Program,
        collection: &CollectionResult,
    ) {
        let main_symbol = self.interner.intern("main");

        for statement in &program.statements {
            if let Statement::Function { name, span, .. } = statement {
                let name = *name;
                // Check for duplicate declaration first (takes precedence)
                if let Some(existing) = self.symbol_table.resolve(name)
                    && self.symbol_table.exists_in_current_scope(name)
                {
                    // Keep duplicate-main diagnostics canonical via E410 from
                    // `validate_main_entrypoint`, avoid redundant E001 noise.
                    if name == main_symbol {
                        continue;
                    }
                    let name_str = self.sym(name);
                    self.errors.push(self.make_redeclaration_error(
                        name_str,
                        *span,
                        Some(existing.span),
                        None,
                    ));
                    continue;
                }
                // Check for import collision
                if self.scope_index == 0 && self.file_scope_symbols.contains(&name) {
                    let name_str = self.sym(name);
                    self.errors
                        .push(self.make_import_collision_error(name_str, *span));
                    continue;
                }
                // Predeclare the function name
                self.symbol_table.define(name, *span);
                self.file_scope_symbols.insert(name);
            }
        }

        tag_diagnostics(&mut self.errors, DiagnosticPhase::Validation);

        // Suppress unused warning — collection is passed for API consistency
        // (main_symbol is derived from the collection context).
        let _ = collection;
    }
}
