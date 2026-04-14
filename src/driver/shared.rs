use crate::{
    diagnostics::{Diagnostic, DiagnosticPhase, DiagnosticsAggregator},
    driver::{
        flags::DriverFlags,
        mode::DiagnosticOutputFormat,
        session::DriverSession,
        support::shared::{DiagnosticRenderRequest, emit_diagnostics, tag_diagnostics},
    },
    shared::cache_paths::CacheLayout,
    syntax::module_graph::ModuleKind,
};

/// Shared compile-time language switches used across driver request types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DriverCompileConfig {
    pub(crate) strict_mode: bool,
    pub(crate) strict_types: bool,
    pub(crate) enable_optimize: bool,
    pub(crate) enable_analyze: bool,
}

impl From<&DriverSession> for DriverCompileConfig {
    fn from(value: &DriverSession) -> Self {
        Self {
            strict_mode: value.strict_mode,
            strict_types: value.strict_types,
            enable_optimize: value.enable_optimize,
            enable_analyze: value.enable_analyze,
        }
    }
}

/// Shared cache configuration used across driver request types.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DriverCacheConfig<'a> {
    pub(crate) cache_layout: &'a CacheLayout,
    pub(crate) no_cache: bool,
}

impl<'a> DriverCacheConfig<'a> {
    pub(crate) fn new(cache_layout: &'a CacheLayout, no_cache: bool) -> Self {
        Self {
            cache_layout,
            no_cache,
        }
    }
}

/// Shared diagnostics policy used across driver entrypoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DriverDiagnosticConfig {
    pub(crate) max_errors: usize,
    pub(crate) diagnostics_format: DiagnosticOutputFormat,
    pub(crate) all_errors: bool,
}

impl From<&DriverSession> for DriverDiagnosticConfig {
    fn from(value: &DriverSession) -> Self {
        Self {
            max_errors: value.max_errors,
            diagnostics_format: value.diagnostics_format,
            all_errors: value.all_errors,
        }
    }
}

/// Shared runtime reporting switches used across driver request types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DriverRuntimeConfig {
    pub(crate) verbose: bool,
    pub(crate) show_stats: bool,
    pub(crate) trace: bool,
    pub(crate) trace_aether: bool,
    pub(crate) profiling: bool,
    pub(crate) leak_detector: bool,
}

impl From<&DriverFlags> for DriverRuntimeConfig {
    fn from(value: &DriverFlags) -> Self {
        Self {
            verbose: value.runtime.verbose,
            show_stats: value.runtime.show_stats,
            trace: value.runtime.trace,
            trace_aether: value.runtime.trace_aether,
            profiling: value.runtime.profiling,
            leak_detector: value.runtime.leak_detector,
        }
    }
}

/// Tags diagnostics with a phase and fills in the source path when missing.
pub(crate) fn tag_and_attach_file(diags: &mut Vec<Diagnostic>, phase: DiagnosticPhase, path: &str) {
    tag_diagnostics(diags, phase);
    for diag in diags {
        if diag.file().is_none() {
            diag.set_file(path.to_string());
        }
    }
}

/// Emits diagnostics and exits when any error diagnostics are present.
pub(crate) fn emit_diagnostics_or_exit(
    diagnostics: &[Diagnostic],
    path: &str,
    source: &str,
    show_file_headers: bool,
    config: DriverDiagnosticConfig,
) {
    if diagnostics.is_empty() {
        return;
    }

    let report = DiagnosticsAggregator::new(diagnostics)
        .with_default_source(path, source)
        .with_file_headers(show_file_headers)
        .with_max_errors(Some(config.max_errors))
        .with_stage_filtering(!config.all_errors)
        .report();

    emit_diagnostics(DiagnosticRenderRequest {
        diagnostics,
        default_file: Some(path),
        default_source: Some(source),
        show_file_headers,
        max_errors: config.max_errors,
        format: config.diagnostics_format,
        all_errors: config.all_errors,
        text_to_stderr: true,
    });

    if report.counts.errors > 0 {
        std::process::exit(1);
    }
}

/// Keeps stdlib modules ahead of user modules in deterministic driver traversals.
pub(crate) fn sort_stdlib_first<T>(items: &mut [T], kind_of: impl Fn(&T) -> ModuleKind) {
    items.sort_by_key(|item| {
        if kind_of(item) == ModuleKind::FlowStdlib {
            0
        } else {
            1
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        DriverCompileConfig, DriverDiagnosticConfig, DriverRuntimeConfig, sort_stdlib_first,
        tag_and_attach_file,
    };
    use crate::{
        diagnostics::{Diagnostic, DiagnosticPhase},
        driver::{
            mode::DiagnosticOutputFormat,
            test_support::{base_flags, base_session},
        },
        syntax::module_graph::ModuleKind,
    };
    #[test]
    fn compile_config_matches_session() {
        let mut session = base_session();
        session.strict_mode = true;
        session.strict_types = true;
        session.enable_optimize = true;

        let config = DriverCompileConfig::from(&session);

        assert!(config.strict_mode);
        assert!(config.strict_types);
        assert!(config.enable_optimize);
        assert!(!config.enable_analyze);
    }

    #[test]
    fn diagnostic_config_matches_session() {
        let mut session = base_session();
        session.max_errors = 42;
        session.diagnostics_format = DiagnosticOutputFormat::JsonCompact;
        session.all_errors = true;

        let config = DriverDiagnosticConfig::from(&session);

        assert_eq!(config.max_errors, 42);
        assert_eq!(
            config.diagnostics_format,
            DiagnosticOutputFormat::JsonCompact
        );
        assert!(config.all_errors);
    }

    #[test]
    fn runtime_config_matches_flags() {
        let mut flags = base_flags();
        flags.runtime.verbose = true;
        flags.runtime.show_stats = true;
        flags.runtime.trace = true;
        flags.runtime.profiling = true;

        let config = DriverRuntimeConfig::from(&flags);

        assert!(config.verbose);
        assert!(config.show_stats);
        assert!(config.trace);
        assert!(config.profiling);
        assert!(!config.trace_aether);
    }

    #[test]
    fn tag_and_attach_file_applies_phase_and_missing_file() {
        let mut diags = vec![Diagnostic::warning("warn")];

        tag_and_attach_file(&mut diags, DiagnosticPhase::Parse, "main.flx");

        assert_eq!(diags[0].phase(), Some(DiagnosticPhase::Parse));
        assert_eq!(diags[0].file(), Some("main.flx"));
    }

    #[test]
    fn sort_stdlib_first_places_flow_modules_before_user_modules() {
        #[derive(Clone, Copy)]
        struct TestNode {
            kind: ModuleKind,
        }
        let mut nodes = vec![
            TestNode {
                kind: ModuleKind::User,
            },
            TestNode {
                kind: ModuleKind::FlowStdlib,
            },
        ];

        sort_stdlib_first(&mut nodes, |node| node.kind);

        assert_eq!(nodes[0].kind, ModuleKind::FlowStdlib);
        assert_eq!(nodes[1].kind, ModuleKind::User);
    }
}
