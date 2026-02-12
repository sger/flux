//! Related diagnostic types for cross-file references

use crate::diagnostics::position::Span;

/// Kind of related diagnostic
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelatedKind {
    /// Additional note
    Note,
    /// Helpful suggestion
    Help,
    /// Related context
    Related,
}

/// A related diagnostic that points to another location
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedDiagnostic {
    pub kind: RelatedKind,
    pub message: String,
    pub span: Option<Span>,
    pub file: Option<String>,
}

impl RelatedDiagnostic {
    /// Create a note-style related diagnostic
    pub fn note(message: impl Into<String>) -> Self {
        Self {
            kind: RelatedKind::Note,
            message: message.into(),
            span: None,
            file: None,
        }
    }

    /// Create a help-style related diagnostic
    pub fn help(message: impl Into<String>) -> Self {
        Self {
            kind: RelatedKind::Help,
            message: message.into(),
            span: None,
            file: None,
        }
    }

    /// Create a general related diagnostic
    pub fn related(message: impl Into<String>) -> Self {
        Self {
            kind: RelatedKind::Related,
            message: message.into(),
            span: None,
            file: None,
        }
    }

    /// Add a source span to this related diagnostic
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Set the file for this related diagnostic
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
}
