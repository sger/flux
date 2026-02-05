use super::builders::DiagnosticBuilder;
use super::types::*;
use super::{ErrorCode, ErrorType, format_message};
use super::rendering;
use crate::frontend::position::{Position, Span};
use std::borrow::Cow;
use std::collections::HashMap;
use std::{env, fs};

/// The core diagnostic struct representing an error, warning, or note
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    severity: Severity,
    title: String,
    code: Option<String>,
    error_type: Option<ErrorType>,
    message: Option<String>,
    file: Option<String>,
    span: Option<Span>,
    labels: Vec<Label>,
    hints: Vec<Hint>,
    suggestions: Vec<InlineSuggestion>,
    hint_chains: Vec<HintChain>,
    related: Vec<RelatedDiagnostic>,
}

// ICE = Internal Compiler Error (a compiler bug, not user code).
#[macro_export]
macro_rules! ice {
    ($msg:expr) => {{
        #[allow(deprecated)]
        let diag = $crate::frontend::diagnostics::Diagnostic::error("INTERNAL COMPILER ERROR")
            .with_message($msg)
            .with_error_type($crate::frontend::diagnostics::ErrorType::Compiler)
            .with_hint_text(format!("{}:{} ({})", file!(), line!(), module_path!()));
        diag
    }};
}

impl Diagnostic {
    /// Create a new error diagnostic with the given title.
    ///
    /// # Deprecation Notice
    /// This method is deprecated for production code. Use error constructor functions instead:
    /// - Parser errors: `compiler_errors::unknown_keyword()`, `compiler_errors::unexpected_token()`, etc.
    /// - Runtime errors: `runtime_errors::invalid_operation()`, etc.
    ///
    /// This ensures consistent error codes and messages from a single source of truth.
    /// Only use this method in tests or internal diagnostics system code.
    #[deprecated(
        since = "0.2.0",
        note = "Use error constructor functions from compiler_errors or runtime_errors instead"
    )]
    pub fn error(title: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            title: title.into(),
            code: None,
            error_type: None,
            message: None,
            file: None,
            span: None,
            labels: Vec::new(),
            hints: Vec::new(),
            suggestions: Vec::new(),
            hint_chains: Vec::new(),
            related: Vec::new(),
        }
    }

    /// Create a new warning diagnostic with the given title.
    pub fn warning(title: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            title: title.into(),
            code: None,
            error_type: None,
            message: None,
            file: None,
            span: None,
            labels: Vec::new(),
            hints: Vec::new(),
            suggestions: Vec::new(),
            hint_chains: Vec::new(),
            related: Vec::new(),
        }
    }

    /// Get the starting position from the span (derived field)
    pub fn position(&self) -> Option<Position> {
        self.span.map(|s| s.start)
    }

    // Getters for read access
    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub fn error_type(&self) -> Option<ErrorType> {
        self.error_type
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    pub fn span(&self) -> Option<Span> {
        self.span
    }

    pub fn hints(&self) -> &[Hint] {
        &self.hints
    }

    pub fn labels(&self) -> &[Label] {
        &self.labels
    }

    pub fn suggestions(&self) -> &[InlineSuggestion] {
        &self.suggestions
    }

    pub fn hint_chains(&self) -> &[HintChain] {
        &self.hint_chains
    }

    pub fn related(&self) -> &[RelatedDiagnostic] {
        &self.related
    }

    // Setter for file (needed by module_graph)
    pub fn set_file(&mut self, file: impl Into<String>) {
        self.file = Some(file.into());
    }
}

// ===== Builder Pattern Implementation =====
// All builder methods (with_*) are implemented via the DiagnosticBuilder trait
impl DiagnosticBuilder for Diagnostic {
    fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    fn with_error_type(mut self, error_type: ErrorType) -> Self {
        self.error_type = Some(error_type);
        self
    }

    fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    fn with_position(mut self, position: Position) -> Self {
        self.span = Some(Span::new(position, position));
        self
    }

    fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Add a hint to the diagnostic
    fn with_hint(mut self, hint: Hint) -> Self {
        self.hints.push(hint);
        self
    }

    /// Add a text-only hint (convenience method for backward compatibility)
    fn with_hint_text(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        let cleaned = text
            .strip_prefix("Hint:\n")
            .or_else(|| text.strip_prefix("Hint:"))
            .unwrap_or(text.as_str())
            .trim_start();
        self.hints.push(Hint::text(cleaned));
        self
    }

