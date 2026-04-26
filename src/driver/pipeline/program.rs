//! Program execution pipeline orchestration shared by VM and native backends.

use std::{collections::HashSet, path::PathBuf, time::Instant};

use crate as flux;
#[cfg(feature = "llvm")]
use crate::driver::backend::Backend;
#[cfg(feature = "llvm")]
use crate::driver::backend_policy::{
    compile_backend_label, execute_backend_label, should_prewarm_toolchain,
    should_render_native_runtime_error,
};
#[cfg(feature = "llvm")]
use crate::driver::run_program::backend::native::{
    NativeOutputConfig, NativeProgramInput, NativeReportConfig, NativeRunRequest,
    run_native_backend,
};
use crate::driver::{
    flags::DriverFlags,
    mode::{AetherDumpMode, CoreDumpMode},
    reporting::report::print_backend_representation_contract,
    run_program::{
        backend::vm::{ParallelVmRunRequest, VmRunRequest, run_vm, try_run_parallel_vm},
        dumps::{DumpRequest, handle_dumps},
        frontend::{ProgramContext, build_program_context},
        modules::{CompileModulesRequest, compile_modules},
    },
    session::DriverSession,
    shared::{
        DriverCacheConfig, DriverCompileConfig, DriverDiagnosticConfig, DriverRuntimeConfig,
        emit_diagnostics_or_exit,
    },
};
#[cfg(feature = "llvm")]
use flux::llvm::pipeline::toolchain_info;
use flux::{
    bytecode::bytecode_cache::hash_bytes,
    compiler::Compiler,
    diagnostics::{Diagnostic, Severity},
    shared::cache_paths::CacheLayout,
    syntax::{module_graph::ModuleGraph, program::Program},
};

#[derive(Clone, Copy)]
/// Immutable request describing a single program run.
pub(crate) struct RunProgramRequest<'a> {
    pub(crate) path: &'a str,
    pub(crate) flags: &'a DriverFlags,
    pub(crate) session: &'a DriverSession,
}

/// Shared orchestration state carried through the program pipeline stages.
struct RunContext {
    source: String,
    program: Program,
    graph: ModuleGraph,
    failed_modules: HashSet<PathBuf>,
    entry_path: PathBuf,
    cache_layout: CacheLayout,
    compiler: Compiler,
    all_diagnostics: Vec<Diagnostic>,
    parse_ms: f64,
    compile_start: Instant,
    module_count: usize,
    is_multimodule: bool,
    entry_has_errors: bool,
    strict_hash: [u8; 32],
}

/// Determines whether dump handling needs a whole-program merged view.
fn should_build_merged_program(flags: &DriverFlags, is_multimodule: bool) -> bool {
    is_multimodule
        && (flags.dumps.dump_aether != AetherDumpMode::None
            || !matches!(flags.dumps.dump_core, CoreDumpMode::None)
            || flags.dumps.dump_cfg
            || flags.dumps.dump_lir
            || flags.dumps.dump_lir_llvm)
}

/// Concatenates module programs in topological order for dump-only surfaces.
fn merge_programs<'a>(programs: impl IntoIterator<Item = &'a Program>) -> Program {
    let mut merged = Program::new();
    for program in programs {
        merged.statements.extend(program.statements.clone());
    }
    merged
}

/// Returns whether the cached parallel VM fast-path is eligible for this run.
fn should_try_parallel_vm_fast_path(flags: &DriverFlags, is_multimodule: bool) -> bool {
    is_multimodule && flags.allow_vm_cache() && !flags.cache.no_cache
}

/// Returns whether the compiled run should dispatch to the native backend.
#[cfg_attr(not(feature = "llvm"), allow(dead_code))]
fn should_dispatch_native_backend(flags: &DriverFlags) -> bool {
    #[cfg(feature = "llvm")]
    {
        flags.backend.selected == Backend::Native
    }

    #[cfg(not(feature = "llvm"))]
    {
        let _ = flags;
        false
    }
}

