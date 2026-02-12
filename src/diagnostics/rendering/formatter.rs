//! Text formatting utilities for diagnostics

use crate::diagnostics::Diagnostic;
use std::borrow::Cow;

/// Render multiple diagnostics with double newline separation
pub fn render_diagnostics(
    diagnostics: &[Diagnostic],
    source: Option<&str>,
    default_file: Option<&str>,
) -> String {
    diagnostics
        .iter()
        .map(|diag| diag.render(source, default_file))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Format a file path for display, converting absolute paths to relative when possible
pub fn render_display_path(file: &str) -> Cow<'_, str> {
    let path = std::path::Path::new(file);
    if path.is_absolute()
        && let Ok(cwd) = std::env::current_dir()
        && let Ok(stripped) = path.strip_prefix(&cwd)
    {
        return Cow::Owned(stripped.to_string_lossy().to_string());
    }
    Cow::Borrowed(file)
}
