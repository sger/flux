//! Stack trace types for runtime diagnostics.

/// A single rendered frame in a runtime stack trace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StackTraceFrame {
    /// Human-readable frame text, e.g. `main (src/app.flx:10:5)`.
    pub text: String,
}

impl StackTraceFrame {
    /// Create a stack trace frame from its rendered text.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}