fn has_error_diagnostics(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

/// Builds the initial run context after frontend parsing and compiler setup.
fn prepare_run_context(request: RunProgramRequest<'_>) -> Result<RunContext, String> {
    let ProgramContext {
        source,
        program,
        graph_result,
        entry_has_errors,
        parse_ms,
        all_diagnostics,
        entry_path,
        cache_layout,
    } = build_program_context(
        request.path,
        &request.session.roots,
        request.session.roots_only,
        request.session.cache_dir_path(),
        request.flags.runtime.trace_aether,
        request.flags.is_native_backend(),
    )?;

    let strict_hash =
        hash_bytes(format!("strict={}\n", u8::from(request.session.strict_mode)).as_bytes());
    let module_count = graph_result.graph.module_count();
    let is_multimodule = module_count > 1;

    #[cfg(feature = "llvm")]
    if should_prewarm_toolchain(request.flags) {
        let _ = toolchain_info();
    }

    let compile_start = Instant::now();
    let mut compiler = Compiler::new_with_interner(request.path, graph_result.interner);
    compiler.set_strict_mode(request.session.strict_mode);
    if request.flags.runtime.profiling {
        compiler.set_profiling(true);
    }

    Ok(RunContext {
        source,
        program,
        graph: graph_result.graph,
        failed_modules: graph_result.failed_modules,
        entry_path,
        cache_layout,
        compiler,
        all_diagnostics,
        parse_ms,
        compile_start,
        module_count,
        is_multimodule,
        entry_has_errors,
        strict_hash,
    })
}

/// Attempts the cached parallel VM execution path for eligible multimodule runs.
fn try_run_parallel_vm_fast_path(ctx: &mut RunContext, request: RunProgramRequest<'_>) -> bool {
    if !should_try_parallel_vm_fast_path(request.flags, ctx.is_multimodule) {
        return false;
    }

    let entry_canonical = std::fs::canonicalize(&ctx.entry_path).ok();
    try_run_parallel_vm(ParallelVmRunRequest {
        graph: &ctx.graph,
        entry_canonical: entry_canonical.as_ref(),
        graph_interner: &ctx.compiler.interner,
        cache: DriverCacheConfig::new(&ctx.cache_layout, request.flags.cache.no_cache),
        compile: DriverCompileConfig::from(request.session),
        diagnostics: DriverDiagnosticConfig::from(request.session),
        runtime: DriverRuntimeConfig::from(request.flags),
        flags: request.flags,
        all_diagnostics: &mut ctx.all_diagnostics,
        path: request.path,
        source: ctx.source.as_str(),
        parse_ms: ctx.parse_ms,
        compile_start: ctx.compile_start,
        module_count: ctx.module_count,
    })
}

/// Runs the standard module compilation pipeline into the shared compiler state.
fn compile_modules_for_run(ctx: &mut RunContext, request: RunProgramRequest<'_>) {
    compile_modules(CompileModulesRequest {
        graph: &ctx.graph,
        entry_path: &ctx.entry_path,
        failed_modules: &ctx.failed_modules,
        compiler: &mut ctx.compiler,
        cache: DriverCacheConfig::new(&ctx.cache_layout, request.flags.cache.no_cache),
        compile: DriverCompileConfig::from(request.session),
        runtime: DriverRuntimeConfig::from(request.flags),
        allow_cached_module_bytecode: request.flags.allow_vm_cache(),
        backend: request.flags.backend.selected,
        strict_hash: ctx.strict_hash,
        entry_has_errors: ctx.entry_has_errors,
        all_diagnostics: &mut ctx.all_diagnostics,
    });
}

/// Emits compile diagnostics and exits when any error diagnostics are present.
fn emit_compile_diagnostics_or_exit(ctx: &RunContext, request: RunProgramRequest<'_>) {
    emit_diagnostics_or_exit(
        &ctx.all_diagnostics,
        request.path,
        ctx.source.as_str(),
        ctx.is_multimodule,
        DriverDiagnosticConfig::from(request.session),
    );
}

/// Builds the program value passed to dump surfaces, merging modules only when needed.
fn build_dump_program(ctx: &RunContext, flags: &DriverFlags) -> Program {
    if should_build_merged_program(flags, ctx.is_multimodule) {
        merge_programs(ctx.graph.topo_order().into_iter().map(|node| &node.program))
    } else {
        ctx.program.clone()
    }
}

/// Executes dump requests and returns whether the pipeline should stop afterwards.
fn handle_dump_requests(
    ctx: &mut RunContext,
    request: RunProgramRequest<'_>,
    merged_program: &Program,
) -> bool {
    handle_dumps(DumpRequest {
        compiler: &mut ctx.compiler,
        merged_program,
        path: request.path,
        source: ctx.source.as_str(),
        is_multimodule: ctx.is_multimodule,
        max_errors: request.session.max_errors,
        diagnostics_format: request.session.diagnostics_format,
        all_errors: request.session.all_errors,
        enable_optimize: request.session.enable_optimize,
        dump_aether: request.flags.dumps.dump_aether,
        dump_core: request.flags.dumps.dump_core,
        dump_lir: request.flags.dumps.dump_lir,
        dump_cfg: request.flags.dumps.dump_cfg,
        dump_lir_llvm: request.flags.dumps.dump_lir_llvm,
    })
}

/// Dispatches the compiled program to the selected backend runtime.
fn dispatch_backend(ctx: &mut RunContext, request: RunProgramRequest<'_>) {
    #[cfg(feature = "llvm")]
    if should_dispatch_native_backend(request.flags) {
        run_native_backend(NativeRunRequest {
            program: NativeProgramInput {
                graph: &ctx.graph,
                compiler: &mut ctx.compiler,
                path: request.path,
                source: ctx.source.as_str(),
                is_multimodule: ctx.is_multimodule,
                module_count: ctx.module_count,
                parse_ms: ctx.parse_ms,
                compile_start: ctx.compile_start,
                all_diagnostics: &mut ctx.all_diagnostics,
            },
            cache: DriverCacheConfig::new(&ctx.cache_layout, request.flags.cache.no_cache),
            diagnostics: DriverDiagnosticConfig::from(request.session),
            compile: DriverCompileConfig::from(request.session),
            runtime: DriverRuntimeConfig::from(request.flags),
            output: NativeOutputConfig {
                emit_llvm: request.flags.backend.emit_llvm,
                emit_binary: request.flags.backend.emit_binary,
                output_path: request.flags.backend.output_path.clone(),
            },
            report: NativeReportConfig {
                render_runtime_error: should_render_native_runtime_error(request.flags),
                compile_backend_label: compile_backend_label(request.flags),
                execute_backend_label: execute_backend_label(request.flags),
            },
        });
        return;
    }

    let compile_ms = ctx.compile_start.elapsed().as_secs_f64() * 1000.0;
    run_vm(VmRunRequest {
        compiler: &mut ctx.compiler,
        program: &ctx.program,
        path: request.path,
        source: ctx.source.as_str(),
        is_multimodule: ctx.is_multimodule,
        module_count: ctx.module_count,
        parse_ms: ctx.parse_ms,
        compile_ms,
        flags: request.flags,
        compile: DriverCompileConfig::from(request.session),
        diagnostics: DriverDiagnosticConfig::from(request.session),
        runtime: DriverRuntimeConfig::from(request.flags),
    });
}

/// Runs the end-to-end program pipeline for a single source file.
pub(crate) fn run_file(request: RunProgramRequest<'_>) {
    if request.flags.dumps.dump_repr {
        print_backend_representation_contract();
        return;
    }
    match prepare_run_context(request) {
        Ok(mut ctx) => {
            if has_error_diagnostics(&ctx.all_diagnostics) {
                emit_compile_diagnostics_or_exit(&ctx, request);
            }

            if try_run_parallel_vm_fast_path(&mut ctx, request) {
                return;
            }

            compile_modules_for_run(&mut ctx, request);

            // When the native backend will handle execution, it replays module
            // diagnostics itself and emits them.  Printing warnings here would
            // cause them to appear twice.  We still need to exit on errors
            // before handing off to the native pipeline.
            #[cfg(feature = "llvm")]
            if should_dispatch_native_backend(request.flags) {
                if has_error_diagnostics(&ctx.all_diagnostics) {
                    emit_compile_diagnostics_or_exit(&ctx, request);
                }
                // Clear frontend diagnostics — native backend collects its own.
                ctx.all_diagnostics.clear();
            } else {
                emit_compile_diagnostics_or_exit(&ctx, request);
            }

            #[cfg(not(feature = "llvm"))]
            emit_compile_diagnostics_or_exit(&ctx, request);

            let merged_program = build_dump_program(&ctx, request.flags);
            if handle_dump_requests(&mut ctx, request, &merged_program) {
                return;
            }

            dispatch_backend(&mut ctx, request);
        }
        Err(e) => eprintln!("{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        has_error_diagnostics, merge_programs, should_build_merged_program,
        should_dispatch_native_backend,
        should_try_parallel_vm_fast_path,
    };
    use crate::{
        diagnostics::{Diagnostic, Severity, position::Span},
        driver::{
            backend::Backend,
            mode::{AetherDumpMode, CoreDumpMode},
            test_support::base_flags,
        },
        syntax::{
            program::Program,
            statement::{ImportExposing, Statement},
            symbol::Symbol,
        },
    };

    fn import_statement(symbol: Symbol) -> Statement {
        Statement::Import {
            name: symbol,
            alias: None,
            except: Vec::new(),
            exposing: ImportExposing::None,
            span: Span::default(),
        }
    }

    fn diagnostic_with_severity(severity: Severity) -> Diagnostic {
        let mut diagnostic = Diagnostic::warning("test diagnostic");
        diagnostic.severity = severity;
        diagnostic
    }

    #[test]
    fn has_error_diagnostics_ignores_non_errors() {
        assert!(!has_error_diagnostics(&[]));
        assert!(!has_error_diagnostics(&[diagnostic_with_severity(
            Severity::Warning
        )]));
        assert!(has_error_diagnostics(&[diagnostic_with_severity(
            Severity::Error
        )]));
    }

    #[test]
    fn merged_program_is_only_built_for_multimodule_dump_surfaces() {
        let flags = base_flags();
        assert!(!should_build_merged_program(&flags, false));
        assert!(!should_build_merged_program(&flags, true));

        let mut dump_core_flags = base_flags();
        dump_core_flags.dumps.dump_core = CoreDumpMode::Readable;
        assert!(!should_build_merged_program(&dump_core_flags, false));
        assert!(should_build_merged_program(&dump_core_flags, true));

        let mut dump_aether_flags = base_flags();
        dump_aether_flags.dumps.dump_aether = AetherDumpMode::Summary;
        assert!(should_build_merged_program(&dump_aether_flags, true));

        let mut dump_cfg_flags = base_flags();
        dump_cfg_flags.dumps.dump_cfg = true;
        assert!(should_build_merged_program(&dump_cfg_flags, true));

        let mut dump_lir_flags = base_flags();
        dump_lir_flags.dumps.dump_lir = true;
        assert!(should_build_merged_program(&dump_lir_flags, true));

        let mut dump_lir_llvm_flags = base_flags();
        dump_lir_llvm_flags.dumps.dump_lir_llvm = true;
        assert!(should_build_merged_program(&dump_lir_llvm_flags, true));
    }

    #[test]
    fn parallel_vm_fast_path_requires_multimodule_cacheable_vm_run() {
        let flags = base_flags();
        assert!(!should_try_parallel_vm_fast_path(&flags, false));
        assert!(should_try_parallel_vm_fast_path(&flags, true));

        let mut no_cache_flags = base_flags();
        no_cache_flags.cache.no_cache = true;
        assert!(!should_try_parallel_vm_fast_path(&no_cache_flags, true));
    }

    #[test]
    fn backend_dispatch_defaults_to_vm() {
        let flags = base_flags();

        #[cfg(feature = "llvm")]
        assert!(!should_dispatch_native_backend(&flags));
        #[cfg(not(feature = "llvm"))]
        assert!(!should_dispatch_native_backend(&flags));
    }

    #[test]
    fn backend_dispatch_uses_explicit_native_selection() {
        let mut flags = base_flags();
        flags.backend.selected = Backend::Native;

        #[cfg(feature = "llvm")]
        assert!(should_dispatch_native_backend(&flags));
        #[cfg(not(feature = "llvm"))]
        assert!(!should_dispatch_native_backend(&flags));
    }

    #[test]
    fn backend_dispatch_follows_finalized_native_output_flags() {
        let mut emit_llvm_flags = base_flags();
        emit_llvm_flags.backend.emit_llvm = true;
        let emit_llvm_flags = emit_llvm_flags.finalize_backend();

        let mut emit_binary_flags = base_flags();
        emit_binary_flags.backend.emit_binary = true;
        let emit_binary_flags = emit_binary_flags.finalize_backend();

        #[cfg(feature = "llvm")]
        {
            assert!(should_dispatch_native_backend(&emit_llvm_flags));
            assert!(should_dispatch_native_backend(&emit_binary_flags));
        }
        #[cfg(not(feature = "llvm"))]
        {
            assert!(!should_dispatch_native_backend(&emit_llvm_flags));
            assert!(!should_dispatch_native_backend(&emit_binary_flags));
        }
    }

    #[test]
    fn dump_flags_do_not_change_dispatch_without_backend_selection() {
        let mut flags = base_flags();
        flags.dumps.dump_cfg = true;
        flags.dumps.dump_core = CoreDumpMode::Readable;
        flags.dumps.dump_aether = AetherDumpMode::Summary;

        assert!(!should_dispatch_native_backend(&flags));
    }

    #[test]
    fn merge_programs_preserves_topological_statement_order() {
        let first = Program {
            statements: vec![import_statement(Symbol::SENTINEL)],
            span: Span::default(),
        };
        let second = Program {
            statements: vec![import_statement(Symbol::new(7))],
            span: Span::default(),
        };

        let merged = merge_programs([&first, &second]);

        assert_eq!(merged.statements.len(), 2);
        match &merged.statements[0] {
            Statement::Import { name, .. } => assert_eq!(*name, Symbol::SENTINEL),
            other => panic!("expected import statement, got {other:?}"),
        }
        match &merged.statements[1] {
            Statement::Import { name, .. } => assert_eq!(*name, Symbol::new(7)),
            other => panic!("expected import statement, got {other:?}"),
        }
    }
}
