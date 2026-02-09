use crate::frontend::{
    diagnostics::{
        DUPLICATE_NAME, Diagnostic, DiagnosticBuilder, IMMUTABLE_BINDING, IMPORT_NAME_COLLISION,
        OUTER_ASSIGNMENT, PRIVATE_MEMBER, UNDEFINED_VARIABLE,
    },
    position::Span,
};

use super::{CompileResult, Compiler, suggestions::find_similar_names};

impl Compiler {
    pub(super) fn make_immutability_error(&self, name: &str, span: Span) -> Diagnostic {
        Diagnostic::make_error(&IMMUTABLE_BINDING, &[name], self.file_path.clone(), span)
    }

    pub(super) fn make_redeclaration_error(
        &self,
        name: &str,
        span: Span,
        existing_span: Option<Span>,
        hint_text: Option<&str>,
    ) -> Diagnostic {
        let mut diagnostic =
            Diagnostic::make_error(&DUPLICATE_NAME, &[name], self.file_path.clone(), span);
        if let Some(text) = hint_text {
            diagnostic = diagnostic.with_hint_text(text);
        }
        if let Some(existing_span) = existing_span {
            diagnostic = diagnostic.with_hint_labeled("", existing_span, "first defined here");
        }
        diagnostic
    }

    pub(super) fn make_undefined_variable_error(&self, name: &str, span: Span) -> Diagnostic {
        let mut diagnostic =
            Diagnostic::make_error(&UNDEFINED_VARIABLE, &[name], self.file_path.clone(), span);

        // Get all available symbol names from the symbol table
        let available_names: Vec<String> = self
            .symbol_table
            .all_symbol_names()
            .into_iter()
            .map(|s| self.interner.resolve(s).to_string())
            .collect();

        // Find similar names using fuzzy matching
        let suggestions = find_similar_names(name, &available_names, 3);

        // Add suggestions as hints
        if !suggestions.is_empty() {
            let suggestion_text = if suggestions.len() == 1 {
                format!("Did you mean `{}`?", suggestions[0])
            } else {
                let names = suggestions
                    .iter()
                    .map(|s| format!("`{}`", s))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Did you mean one of: {}?", names)
            };
            diagnostic = diagnostic.with_help(suggestion_text);
        }

        diagnostic
    }

    pub(super) fn make_import_collision_error(&self, name: &str, span: Span) -> Diagnostic {
        Diagnostic::make_error(
            &IMPORT_NAME_COLLISION,
            &[name],
            self.file_path.clone(),
            span,
        )
    }

    pub(super) fn make_outer_assignment_error(&self, name: &str, span: Span) -> Diagnostic {
        Diagnostic::make_error(&OUTER_ASSIGNMENT, &[name], self.file_path.clone(), span)
    }

    pub(super) fn check_private_member(
        &self,
        member: &str,
        expr_span: Span,
        module_name: Option<&str>,
    ) -> CompileResult<()> {
        if !member.starts_with('_') {
            return Ok(());
        }

        let same_module = module_name.is_some_and(|name| {
            self.current_module_prefix
                .map(|prefix| self.interner.resolve(prefix) == name)
                .unwrap_or(false)
        });
        if same_module {
            return Ok(());
        }

        Err(Self::boxed(Diagnostic::make_error(
            &PRIVATE_MEMBER,
            &[member],
            self.file_path.clone(),
            expr_span,
        )))
    }
}
