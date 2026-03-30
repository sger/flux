//! Text formatting utilities for diagnostics

use crate::diagnostics::Diagnostic;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

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

fn strip_windows_verbatim_prefix(path: &str) -> Cow<'_, str> {
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return Cow::Owned(rest.replace('\\', "/"));
    }
    if let Some(rest) = path.strip_prefix("//?/") {
        let is_drive_path = rest.as_bytes().get(1) == Some(&b':')
            && rest
                .as_bytes()
                .first()
                .is_some_and(|b| b.is_ascii_alphabetic());
        if is_drive_path {
            return Cow::Owned(rest.to_string());
        }
        return Cow::Owned(format!("/{rest}"));
    }
    Cow::Borrowed(path)
}

fn path_buf_from_display_str(file: &str) -> PathBuf {
    let stripped = strip_windows_verbatim_prefix(file);
    if std::path::MAIN_SEPARATOR == '/' {
        PathBuf::from(stripped.as_ref())
    } else {
        PathBuf::from(stripped.as_ref().replace('/', "\\"))
    }
}

/// Format a file path for display, converting absolute paths to relative when possible
pub fn render_display_path(file: &str) -> Cow<'_, str> {
    let stripped = strip_windows_verbatim_prefix(file);
    let normalized = stripped.as_ref().replace('\\', "/");
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_normalized = cwd.to_string_lossy().replace('\\', "/");
        if let Some(rel) = normalized.strip_prefix(&(cwd_normalized.clone() + "/")) {
            return Cow::Owned(rel.to_string());
        }
        if let Ok(canon_cwd) = std::fs::canonicalize(&cwd) {
            let canon_cwd_normalized = canon_cwd.to_string_lossy().replace('\\', "/");
            if let Some(rel) = normalized.strip_prefix(&(canon_cwd_normalized + "/")) {
                return Cow::Owned(rel.to_string());
            }
        }
    }

    let path_buf = path_buf_from_display_str(stripped.as_ref());
    let path = Path::new(&path_buf);
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
    Cow::Owned(normalized)
}
