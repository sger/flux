//! Proposal 0151, Phase 3: import-collision validation.
//!
//! Catches two structural collisions that the `exposing (...)` clause
//! can introduce:
//!
//! - **E457** — `exposing (foo)` brings `foo` into unqualified scope,
//!   but a local top-level `fn foo` (or another locally bound short
//!   name) already owns that name. The compiler cannot route bare `foo`
//!   to two different targets.
//!
//! - **E458** — a file-level `import A exposing (foo)` and an
//!   inside-`module` `import B exposing (foo)` both bind `foo` in
//!   overlapping scopes but to different module targets. The user must
//!   pick one source.
//!
//! The walker is intentionally conservative: only `ImportExposing::Names`
//! is checked. `ImportExposing::All` is not yet supported because the
//! exposed-name set is unbounded without scheme metadata. `ExposeNone`
//! never collides.

use std::collections::HashMap;

use crate::diagnostics::{
    DiagnosticBuilder,
    compiler_errors::{EXPOSING_LOCAL_COLLISION, IMPORT_NAME_COLLISION_FILE_VS_MODULE},
    diagnostic_for,
};
use crate::syntax::{
    Identifier,
    statement::{ImportExposing, Statement},
};

use super::super::Compiler;

impl Compiler {
    /// Phase 3 entry point — invoked by `phase_collection` after the
    /// existing class/instance walkers have populated `class_env`.
    pub(in crate::compiler) fn validate_import_collisions(
        &mut self,
        program: &crate::syntax::program::Program,
    ) {
        // Step 1: build the set of locally-declared top-level short names
        // at file scope. We treat top-level `fn` and `let` declarations as
        // locals; module-body declarations are scoped to their module.
        let mut file_locals: HashMap<Identifier, crate::diagnostics::position::Span> =
            HashMap::new();
        for stmt in &program.statements {
            match stmt {
                Statement::Function { name, span, .. } => {
                    file_locals.entry(*name).or_insert(*span);
                }
                Statement::Let { name, span, .. } => {
                    file_locals.entry(*name).or_insert(*span);
                }
                _ => {}
            }
        }

        // Step 2: walk file-level imports and fire E457 against the
        // file-level locals; also seed the file-scope name→module map
        // for the cross-scope check (E458).
        let mut file_exposed: HashMap<Identifier, Identifier> = HashMap::new();
        for stmt in &program.statements {
            let Statement::Import {
                name: module_name,
                exposing,
                span,
                ..
            } = stmt
            else {
                continue;
            };

            let ImportExposing::Names(names) = exposing else {
                continue;
            };

            for &exposed in names {
                // E457: exposing-vs-local collision.
                if file_locals.contains_key(&exposed) {
                    self.emit_e457(exposed, *span);
                    continue;
                }

                // Record the file-scope binding so the module-body
                // pass can detect cross-scope conflicts.
                if let Some(prev_module) = file_exposed.insert(exposed, *module_name)
                    && prev_module != *module_name
                {
                    self.emit_e458(exposed, *span);
                }
            }
        }

        // Step 3: recurse into `Module { ... }` bodies and run the same
        // collision check against the module-local declarations and the
        // already-collected file-scope exposed names.
        for stmt in &program.statements {
            if let Statement::Module { body, .. } = stmt {
                self.validate_module_body_imports(&body.statements, &file_exposed);
            }
        }
    }

    /// Recurse into a single module body. The module-body imports
    /// collide with module-local declarations (E457) and with any
    /// file-scope exposed name that points at a different module
    /// target (E458).
    fn validate_module_body_imports(
        &mut self,
        statements: &[Statement],
        file_exposed: &HashMap<Identifier, Identifier>,
    ) {
        let mut module_locals: HashMap<Identifier, crate::diagnostics::position::Span> =
            HashMap::new();
        for stmt in statements {
            match stmt {
                Statement::Function { name, span, .. } => {
                    module_locals.entry(*name).or_insert(*span);
                }
                Statement::Let { name, span, .. } => {
                    module_locals.entry(*name).or_insert(*span);
                }
                _ => {}
            }
        }

        for stmt in statements {
            let Statement::Import {
                name: module_name,
                exposing,
                span,
                ..
            } = stmt
            else {
                continue;
            };
            let ImportExposing::Names(names) = exposing else {
                continue;
            };

            for &exposed in names {
                if module_locals.contains_key(&exposed) {
                    self.emit_e457(exposed, *span);
                    continue;
                }
                if let Some(file_module) = file_exposed.get(&exposed)
                    && *file_module != *module_name
                {
                    self.emit_e458(exposed, *span);
                }
            }
        }

        // Recurse into nested modules (the parser doesn't currently
        // support this textually, but the walker is shape-correct).
        for stmt in statements {
            if let Statement::Module { body, .. } = stmt {
                self.validate_module_body_imports(&body.statements, file_exposed);
            }
        }
    }

    fn emit_e457(&mut self, exposed: Identifier, span: crate::diagnostics::position::Span) {
        let display_name = self.interner.resolve(exposed);
        let diag = diagnostic_for(&EXPOSING_LOCAL_COLLISION)
            .with_span(span)
            .with_message(format!(
                "`exposing ({display_name})` collides with a local declaration of \
                 the same name."
            ))
            .with_hint_text(format!(
                "Either rename the local declaration, drop the exposing entry, or \
                 continue using the qualified `Module.{display_name}` form."
            ));
        self.errors.push(diag);
    }

    fn emit_e458(&mut self, exposed: Identifier, span: crate::diagnostics::position::Span) {
        let display_name = self.interner.resolve(exposed);
        let diag = diagnostic_for(&IMPORT_NAME_COLLISION_FILE_VS_MODULE)
            .with_span(span)
            .with_message(format!(
                "`{display_name}` is exposed by two different imports in overlapping scopes."
            ))
            .with_hint_text(format!(
                "Pick one source for `{display_name}` — drop the duplicate exposing entry \
                 or restructure the imports so the short names don't overlap."
            ));
        self.errors.push(diag);
    }
}
