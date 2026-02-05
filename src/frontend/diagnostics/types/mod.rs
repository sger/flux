//! Type definitions for the diagnostics system
//!
//! This module contains the core types used throughout the diagnostics system,
//! including severity levels, hints, labels, suggestions, related diagnostics,
//! and error code definitions.

mod error_code;
mod hint;
mod label;
mod related;
mod severity;
mod suggestion;

pub use error_code::{ErrorCode, ErrorType};
pub use hint::{Hint, HintChain, HintKind};
pub use label::{Label, LabelStyle};
pub use related::{RelatedDiagnostic, RelatedKind};
pub use severity::Severity;
pub use suggestion::InlineSuggestion;