    /// Add a hint with a source location (convenience method)
    fn with_hint_at(mut self, text: impl Into<String>, span: Span) -> Self {
        self.hints.push(Hint::at(text, span));
        self
    }

    /// Add a hint with a source location and label (convenience method)
    fn with_hint_labeled(
        mut self,
        text: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) -> Self {
        self.hints.push(Hint::labeled(text, span, label));
        self
    }

    /// Add a note hint (additional context or information)
    fn with_note(mut self, text: impl Into<String>) -> Self {
        self.hints.push(Hint::note(text));
        self
    }

    /// Add a help hint (explicit instructions on how to fix)
    fn with_help(mut self, text: impl Into<String>) -> Self {
        self.hints.push(Hint::help(text));
        self
    }

    /// Add an example hint (code example demonstrating the solution)
    fn with_example(mut self, text: impl Into<String>) -> Self {
        self.hints.push(Hint::example(text));
        self
    }

    /// Add a primary label to the diagnostic (main error location)
    fn with_primary_label(mut self, span: Span, text: impl Into<String>) -> Self {
        self.labels.push(Label::primary(span, text));
        self
    }

    /// Add a secondary label to the diagnostic (additional context)
    fn with_secondary_label(mut self, span: Span, text: impl Into<String>) -> Self {
        self.labels.push(Label::secondary(span, text));
        self
    }

    /// Add a note label to the diagnostic (informational)
    fn with_note_label(mut self, span: Span, text: impl Into<String>) -> Self {
        self.labels.push(Label::note(span, text));
        self
    }

    /// Add a label with explicit style
    fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    /// Add an inline code suggestion
    fn with_suggestion(mut self, suggestion: InlineSuggestion) -> Self {
        self.suggestions.push(suggestion);
        self
    }

    /// Add an inline suggestion with replacement text (convenience method)
    fn with_suggestion_replace(mut self, span: Span, replacement: impl Into<String>) -> Self {
        self.suggestions
            .push(InlineSuggestion::new(span, replacement));
        self
    }

    /// Add an inline suggestion with message (convenience method)
    fn with_suggestion_message(
        mut self,
        span: Span,
        replacement: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        self.suggestions
            .push(InlineSuggestion::new(span, replacement).with_message(message));
        self
    }

    /// Add a hint chain for step-by-step guidance
    fn with_hint_chain(mut self, chain: HintChain) -> Self {
        self.hint_chains.push(chain);
        self
    }

    /// Add a related diagnostic entry (note/help/related)
    fn with_related(mut self, related: RelatedDiagnostic) -> Self {
        self.related.push(related);
        self
    }

    /// Add a hint chain from a list of steps (convenience method)
    fn with_steps<S: Into<String>>(mut self, steps: impl IntoIterator<Item = S>) -> Self {
        self.hint_chains.push(HintChain::from_steps(steps));
        self
    }

    /// Add a hint chain with steps and conclusion (convenience method)
    fn with_steps_and_conclusion<S: Into<String>>(
        mut self,
        steps: impl IntoIterator<Item = S>,
        conclusion: impl Into<String>,
    ) -> Self {
        self.hint_chains
            .push(HintChain::from_steps(steps).with_conclusion(conclusion));
        self
    }
}

// ===== Factory Methods and Rendering =====
impl Diagnostic {
    /// Generic error builder using ErrorCode specification
    #[allow(deprecated)]
    pub fn make_error(
        err_spec: &'static ErrorCode,
        values: &[&str],
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        let message = format_message(err_spec.message, values);
        let hint = err_spec.hint.map(|h| format_message(h, values));

        let mut diag = Diagnostic::error(err_spec.title)
            .with_code(err_spec.code)
            .with_error_type(err_spec.error_type)
            .with_file(file)
            .with_span(span)
            .with_message(message);

        if let Some(hint_text) = hint {
            diag = diag.with_hint_text(hint_text);
        }

        diag
    }

    /// Generic warning builder using ErrorCode specification
    /// Similar to make_error but creates warnings for non-fatal issues
    pub fn make_warning_from_code(
        warn_spec: &'static ErrorCode,
        values: &[&str],
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        let message = format_message(warn_spec.message, values);
        let hint = warn_spec.hint.map(|h| format_message(h, values));

        let mut diag = Diagnostic::warning(warn_spec.title)
            .with_code(warn_spec.code)
            .with_error_type(warn_spec.error_type)
            .with_file(file)
            .with_span(span)
            .with_message(message);

        if let Some(hint_text) = hint {
            diag = diag.with_hint_text(hint_text);
        }

        diag
    }

