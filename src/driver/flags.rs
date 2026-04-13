use std::path::PathBuf;

use crate::driver::{
    backend::Backend,
    backend_policy,
    mode::{AetherDumpMode, CoreDumpMode, DiagnosticOutputFormat},
};

#[derive(Debug, Clone)]
pub struct DriverFlags {
    pub backend: Backend,
    pub input_path: Option<String>,
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
    pub verbose: bool,
    pub leak_detector: bool,
    pub trace: bool,
    pub no_cache: bool,
    pub show_stats: bool,
    pub trace_aether: bool,
    pub profiling: bool,
    pub dump_repr: bool,
    pub dump_cfg: bool,
    pub dump_core: CoreDumpMode,
    pub dump_aether: AetherDumpMode,
    pub dump_lir: bool,
    pub dump_lir_llvm: bool,
    pub use_core_to_llvm: bool,
    pub emit_llvm: bool,
    pub emit_binary: bool,
    pub output_path: Option<String>,
    pub test_filter: Option<String>,
}

impl DriverFlags {
    pub fn finalize_backend(mut self) -> Self {
        self.backend = Backend::select(&self);
        self
    }

    pub fn is_native_backend(&self) -> bool {
        self.backend == Backend::Native
    }

    pub fn has_dump_requests(&self) -> bool {
        backend_policy::has_dump_requests(self)
    }

    pub fn allow_vm_cache(&self) -> bool {
        backend_policy::allow_vm_cache(self)
    }
}
