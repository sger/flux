//! Label types for inline source code annotations

use crate::frontend::position::Span;

/// Style for inline source labels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabelStyle {
    /// Primary label - main focus of the error (rendered in red)
    Primary,
    /// Secondary label - additional context (rendered in blue)
    Secondary,
    /// Note label - informational (rendered in cyan)
    Note,
}

/// An inline label that annotates a specific span within the source snippet
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub text: String,
    pub style: LabelStyle,
}

impl Label {
    /// Create a primary label (main error location)
    pub fn primary(span: Span, text: impl Into<String>) -> Self {
        Self {
            span,
            text: text.into(),
            style: LabelStyle::Primary,
        }
    }

    /// Create a secondary label (additional context)
    pub fn secondary(span: Span, text: impl Into<String>) -> Self {
        Self {
            span,
            text: text.into(),
            style: LabelStyle::Secondary,
        }
    }

    /// Create a note label (informational)
    pub fn note(span: Span, text: impl Into<String>) -> Self {
        Self {
            span,
            text: text.into(),
            style: LabelStyle::Note,
        }
    }
}
