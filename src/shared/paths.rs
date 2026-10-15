//! Paths shown to people and to other tools.
//!
//! Canonicalizing a path on Windows gives it the `\\?\` prefix of a verbatim
//! path. Windows itself accepts that, but people find it noise, and editors
//! and debuggers (VS Code, Visual Studio, cdb) may not take it for the same
//! file as its usual spelling. So a path leaves Flux without it.

use std::borrow::Cow;

/// `path` without the `\\?\` a canonical Windows path starts with:
/// `\\?\C:\src\app.flx` is `C:\src\app.flx`, and `\\?\UNC\server\share` is
/// `\\server\share`. Any other path is returned as it is.
pub fn without_verbatim_prefix(path: &str) -> Cow<'_, str> {
    if let Some(share) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}").into()
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drive_path_loses_the_prefix() {
        assert_eq!(
            without_verbatim_prefix(r"\\?\E:\src\app.flx"),
            r"E:\src\app.flx"
        );
    }

    #[test]
    fn a_share_keeps_its_two_backslashes() {
        assert_eq!(
            without_verbatim_prefix(r"\\?\UNC\server\share\app.flx"),
            r"\\server\share\app.flx"
        );
    }

    #[test]
    fn other_paths_are_unchanged() {
        assert_eq!(without_verbatim_prefix("/src/app.flx"), "/src/app.flx");
        assert_eq!(
            without_verbatim_prefix(r"E:\src\app.flx"),
            r"E:\src\app.flx"
        );
    }
}
