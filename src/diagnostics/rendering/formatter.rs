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
    {
        // Canonicalize both paths to handle Windows \\?\ prefix mismatches.
        let canon_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let canon_cwd = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
        if let Ok(stripped) = canon_path.strip_prefix(&canon_cwd) {
            // Always use forward slashes for consistent cross-platform display.
            return Cow::Owned(stripped.to_string_lossy().replace('\\', "/"));
        }
    }
    Cow::Borrowed(file)
}
