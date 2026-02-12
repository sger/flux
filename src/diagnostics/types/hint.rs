//! Hint types for providing guidance and additional context in diagnostics

use crate::syntax::position::Span;

/// Kind of hint to display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HintKind {
    /// General hint or suggestion (default)
    Hint,
    /// Additional context or information
    Note,
    /// Explicit help on how to fix
    Help,
    /// Code example demonstrating the solution
    Example,
}

/// A hint with optional source location and label
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub kind: HintKind,
    pub text: String,
    pub span: Option<Span>,
    pub label: Option<String>,
    pub file: Option<String>,
}

impl Hint {
    /// Create a simple text-only hint
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: HintKind::Hint,
            text: text.into(),
            span: None,
            label: None,
            file: None,
        }
    }

    /// Create a hint with a source location
    pub fn at(text: impl Into<String>, span: Span) -> Self {
        Self {
            kind: HintKind::Hint,
            text: text.into(),
            span: Some(span),
            label: None,
            file: None,
        }
    }

    /// Create a hint with a source location and label
    pub fn labeled(text: impl Into<String>, span: Span, label: impl Into<String>) -> Self {
        Self {
            kind: HintKind::Hint,
            text: text.into(),
            span: Some(span),
            label: Some(label.into()),
            file: None,
        }
    }

    /// Create a note hint
    pub fn note(text: impl Into<String>) -> Self {
        Self {
            kind: HintKind::Note,
            text: text.into(),
            span: None,
            label: None,
            file: None,
        }
    }

    /// Create a help hint
    pub fn help(text: impl Into<String>) -> Self {
        Self {
            kind: HintKind::Help,
            text: text.into(),
            span: None,
            label: None,
            file: None,
        }
    }

    /// Create an example hint
    pub fn example(text: impl Into<String>) -> Self {
        Self {
            kind: HintKind::Example,
            text: text.into(),
            span: None,
            label: None,
            file: None,
        }
    }

    /// Add a label to this hint
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the file for this hint (for cross-file references)
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
}

/// A hint chain that provides step-by-step guidance for complex errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HintChain {
    pub steps: Vec<String>,
    pub conclusion: Option<String>,
}

impl HintChain {
    /// Create a new hint chain with the given steps
    pub fn new(steps: Vec<String>) -> Self {
        Self {
            steps,
            conclusion: None,
        }
    }

    /// Add a conclusion to the hint chain
    pub fn with_conclusion(mut self, conclusion: impl Into<String>) -> Self {
        self.conclusion = Some(conclusion.into());
        self
    }

    /// Create a hint chain from a slice of step strings
    pub fn from_steps<S: Into<String>>(steps: impl IntoIterator<Item = S>) -> Self {
        Self {
            steps: steps.into_iter().map(|s| s.into()).collect(),
            conclusion: None,
        }
    }
}
