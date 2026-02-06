//! Inline suggestions for code fixes

use crate::frontend::position::Span;

/// An inline suggestion that shows how to fix the code
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineSuggestion {
    pub replacement: String,
    pub span: Span,
    pub message: Option<String>,
}

impl InlineSuggestion {
    /// Create a suggestion with a replacement text
    pub fn new(span: Span, replacement: impl Into<String>) -> Self {
        Self {
            span,
            replacement: replacement.into(),
            message: None,
        }
    }

    /// Add a message explaining the suggestion
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}