    /// Dynamic error builder for runtime-generated error information
    /// Use this when error details come from runtime values rather than static ErrorCode
    #[allow(deprecated)]
    pub fn make_error_dynamic(
        code: impl Into<String>,
        title: impl Into<String>,
        error_type: ErrorType,
        message: impl Into<String>,
        hint: Option<String>,
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        let mut diag = Diagnostic::error(title)
            .with_code(code)
            .with_error_type(error_type)
            .with_file(file)
            .with_span(span)
            .with_message(message);

        if let Some(hint_text) = hint {
            diag = diag.with_hint_text(hint_text);
        }

        diag
    }

    /// Warning builder for linter and non-fatal issues
    pub fn make_warning(
        code: impl Into<String>,
        title: impl Into<String>,
        message: impl Into<String>,
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        Diagnostic::warning(title)
            .with_code(code)
            .with_file(file)
            .with_span(span)
            .with_message(message)
    }

    /// Note builder for informational diagnostics.
    pub fn make_note(
        title: impl Into<String>,
        message: impl Into<String>,
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        Self {
            severity: Severity::Note,
            title: title.into(),
            code: None,
            error_type: None,
            message: Some(message.into()),
            file: Some(file.into()),
            span: Some(span),
            labels: Vec::new(),
            hints: Vec::new(),
            suggestions: Vec::new(),
            hint_chains: Vec::new(),
            related: Vec::new(),
        }
    }

    /// Help builder for suggestions and assistance.
    pub fn make_help(
        title: impl Into<String>,
        message: impl Into<String>,
        file: impl Into<String>,
        span: Span,
    ) -> Self {
        Self {
            severity: Severity::Help,
            title: title.into(),
            code: None,
            error_type: None,
            message: Some(message.into()),
            file: Some(file.into()),
            span: Some(span),
            labels: Vec::new(),
            hints: Vec::new(),
            suggestions: Vec::new(),
            hint_chains: Vec::new(),
            related: Vec::new(),
        }
    }







    pub fn render(&self, source: Option<&str>, default_file: Option<&str>) -> String {
        self.render_with_context(source, default_file, None)
    }

    pub fn render_with_sources(
        &self,
        default_file: Option<&str>,
        sources_by_file: Option<&HashMap<String, String>>,
    ) -> String {
        let primary_source = sources_by_file.and_then(|sources| {
            self.file
                .as_deref()
                .or(default_file)
                .and_then(|file| sources.get(file).map(|s| s.as_str()))
        });
        self.render_with_context(primary_source, default_file, sources_by_file)
    }

    fn render_with_context(
        &self,
        source: Option<&str>,
        default_file: Option<&str>,
        sources_by_file: Option<&HashMap<String, String>>,
    ) -> String {
        let mut fallback_source: Option<String> = None;
        let source = match source {
            Some(source) => Some(source),
            None => {
                let file = self
                    .file
                    .as_deref()
                    .filter(|f| !f.is_empty())
                    .or(default_file);
                if let Some(file) = file {
                    fallback_source = fs::read_to_string(file).ok();
                }
                fallback_source.as_deref()
            }
        };
        let mut out = String::new();
        let use_color = env::var_os("NO_COLOR").is_none();
        let file = self
            .file
            .as_deref()
            .filter(|f| !f.is_empty())
            .or(default_file)
            .map(rendering::render_display_path)
            .unwrap_or_else(|| Cow::Borrowed("<unknown>"));
        let code = self.code.as_deref().unwrap_or("E000");

        // Render header
        rendering::render_header(
            &mut out,
            self.severity,
            self.error_type,
            &self.title,
            code,
            use_color,
        );

        // Render message
        rendering::render_message(&mut out, self.message.as_deref());

        // Render location
        rendering::render_location(&mut out, source, file.as_ref(), self.span, self.message.as_deref());

        // Render source snippet with primary span and labels
        if let Some(span) = self.span {
            rendering::render_source_snippet(&mut out, source, span, &self.labels, use_color);
        }

        // Render suggestions
        rendering::render_suggestions(&mut out, source, &self.suggestions, use_color);

        // Render hints
        rendering::render_hints(
            &mut out,
            source,
            self.file.as_deref(),
            default_file,
            &self.hints,
            &self.hint_chains,
            sources_by_file,
            use_color,
        );

        // Render related diagnostics
        rendering::render_related(
            &mut out,
            source,
            self.file.as_deref(),
            default_file,
            &self.related,
            sources_by_file,
            use_color,
        );

        if !out.ends_with('\n') {
            out.push('\n');
        }

        out
    }
}

