//! Diagnostic severity levels

/// Severity level of a diagnostic message
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Error: indicates a problem that prevents compilation/execution
    Error,
    /// Warning: indicates a potential problem that doesn't prevent execution
    Warning,
    /// Note: provides additional context or information
    Note,
    /// Help: provides guidance or suggestions
    Help,
}
