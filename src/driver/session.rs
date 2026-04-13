use std::path::{Path, PathBuf};

use crate::driver::{flags::DriverFlags, mode::DiagnosticOutputFormat};

#[derive(Debug, Clone)]
pub struct DriverSession {
    pub root_only: bool,
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
            root_only: value.roots_only,
            enable_optimize: value.enable_optimize,
            enable_analyze: value.enable_analyze,
            max_errors: value.max_errors,
            roots: value.roots.clone(),
            cache_dir: value.cache_dir.clone(),
            strict_mode: value.strict_mode,
            strict_types: value.strict_types,
            diagnostics_format: value.diagnostics_format,
            all_errors: value.all_errors,
        }
    }
}

impl DriverSession {
    pub fn cache_dir_path(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use crate::driver::{flags::DriverFlags, mode::DiagnosticOutputFormat, session::DriverSession};

    #[test]
    fn derives_driver_session_from_shared_options() {
        let flags = DriverFlags {
            input_path: None,
            roots_only: true,
            enable_optimize: true,
            enable_analyze: false,
            max_errors: 42,
            roots: vec![PathBuf::from("tests/flux"), PathBuf::from("examples")],
            cache_dir: Some(PathBuf::from(".flux-cache")),
            strict_mode: true,
            strict_types: false,
            diagnostics_format: crate::driver::mode::DiagnosticOutputFormat::JsonCompact,
            all_errors: true,
            verbose: false,
            leak_detector: false,
            trace: false,
            no_cache: false,
            show_stats: false,
            trace_aether: false,
            profiling: false,
            dump_repr: false,
            dump_cfg: false,
            dump_core: crate::driver::mode::CoreDumpMode::None,
            dump_aether: crate::driver::mode::AetherDumpMode::None,
            dump_lir: false,
            dump_lir_llvm: false,
            use_core_to_llvm: false,
            emit_llvm: false,
            emit_binary: false,
            output_path: None,
            test_filter: None,
        };

        let session = DriverSession::from(&flags);

        assert!(session.root_only);
        assert!(session.enable_optimize);
        assert!(!session.enable_analyze);
        assert_eq!(session.max_errors, 42);
        assert_eq!(session.roots, flags.roots);
        assert_eq!(session.cache_dir, Some(PathBuf::from(".flux-cache")));
        assert!(session.strict_mode);
        assert!(!session.strict_types);
        assert_eq!(
            session.diagnostics_format,
            DiagnosticOutputFormat::JsonCompact
        );
        assert!(session.all_errors);
    }

    #[test]
    fn cache_dir_path_returns_borrowed_path() {
        let session = DriverSession {
            root_only: false,
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
        )
    }
}
