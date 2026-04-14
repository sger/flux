//! Program execution pipeline orchestration shared by VM and native backends.

use std::{collections::HashSet, path::PathBuf, time::Instant};

use crate as flux;
#[cfg(feature = "core_to_llvm")]
use crate::driver::backend::Backend;
#[cfg(feature = "core_to_llvm")]
use crate::driver::run_program::backend::native::{NativeRunRequest, run_native_backend};
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
    support::shared::emit_diagnostics,
};
use flux::{
    bytecode::{bytecode_cache::hash_bytes, compiler::Compiler},
    cache_paths::CacheLayout,
    diagnostics::{Diagnostic, DiagnosticsAggregator},
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

    let strict_hash = hash_bytes(if request.session.strict_mode {
        b"strict=1"
    } else {
        b"strict=0"
    });
    let module_count = graph_result.graph.module_count();
    let is_multimodule = module_count > 1;

    #[cfg(feature = "core_to_llvm")]
    if crate::driver::backend_policy::should_prewarm_toolchain(request.flags) {
        let _ = flux::core_to_llvm::pipeline::toolchain_info();
    }

    let compile_start = Instant::now();
    let mut compiler = Compiler::new_with_interner(request.path, graph_result.interner);
    compiler.set_strict_mode(request.session.strict_mode);
    compiler.set_strict_types(request.session.strict_types);
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
        cache_layout: &ctx.cache_layout,
        flags: request.flags,
        session: request.session,
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
        cache_layout: &ctx.cache_layout,
        no_cache: request.flags.cache.no_cache,
        strict_mode: request.session.strict_mode,
        strict_types: request.session.strict_types,
        enable_optimize: request.session.enable_optimize,
        enable_analyze: request.session.enable_analyze,
        verbose: request.flags.runtime.verbose,
        allow_cached_module_bytecode: request.flags.allow_vm_cache(),
        backend: request.flags.backend.selected,
        strict_hash: ctx.strict_hash,
        entry_has_errors: ctx.entry_has_errors,
        all_diagnostics: &mut ctx.all_diagnostics,
    });
}

