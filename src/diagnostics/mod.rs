//! Structured diagnostics: severity, error codes, source spans, rendering.

pub mod aggregator;
pub mod builders;
pub mod compiler_errors;
pub mod diagnostic;
pub mod format;
// `Position`/`Span` are source vocabulary, not diagnostic vocabulary — they
// live in `crate::source`. Re-exported so `diagnostics::position::Span`
// resolves, as ~everything spells it that way.
pub use crate::source::position;
pub mod quality;
pub mod ranking;
pub mod registry;
pub mod rendering;
pub mod runtime_errors;
pub mod text_similarity;
pub mod types;

pub use aggregator::{
    DEFAULT_MAX_ERRORS, DiagnosticCounts, DiagnosticsAggregator, DiagnosticsReport,
    render_diagnostics_multi,
};
pub use builders::DiagnosticBuilder;
pub use diagnostic::Diagnostic;
pub use format::{format_message, format_message_named};
pub use quality::*;
pub use registry::{ERROR_CODES, diagnostic_for, lookup_error_code};
pub use rendering::render_diagnostics_json;
pub use rendering::{render_diagnostics, render_display_path};
pub use types::{
    DiagnosticCategory, DiagnosticPhase, ErrorCode, ErrorType, Hint, HintChain, HintKind,
    InlineSuggestion, Label, LabelStyle, RelatedDiagnostic, RelatedKind, Severity, StackTraceFrame,
};

pub use compiler_errors::*;
pub use runtime_errors::*;

#[cfg(test)]
mod diagnostics_test;
