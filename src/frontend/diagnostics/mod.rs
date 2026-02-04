//! Diagnostics module.
//!
//! Provides structured diagnostics with severity, optional error codes, source spans,
//! and rendering helpers for consistent compiler/runtime output.

pub mod aggregator;
pub mod compiler_errors;
pub mod diagnostic;
pub mod format;
pub mod registry;
pub mod runtime_errors;
pub mod types;

pub use aggregator::{
    DEFAULT_MAX_ERRORS, DiagnosticCounts, DiagnosticsAggregator, DiagnosticsReport,
    render_diagnostics_multi,
};
pub use diagnostic::{
    Diagnostic, Hint, HintChain, HintKind, InlineSuggestion, Label, LabelStyle, RelatedDiagnostic,
    RelatedKind, Severity, render_diagnostics, render_display_path,
};
pub use format::{format_message, format_message_named};
pub use registry::{ERROR_CODES, diag_enhanced, lookup_error_code};
pub use types::{ErrorCode, ErrorType};

pub use compiler_errors::*;
pub use runtime_errors::*;
