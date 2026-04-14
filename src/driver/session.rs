use std::path::{Path, PathBuf};

use crate::driver::{DiagnosticOutputFormat, flags::DriverFlags};

#[derive(Debug, Clone)]
pub struct DriverSession {
    pub roots_only: bool,
    pub enable_optimize: bool,
    pub enable_analyze: bool,
    pub max_errors: usize,
    pub roots: Vec<PathBuf>,
    pub cache_dir: Option<PathBuf>,
    pub strict_mode: bool,
    pub strict_types: bool,
    pub diagnostics_format: DiagnosticOutputFormat,
    pub all_errors: bool,
}

impl From<&DriverFlags> for DriverSession {
    fn from(value: &DriverFlags) -> Self {
        Self {
            roots_only: value.input.roots_only,
            enable_optimize: value.language.enable_optimize,
            enable_analyze: value.language.enable_analyze,
            max_errors: value.diagnostics.max_errors,
            roots: value.input.roots.clone(),
            cache_dir: value.cache.cache_dir.clone(),
            strict_mode: value.language.strict_mode,
            strict_types: value.language.strict_types,
            diagnostics_format: value.diagnostics.diagnostics_format,
            all_errors: value.diagnostics.all_errors,
        }
    }
}

impl DriverSession {
    pub fn cache_dir_path(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::DriverSession;
    use crate::driver::{DiagnosticOutputFormat, test_support::base_flags};

    #[test]
    fn derives_driver_session_from_shared_options() {
        let mut flags = base_flags();
        flags.input.roots = vec![PathBuf::from("tests/flux"), PathBuf::from("examples")];
        flags.input.roots_only = true;
        flags.input.test_filter = Some("session-only".into());
        flags.runtime.leak_detector = true;
        flags.runtime.trace = true;
        flags.dumps.dump_repr = true;
        flags.diagnostics.max_errors = 42;
        flags.diagnostics.diagnostics_format = DiagnosticOutputFormat::JsonCompact;
        flags.diagnostics.all_errors = true;
        flags.cache.cache_dir = Some(PathBuf::from(".flux-cache"));
        flags.cache.no_cache = true;
        flags.language.enable_optimize = true;
        flags.language.strict_mode = true;

        let session = DriverSession::from(&flags);

        assert!(session.roots_only);
        assert!(session.enable_optimize);
        assert!(!session.enable_analyze);
        assert_eq!(session.max_errors, 42);
        assert_eq!(session.roots, flags.input.roots);
        assert_eq!(session.cache_dir, Some(PathBuf::from(".flux-cache")));
        assert!(session.strict_mode);
        assert!(!session.strict_types);
        assert_eq!(
            session.diagnostics_format,
            DiagnosticOutputFormat::JsonCompact
        );
        assert!(session.all_errors);
        assert!(flags.runtime.trace);
        assert!(flags.cache.no_cache);
        assert_eq!(flags.input.test_filter.as_deref(), Some("session-only"));
    }

    #[test]
    fn cache_dir_path_returns_borrowed_path() {
        let session = DriverSession {
            roots_only: false,
            enable_optimize: false,
            enable_analyze: false,
            max_errors: 1,
            roots: Vec::new(),
            cache_dir: Some(PathBuf::from("target/flux-cache")),
            strict_mode: false,
            strict_types: false,
            diagnostics_format: DiagnosticOutputFormat::Text,
            all_errors: false,
        };

        assert_eq!(
            session.cache_dir_path(),
            Some(PathBuf::from("target/flux-cache").as_path())
        );
    }
}
