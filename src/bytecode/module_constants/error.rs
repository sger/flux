//! Error types for constant evaluation.

/// Error during compile-time constant evaluation.
#[derive(Debug, Clone)]
pub struct ConstEvalError {
    /// Error code (e.g., "E041")
    pub code: &'static str,
    /// Error message
    pub message: String,
    /// Optional hint for fixing the error
    pub hint: Option<String>,
}

impl ConstEvalError {
    /// Create a new constant evaluation error.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    /// Add a hint to the error.
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}
