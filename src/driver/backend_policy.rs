use crate::driver::{
    AetherDumpMode, CoreDumpMode, backend::Backend, flags::DriverFlags,
    reporting::report::TraceBackend,
};

pub fn has_dump_requests(flags: &DriverFlags) -> bool {
    flags.dumps.dump_aether != AetherDumpMode::None
        || !matches!(flags.dumps.dump_core, CoreDumpMode::None)
        || flags.dumps.dump_cfg
        || flags.dumps.dump_lir
        || flags.dumps.dump_lir_llvm
        || flags.runtime.trace_aether
        || flags.dumps.dump_repr
}

pub fn allow_vm_cache(flags: &DriverFlags) -> bool {
    flags.backend.selected == Backend::Vm && !has_dump_requests(flags)
}

pub fn validate_dump_flags(flags: &DriverFlags) -> Result<(), &'static str> {
    if (flags.dumps.dump_lir || flags.dumps.dump_lir_llvm)
        && flags.backend.selected != Backend::Native
    {
        return Err(
            "Error: --dump-lir/--dump-lir-llvm requires the native backend (use --native).",
        );
    }
    Ok(())
}

pub fn validate_flags(flags: &DriverFlags, is_test_mode: bool) -> Result<(), &'static str> {
    #[cfg(not(feature = "llvm"))]
    {
        if flags.backend.use_llvm
            || flags.backend.emit_llvm
            || flags.backend.emit_binary
            || flags.dumps.dump_lir
            || flags.dumps.dump_lir_llvm
        {
            return Err("Error: native backend features require `llvm`.");
        }
    }

    if flags.runtime.trace_aether
        && (!matches!(flags.dumps.dump_core, CoreDumpMode::None)
            || flags.dumps.dump_repr
            || flags.dumps.dump_cfg
            || flags.dumps.dump_aether != AetherDumpMode::None
            || is_test_mode)
    {
        return Err(
            "Error: --trace-aether only supports normal program execution. Use --dump-aether for report-only output.",
        );
    }

    validate_dump_flags(flags)?;
    Ok(())
}

pub fn should_prewarm_toolchain(flags: &DriverFlags) -> bool {
    flags.runtime.verbose && flags.is_native_backend()
}

pub fn native_cache_available() -> bool {
    cfg!(feature = "llvm")
}

pub fn native_cache_unavailable_message() -> &'static str {
    "native cache inspection requires `llvm` feature"
}

pub fn should_run_tests_native(flags: &DriverFlags) -> bool {
    flags.is_native_backend()
}

pub fn should_render_native_runtime_error(flags: &DriverFlags) -> bool {
    flags.is_native_backend()
}

pub fn compile_backend_label(flags: &DriverFlags) -> &'static str {
    if flags.is_native_backend() {
        "llvm"
    } else {
        "bytecode"
    }
}

pub fn execute_backend_label(flags: &DriverFlags) -> &'static str {
    if flags.is_native_backend() {
        "native"
    } else {
        "vm"
    }
}

pub fn should_show_native_cache(flags: &DriverFlags) -> bool {
    flags.is_native_backend() && native_cache_available()
}

pub fn vm_run_banner() -> &'static str {
    "[cfg→vm] Running via CFG → bytecode VM backend..."
}

pub fn native_lir_lowering_banner() -> &'static str {
    "[lir→llvm] Compiling via LIR → LLVM native backend..."
}

pub fn native_module_lowering_banner() -> &'static str {
    "[lir→llvm] Compiling via per-module LLVM native backend..."
}

pub fn native_binary_up_to_date_banner() -> &'static str {
    "[lir→llvm] binary up-to-date, skipping link"
}

pub(crate) fn trace_backend_label(backend: TraceBackend) -> &'static str {
    match backend {
        TraceBackend::Vm => "vm",
        TraceBackend::Native => "native",
    }
}

pub fn vm_cache_label() -> &'static str {
    "vm"
}

pub fn native_cache_label() -> &'static str {
    "native"
}

