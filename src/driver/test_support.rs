use crate::driver::{
    AetherDumpMode, CoreDumpMode, DiagnosticOutputFormat,
    backend::Backend,
    flags::{
        DriverBackendFlags, DriverCacheFlags, DriverDiagnosticFlags, DriverDumpFlags, DriverFlags,
        DriverInputFlags, DriverLanguageFlags, DriverRuntimeFlags,
    },
    session::DriverSession,
};

pub(crate) fn base_flags() -> DriverFlags {
    DriverFlags {
        backend: DriverBackendFlags {
            selected: Backend::Vm,
            use_llvm: false,
            emit_llvm: false,
            emit_binary: false,
            output_path: None,
        },
        input: DriverInputFlags {
            input_path: None,
            roots: Vec::new(),
            roots_only: false,
            test_filter: None,
        },
        runtime: DriverRuntimeFlags {
            verbose: false,
            leak_detector: false,
            trace: false,
            trace_aether: false,
            show_stats: false,
            profiling: false,
        },
        dumps: DriverDumpFlags {
            dump_repr: false,
            dump_cfg: false,
            dump_core: CoreDumpMode::None,
            dump_aether: AetherDumpMode::None,
            dump_lir: false,
            dump_lir_llvm: false,
        },
        diagnostics: DriverDiagnosticFlags {
            max_errors: 1,
            diagnostics_format: DiagnosticOutputFormat::Text,
            all_errors: false,
        },
        cache: DriverCacheFlags {
            cache_dir: None,
            no_cache: false,
        },
        language: DriverLanguageFlags {
            enable_optimize: false,
            enable_analyze: false,
            strict_mode: false,
            strict_inference: false,
        },
    }
}

pub(crate) fn base_session() -> DriverSession {
    DriverSession::from(&base_flags())
}