/// Emits compile diagnostics and exits when any error diagnostics are present.
fn emit_compile_diagnostics_or_exit(ctx: &RunContext, request: RunProgramRequest<'_>) {
    if ctx.all_diagnostics.is_empty() {
        return;
    }

    let report = DiagnosticsAggregator::new(&ctx.all_diagnostics)
        .with_default_source(request.path, ctx.source.as_str())
        .with_file_headers(ctx.is_multimodule)
        .with_max_errors(Some(request.session.max_errors))
        .with_stage_filtering(!request.session.all_errors)
        .report();
    if report.counts.errors > 0 {
        emit_diagnostics(
            &ctx.all_diagnostics,
            Some(request.path),
            Some(ctx.source.as_str()),
            ctx.is_multimodule,
            request.session.max_errors,
            request.session.diagnostics_format,
            request.session.all_errors,
            true,
        );
        std::process::exit(1);
    }
    emit_diagnostics(
        &ctx.all_diagnostics,
        Some(request.path),
        Some(ctx.source.as_str()),
        ctx.is_multimodule,
        request.session.max_errors,
        request.session.diagnostics_format,
        request.session.all_errors,
        true,
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
    #[cfg(feature = "core_to_llvm")]
    if request.flags.backend.selected == Backend::Native {
        run_native_backend(NativeRunRequest {
            graph: &ctx.graph,
            compiler: &mut ctx.compiler,
            cache_layout: &ctx.cache_layout,
            path: request.path,
            source: ctx.source.as_str(),
            max_errors: request.session.max_errors,
            diagnostics_format: request.session.diagnostics_format,
            all_errors: request.session.all_errors,
            is_multimodule: ctx.is_multimodule,
            module_count: ctx.module_count,
            parse_ms: ctx.parse_ms,
            compile_start: ctx.compile_start,
            strict_mode: request.session.strict_mode,
            strict_types: request.session.strict_types,
            enable_optimize: request.session.enable_optimize,
            enable_analyze: request.session.enable_analyze,
            verbose: request.flags.runtime.verbose,
            show_stats: request.flags.runtime.show_stats,
            no_cache: request.flags.cache.no_cache,
            emit_llvm: request.flags.backend.emit_llvm,
            emit_binary: request.flags.backend.emit_binary,
            output_path: request.flags.backend.output_path.clone(),
            render_runtime_error: crate::driver::backend_policy::should_render_native_runtime_error(
                request.flags,
            ),
            compile_backend_label: crate::driver::backend_policy::compile_backend_label(
                request.flags,
            ),
            execute_backend_label: crate::driver::backend_policy::execute_backend_label(
                request.flags,
            ),
            all_diagnostics: &mut ctx.all_diagnostics,
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
        session: request.session,
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
            if try_run_parallel_vm_fast_path(&mut ctx, request) {
                return;
            }

            compile_modules_for_run(&mut ctx, request);
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
    use super::{merge_programs, should_build_merged_program, should_try_parallel_vm_fast_path};
    use crate::{
        diagnostics::position::Span,
        driver::{
            backend::Backend,
            flags::{
                DriverBackendFlags, DriverCacheFlags, DriverDiagnosticFlags, DriverDumpFlags,
                DriverFlags, DriverInputFlags, DriverLanguageFlags, DriverRuntimeFlags,
            },
            mode::{AetherDumpMode, CoreDumpMode, DiagnosticOutputFormat},
        },
        syntax::{
            program::Program,
            statement::{ImportExposing, Statement},
            symbol::Symbol,
        },
    };

    fn make_flags() -> DriverFlags {
        DriverFlags {
            backend: DriverBackendFlags {
                selected: Backend::Vm,
                use_core_to_llvm: false,
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
                max_errors: 20,
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
                strict_types: false,
            },
        }
    }

    fn import_statement(symbol: Symbol) -> Statement {
        Statement::Import {
            name: symbol,
            alias: None,
            except: Vec::new(),
            exposing: ImportExposing::None,
            span: Span::default(),
        }
    }

    #[test]
    fn merged_program_is_only_built_for_multimodule_dump_surfaces() {
        let flags = make_flags();
        assert!(!should_build_merged_program(&flags, false));
        assert!(!should_build_merged_program(&flags, true));

        let mut dump_core_flags = make_flags();
        dump_core_flags.dumps.dump_core = CoreDumpMode::Readable;
        assert!(!should_build_merged_program(&dump_core_flags, false));
        assert!(should_build_merged_program(&dump_core_flags, true));

        let mut dump_aether_flags = make_flags();
        dump_aether_flags.dumps.dump_aether = AetherDumpMode::Summary;
        assert!(should_build_merged_program(&dump_aether_flags, true));

        let mut dump_cfg_flags = make_flags();
        dump_cfg_flags.dumps.dump_cfg = true;
        assert!(should_build_merged_program(&dump_cfg_flags, true));

        let mut dump_lir_flags = make_flags();
        dump_lir_flags.dumps.dump_lir = true;
        assert!(should_build_merged_program(&dump_lir_flags, true));

        let mut dump_lir_llvm_flags = make_flags();
        dump_lir_llvm_flags.dumps.dump_lir_llvm = true;
        assert!(should_build_merged_program(&dump_lir_llvm_flags, true));
    }

    #[test]
    fn parallel_vm_fast_path_requires_multimodule_cacheable_vm_run() {
        let flags = make_flags();
        assert!(!should_try_parallel_vm_fast_path(&flags, false));
        assert!(should_try_parallel_vm_fast_path(&flags, true));

        let mut no_cache_flags = make_flags();
        no_cache_flags.cache.no_cache = true;
        assert!(!should_try_parallel_vm_fast_path(&no_cache_flags, true));
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
