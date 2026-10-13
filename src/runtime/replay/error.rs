//! Why recording or replaying a tape stopped, as a diagnostic.

use serde::{Deserialize, Serialize};

use crate::diagnostics::position::Span;
use crate::diagnostics::registry::diagnostic_for;
use crate::diagnostics::types::{DiagnosticPhase, ErrorCode};
use crate::diagnostics::{
    Diagnostic, DiagnosticBuilder, REPLAY_BAD_TAPE, REPLAY_KIND_MISMATCH, REPLAY_LEFTOVER_EVENTS,
    REPLAY_PAYLOAD_MISMATCH, REPLAY_TAPE_EXHAUSTED, REPLAY_UNSUPPORTED_EFFECT,
};

/// The prefix of the stderr line a native program writes when its tape stops
/// it; the driver turns the line back into a diagnostic.
pub const NATIVE_WIRE_PREFIX: &str = "flux-tape-error: ";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum TapeError {
    /// E1022: the program performed a different kind of effect.
    KindMismatch {
        index: u64,
        expected: String,
        actual: String,
    },
    /// E1023: the same kind of effect, with different details.
    PayloadMismatch {
        index: u64,
        what: String,
        tape: String,
        program: String,
    },
    /// E1024: the program performed an effect after the tape ended.
    Exhausted { count: u64, actual: String },
    /// E1025: the program finished with events still unreplayed.
    Leftover {
        count: u64,
        first_index: u64,
        first: String,
    },
    /// E1026: the tape cannot be read, written, or used for this program.
    BadTape { path: String, reason: String },
    /// E1027: an effect that cannot be recorded yet.
    Unsupported { what: String },
}

impl TapeError {
    pub fn code(&self) -> &'static ErrorCode {
        match self {
            TapeError::KindMismatch { .. } => &REPLAY_KIND_MISMATCH,
            TapeError::PayloadMismatch { .. } => &REPLAY_PAYLOAD_MISMATCH,
            TapeError::Exhausted { .. } => &REPLAY_TAPE_EXHAUSTED,
            TapeError::Leftover { .. } => &REPLAY_LEFTOVER_EVENTS,
            TapeError::BadTape { .. } => &REPLAY_BAD_TAPE,
            TapeError::Unsupported { .. } => &REPLAY_UNSUPPORTED_EFFECT,
        }
    }

    pub fn message(&self) -> String {
        match self {
            TapeError::KindMismatch {
                index,
                expected,
                actual,
            } => format!(
                "Replay diverged at event #{index}: the tape expected {expected}, but the program performed {actual}."
            ),
            TapeError::PayloadMismatch {
                index,
                what,
                tape,
                program,
            } => format!(
                "Replay diverged at event #{index}: {what} differs. The tape has {tape}, the program has {program}."
            ),
            TapeError::Exhausted { count, actual } => format!(
                "The tape ended after {count} events, but the program then performed {actual}."
            ),
            TapeError::Leftover {
                count,
                first_index,
                first,
            } => format!(
                "The program finished, but {count} recorded events were never replayed, starting at #{first_index}: {first}."
            ),
            TapeError::BadTape { path, reason } => format!("Cannot use tape `{path}`: {reason}."),
            TapeError::Unsupported { what } => {
                format!("`{what}` cannot be recorded or replayed yet.")
            }
        }
    }

    /// The diagnostic for this error in `file`, at `span` when the failing
    /// effect has a source position (on native, and for a tape rejected
    /// before the program starts, it has none).
    pub fn to_diagnostic(&self, file: &str, span: Option<Span>) -> Diagnostic {
        let diagnostic = diagnostic_for(self.code())
            .with_phase(DiagnosticPhase::Runtime)
            .with_message(self.message())
            .with_file(file.to_string());
        let diagnostic = match span {
            Some(span) => diagnostic.with_span(span),
            None => diagnostic,
        };
        match self.code().hint {
            Some(hint) => diagnostic.with_hint_text(hint),
            None => diagnostic,
        }
    }

    /// The single stderr line a native program writes for this error.
    pub fn to_wire(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string());
        format!("{NATIVE_WIRE_PREFIX}{json}")
    }

    /// Finds the error a native program reported on stderr, if any.
    pub fn from_stderr(stderr: &str) -> Option<TapeError> {
        stderr.lines().find_map(|line| {
            let json = line.strip_prefix(NATIVE_WIRE_PREFIX)?;
            serde_json::from_str(json).ok()
        })
    }
}

impl std::fmt::Display for TapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for TapeError {}
