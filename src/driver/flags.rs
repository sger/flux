use std::path::PathBuf;

use crate::driver::{
    backend::Backend,
    backend_policy,
    mode::{AetherDumpMode, CoreDumpMode, DiagnosticOutputFormat},
};

/// Backend-selection and backend-output switches for a driver invocation.
#[derive(Debug, Clone)]
pub struct DriverBackendFlags {
    pub selected: Backend,
    pub use_core_to_llvm: bool,
    pub emit_llvm: bool,
    pub emit_binary: bool,
    pub output_path: Option<String>,
}

/// Input-path and module-root options for a driver invocation.
#[derive(Debug, Clone)]
pub struct DriverInputFlags {
    pub input_path: Option<String>,
    pub roots: Vec<PathBuf>,
    pub roots_only: bool,
    pub test_filter: Option<String>,
}

/// Runtime-only execution controls that do not affect frontend semantics.
#[derive(Debug, Clone)]
pub struct DriverRuntimeFlags {
    pub verbose: bool,
    pub leak_detector: bool,
    pub trace: bool,
    pub trace_aether: bool,
    pub show_stats: bool,
    pub profiling: bool,
}

/// Dump and inspection surfaces emitted by the driver.
#[derive(Debug, Clone)]
pub struct DriverDumpFlags {
    pub dump_repr: bool,
    pub dump_cfg: bool,
    pub dump_core: CoreDumpMode,
    pub dump_aether: AetherDumpMode,
    pub dump_lir: bool,
    pub dump_lir_llvm: bool,
}

/// Diagnostic rendering and reporting options shared across driver modes.
#[derive(Debug, Clone)]
pub struct DriverDiagnosticFlags {
    pub max_errors: usize,
    pub diagnostics_format: DiagnosticOutputFormat,
    pub all_errors: bool,
}

/// Cache configuration for compiler artifacts and reusable outputs.
#[derive(Debug, Clone)]
pub struct DriverCacheFlags {
    pub cache_dir: Option<PathBuf>,
    pub no_cache: bool,
}

/// Frontend/lowering semantic knobs that affect compilation behavior.
#[derive(Debug, Clone)]
pub struct DriverLanguageFlags {
    pub enable_optimize: bool,
    pub enable_analyze: bool,
    pub strict_mode: bool,
    pub strict_types: bool,
}

/// All per-invocation driver options, grouped by concern.
///
/// This keeps command parsing explicit while avoiding a single flat "bag of flags"
/// passed through every driver subsystem.
#[derive(Debug, Clone)]
pub struct DriverFlags {
    pub backend: DriverBackendFlags,
    pub input: DriverInputFlags,
    pub runtime: DriverRuntimeFlags,
    pub dumps: DriverDumpFlags,
    pub diagnostics: DriverDiagnosticFlags,
    pub cache: DriverCacheFlags,
    pub language: DriverLanguageFlags,
}

impl DriverFlags {
    /// Recomputes the effective backend from the backend-related switches.
    pub fn finalize_backend(mut self) -> Self {
        self.backend.selected = Backend::select(&self);
        self
    }

    /// Returns true when the invocation should run through the native backend.
    pub fn is_native_backend(&self) -> bool {
        self.backend.selected == Backend::Native
    }

    /// Returns true when any dump/report surface disables normal VM-cache reuse.
    pub fn has_dump_requests(&self) -> bool {
        backend_policy::has_dump_requests(self)
    }

    /// Returns true when VM bytecode cache reuse is allowed for this invocation.
    pub fn allow_vm_cache(&self) -> bool {
        backend_policy::allow_vm_cache(self)
    }
}
