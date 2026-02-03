pub mod compiler_errors;
pub mod diagnostic;
pub mod format;
pub mod registry;
pub mod runtime_errors;
pub mod types;

pub use diagnostic::{Diagnostic, Severity, render_diagnostics};
pub use format::{format_message, format_message_named};
pub use registry::{ERROR_CODES, diag_enhanced, get_enhanced};
pub use types::{ErrorCode, ErrorType};

pub use compiler_errors::*;
pub use runtime_errors::*;
