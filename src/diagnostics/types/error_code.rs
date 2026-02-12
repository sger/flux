//! Error code types for the diagnostics system

/// Distinguishes between compile-time and runtime errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    Compiler,
    Runtime,
}

impl ErrorType {
    /// Returns the prefix string used in error headers
    pub fn prefix(&self) -> &'static str {
        match self {
            ErrorType::Compiler => "Compiler error",
            ErrorType::Runtime => "Runtime error",
        }
    }
}

/// Enhanced error code with message template and optional hint
#[derive(Debug, Clone, Copy)]
pub struct ErrorCode {
    pub code: &'static str,
    pub title: &'static str,
    pub error_type: ErrorType,
    pub message: &'static str,
    pub hint: Option<&'static str>,
}
