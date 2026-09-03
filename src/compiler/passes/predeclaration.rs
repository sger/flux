use crate::diagnostics::DiagnosticPhase;
use crate::syntax::{program::Program, statement::Statement};

use super::super::{Compiler, pipeline::CollectionResult, tag_diagnostics};

impl Compiler {
    /// Declare the `__dict_*` global of every instance in scope.
    ///
    /// A unit that passes evidence to a constrained function across a module
    /// boundary names this global, but only the *defining* module's Core
    /// carries the `__dict_*` def that `ir_lowering` declares from, and a
    /// cached dependency contributes no Core at all. Declaring here covers
    /// every path into the compiler — the driver, the cached-interface
    /// preload, and the in-process harnesses that compile a module graph with
    /// no preloading — because by Phase 2 the class environment already holds
    /// the imported instances. `emit_imported_dict_globals` stores them at
    /// load time (KI-061).
    fn predeclare_instance_dictionary_globals(&mut self) {
        // Only classes some visible signature actually constrains can have
        // their evidence demanded here. Declaring the rest would burn a global
        // slot per known instance in every program — and shift the index of
        // every `let` bound after this point.
        let demanded: std::collections::HashSet<_> = self
            .type_env
            .visible_bindings()
            .map(|(_, scheme)| scheme)
            .chain(self.cached_member_schemes.values())
            .flat_map(|scheme| scheme.constraints.iter())
            .filter(|constraint| self.class_env.constraint_needs_dictionary(constraint))
            .map(|constraint| constraint.class_id)
            .collect();
        if demanded.is_empty() {
            return;
        }
        let names = self
            .class_env
            .instances
            .iter()
            .filter(|instance| demanded.contains(&instance.class_id))
            .map(|instance| {
                let type_key = instance
                    .type_args
                    .iter()
                    .map(|arg| arg.display_with(&self.interner))
                    .collect::<Vec<_>>()
                    .join("_");
                crate::types::class_env::dictionary_name(
                    instance.class_id,
                    &type_key,
                    &self.interner,
                )
            })
            .collect::<Vec<_>>();
        for name in names {
            let symbol = self.interner.intern(&name);
            if !self.symbol_table.exists_in_current_scope(symbol) {
                self.symbol_table
                    .define(symbol, crate::diagnostics::position::Span::default());
            }
        }
    }

    /// Phase 2: Forward-declare function names in symbol table.
    ///
    /// Enables forward references and mutual recursion by predeclaring all
    /// module-level function names before code generation.
    pub(in crate::compiler) fn phase_predeclaration(
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
                    // A dispatch stub generated while compiling an earlier
                    // module is not a user declaration, and a real function
                    // takes precedence over it — that is already the
                    // resolution rule, since a name bound to a function never
                    // dispatches as a class method. Let the declaration stand
                    // instead of reporting a redeclaration of a name the user
                    // never wrote (`fn add` against `Flow.Num`'s `add` stub).
                    if !self.generated_dispatch_stub_names.remove(&name) {
                        let name_str = self.sym(name);
                        self.errors.push(self.make_redeclaration_error(
                            name_str,
                            *span,
                            Some(existing.span),
                            None,
                        ));
                        continue;
                    }
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

        // After the program's own names, so declaring evidence never shifts a
        // user function's global index.
        self.predeclare_instance_dictionary_globals();

        tag_diagnostics(&mut self.errors, DiagnosticPhase::Validation);

        // Suppress unused warning — collection is passed for API consistency
        // (main_symbol is derived from the collection context).
        let _ = collection;
    }
}
