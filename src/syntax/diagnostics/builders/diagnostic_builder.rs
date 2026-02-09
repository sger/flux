//! Builder pattern trait for Diagnostic construction
//!
//! This trait provides a fluent API for constructing diagnostics. It's implemented
//! by the Diagnostic struct and can be extended for custom diagnostic types.

use crate::syntax::diagnostics::{
    ErrorType, Hint, HintChain, InlineSuggestion, Label, RelatedDiagnostic,
};
use crate::syntax::position::{Position, Span};

/// Builder trait for constructing diagnostics with a fluent API
///
/// This trait provides all the `with_*` methods used to configure diagnostics.
/// It's automatically implemented for Diagnostic and maintains backward compatibility
/// while organizing the builder pattern in a dedicated module.
///
/// # Example
/// ```
/// use flux::syntax::diagnostics::{diag_enhanced, UNEXPECTED_TOKEN, DiagnosticBuilder};
/// # use flux::syntax::position::{Position, Span};
/// # let span = Span::new(Position::new(1, 0), Position::new(1, 5));
///
/// let diag = diag_enhanced(&UNEXPECTED_TOKEN)
///     .with_span(span)
///     .with_message("Expected ';' after statement")
///     .with_hint_text("Add a semicolon here");
/// #
/// # // Verify the diagnostic was built correctly
/// # assert_eq!(diag.code(), Some("E034"));
/// ```
pub trait DiagnosticBuilder: Sized {
    // ===== Core Field Setters =====

    /// Set the error/warning code (e.g., "E101")
    fn with_code(self, code: impl Into<String>) -> Self;

    /// Set the error type (Compiler or Runtime)
    fn with_error_type(self, error_type: ErrorType) -> Self;

    /// Set the main diagnostic message
    fn with_message(self, message: impl Into<String>) -> Self;

    /// Set the source file path
    fn with_file(self, file: impl Into<String>) -> Self;

    /// Set the source position (converts to a zero-width span)
    fn with_position(self, position: Position) -> Self;

    /// Set the source span
    fn with_span(self, span: Span) -> Self;

    // ===== Hint Methods =====

    /// Add a hint to the diagnostic
    fn with_hint(self, hint: Hint) -> Self;

    /// Add a text-only hint (convenience method for backward compatibility)
    ///
    /// Automatically strips "Hint:" prefix if present for backward compatibility
    fn with_hint_text(self, text: impl Into<String>) -> Self;

    /// Add a hint with a source location (convenience method)
    fn with_hint_at(self, text: impl Into<String>, span: Span) -> Self;

    /// Add a hint with a source location and label (convenience method)
    fn with_hint_labeled(
        self,
        text: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) -> Self;

    /// Add a note hint (additional context or information)
    fn with_note(self, text: impl Into<String>) -> Self;

    /// Add a help hint (explicit instructions on how to fix)
    fn with_help(self, text: impl Into<String>) -> Self;

    /// Add an example hint (code example demonstrating the solution)
    fn with_example(self, text: impl Into<String>) -> Self;

    /// Add a hint chain for step-by-step guidance
    fn with_hint_chain(self, chain: HintChain) -> Self;

    /// Add a hint chain from a list of steps (convenience method)
    fn with_steps<S: Into<String>>(self, steps: impl IntoIterator<Item = S>) -> Self;

    /// Add a hint chain with steps and conclusion (convenience method)
    fn with_steps_and_conclusion<S: Into<String>>(
        self,
        steps: impl IntoIterator<Item = S>,
        conclusion: impl Into<String>,
    ) -> Self;

    // ===== Label Methods =====

    /// Add a primary label to the diagnostic (main error location)
    fn with_primary_label(self, span: Span, text: impl Into<String>) -> Self;

    /// Add a secondary label to the diagnostic (additional context)
    fn with_secondary_label(self, span: Span, text: impl Into<String>) -> Self;

    /// Add a note label to the diagnostic (informational)
    fn with_note_label(self, span: Span, text: impl Into<String>) -> Self;

    /// Add a label with explicit style
    fn with_label(self, label: Label) -> Self;

    // ===== Suggestion Methods =====

    /// Add an inline code suggestion
    fn with_suggestion(self, suggestion: InlineSuggestion) -> Self;

    /// Add an inline suggestion with replacement text (convenience method)
    fn with_suggestion_replace(self, span: Span, replacement: impl Into<String>) -> Self;

    /// Add an inline suggestion with message (convenience method)
    fn with_suggestion_message(
        self,
        span: Span,
        replacement: impl Into<String>,
        message: impl Into<String>,
    ) -> Self;

    // ===== Related Diagnostics =====

    /// Add a related diagnostic entry (note/help/related)
    fn with_related(self, related: RelatedDiagnostic) -> Self;
}