pub fn cache_artifact_prefix(backend: Backend) -> &'static str {
    match backend {
        Backend::Vm => "vm artifact",
        Backend::Native => "native artifact",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{AetherDumpMode, CoreDumpMode, test_support::base_flags};

    #[test]
    fn allow_vm_cache_respects_dumps_and_backend() {
        let mut flags = base_flags();
        assert!(allow_vm_cache(&flags));
        flags.dumps.dump_cfg = true;
        assert!(!allow_vm_cache(&flags));
        flags.dumps.dump_cfg = false;
        flags.backend.selected = Backend::Native;
        assert!(!allow_vm_cache(&flags));
    }

    #[test]
    fn validate_flags_rejects_trace_aether_with_dump() {
        let mut flags = base_flags();
        flags.runtime.trace_aether = true;
        flags.dumps.dump_core = CoreDumpMode::Readable;
        assert!(validate_flags(&flags, false).is_err());
    }

    #[test]
    fn validate_flags_rejects_trace_aether_in_tests() {
        let mut flags = base_flags();
        flags.runtime.trace_aether = true;
        assert!(validate_flags(&flags, true).is_err());
    }

    #[test]
    fn validate_dump_flags_requires_native_backend() {
        let mut flags = base_flags();
        flags.dumps.dump_lir = true;
        assert!(validate_dump_flags(&flags).is_err());
        flags.backend.selected = Backend::Native;
        assert!(validate_dump_flags(&flags).is_ok());
    }

    #[test]
    fn backend_labels_match_backend() {
        let mut flags = base_flags();
        assert_eq!(compile_backend_label(&flags), "bytecode");
        assert_eq!(execute_backend_label(&flags), "vm");
        flags.backend.selected = Backend::Native;
        assert_eq!(compile_backend_label(&flags), "llvm");
        assert_eq!(execute_backend_label(&flags), "native");
    }

    #[test]
    fn cache_artifact_prefix_matches_backend() {
        assert_eq!(cache_artifact_prefix(Backend::Vm), "vm artifact");
        assert_eq!(cache_artifact_prefix(Backend::Native), "native artifact");
    }

    #[test]
    fn trace_backend_labels_match_backend() {
        assert_eq!(trace_backend_label(TraceBackend::Vm), "vm");
        assert_eq!(trace_backend_label(TraceBackend::Native), "native");
    }

    #[test]
    fn native_cache_visibility_depends_on_backend() {
        let mut flags = base_flags();
        assert!(!should_show_native_cache(&flags));
        flags.backend.selected = Backend::Native;
        if native_cache_available() {
            assert!(should_show_native_cache(&flags));
        } else {
            assert!(!should_show_native_cache(&flags));
        }
    }

    #[test]
    fn test_execution_backend_policy_tracks_selected_backend() {
        let mut flags = base_flags();
        assert!(!should_run_tests_native(&flags));
        assert!(!should_render_native_runtime_error(&flags));

        flags.backend.selected = Backend::Native;

        assert!(should_run_tests_native(&flags));
        assert!(should_render_native_runtime_error(&flags));
    }

    #[test]
    fn prewarm_toolchain_requires_verbose_native_execution() {
        let mut flags = base_flags();
        assert!(!should_prewarm_toolchain(&flags));

        flags.runtime.verbose = true;
        assert!(!should_prewarm_toolchain(&flags));

        flags.backend.selected = Backend::Native;
        assert!(should_prewarm_toolchain(&flags));
    }

    #[test]
    fn has_dump_requests_checks_all_dump_groups() {
        let mut flags = base_flags();
        assert!(!has_dump_requests(&flags));

        flags.dumps.dump_repr = true;
        assert!(has_dump_requests(&flags));
        flags.dumps.dump_repr = false;

        flags.dumps.dump_cfg = true;
        assert!(has_dump_requests(&flags));
        flags.dumps.dump_cfg = false;

        flags.dumps.dump_core = CoreDumpMode::Readable;
        assert!(has_dump_requests(&flags));
        flags.dumps.dump_core = CoreDumpMode::None;

        flags.dumps.dump_aether = AetherDumpMode::Summary;
        assert!(has_dump_requests(&flags));
        flags.dumps.dump_aether = AetherDumpMode::None;

        flags.dumps.dump_lir = true;
        assert!(has_dump_requests(&flags));
        flags.dumps.dump_lir = false;

        flags.dumps.dump_lir_llvm = true;
        assert!(has_dump_requests(&flags));
        flags.dumps.dump_lir_llvm = false;

        flags.runtime.trace_aether = true;
        assert!(has_dump_requests(&flags));
    }

    #[test]
    fn finalize_backend_uses_grouped_backend_flags() {
        let mut flags = base_flags();
        flags.backend.use_llvm = true;
        flags = flags.finalize_backend();
        assert_eq!(flags.backend.selected, Backend::Native);
    }
}
