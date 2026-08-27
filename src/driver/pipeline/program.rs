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
#[cfg(feature = "repl")]
use crate::driver::support::shared::{DiagnosticRenderRequest, emit_diagnostics};
use crate::driver::{
    flags::DriverFlags,
    mode::{AetherDumpMode, CoreDumpMode},
    reporting::report::print_backend_representation_contract,
    run_program::{
        backend::vm::{ParallelVmRunRequest, VmRunRequest, run_vm, try_run_parallel_vm},
        dumps::{DumpRequest, handle_dumps},
        frontend::{ProgramContext, build_program_context, build_program_context_from_source},
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
    compiler::Compiler,
    diagnostics::{Diagnostic, Severity},
    shared::cache_paths::CacheLayout,
    syntax::{module_graph::ModuleGraph, program::Program},
};
// REPL-only imports (the `:type` query + session bootstrap); excluded from native
// builds so the `repl` feature can stay off there.
#[cfg(feature = "repl")]
use flux::{
    ast::type_infer::{display_infer_type, infer_program},
    syntax::{expression::ExprId, interner::Interner, statement::Statement},
    vm::VM,
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

/// Whether this run may write VM cache artifacts.
///
/// `flux build` / `flux check` stop before execution, so they take the serial
/// compile path while a later `flux run` takes the parallel one. The two write
/// different module artifacts, and a run consuming what a build left behind
/// fails with "missing global mapping". Until the two paths share a cache
/// format, a check-only run compiles for its diagnostics and writes nothing.
fn may_write_vm_cache(flags: &DriverFlags) -> bool {
    !flags.runtime.check_only
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
    let context = build_program_context(
        request.path,
        &request.session.roots,
        request.session.roots_only,
        request.session.cache_dir_path(),
        request.flags.runtime.trace_aether,
        request.flags.is_native_backend(),
    )?;
    Ok(finish_run_context(request, context))
}

/// Like [`prepare_run_context`] but builds from in-memory `source` rather than
/// reading `request.path` from disk. Powers [`run_from_source`] (`flux eval`).
fn prepare_run_context_from_source(
    request: RunProgramRequest<'_>,
    source: String,
) -> Result<RunContext, String> {
    let context = build_program_context_from_source(
        request.path,
        source,
        &request.session.roots,
        request.session.roots_only,
        request.session.cache_dir_path(),
        request.flags.runtime.trace_aether,
        request.flags.is_native_backend(),
    )?;
    Ok(finish_run_context(request, context))
}

/// Shared tail of context preparation: turn a parsed [`ProgramContext`] into the
/// pipeline's [`RunContext`] (compiler setup, module counts, strict hash).
fn finish_run_context(request: RunProgramRequest<'_>, context: ProgramContext) -> RunContext {
    let ProgramContext {
        source,
        program,
        graph_result,
        entry_has_errors,
        parse_ms,
        all_diagnostics,
        entry_path,
        cache_layout,
    } = context;

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

    RunContext {
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
    }
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
        cache: DriverCacheConfig::new(
            &ctx.cache_layout,
            request.flags.cache.no_cache || !may_write_vm_cache(request.flags),
        ),
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
        cache: DriverCacheConfig::new(
            &ctx.cache_layout,
            request.flags.cache.no_cache || !may_write_vm_cache(request.flags),
        ),
        compile: DriverCompileConfig::from(request.session),
        runtime: DriverRuntimeConfig::from(request.flags),
        allow_cached_module_bytecode: request.flags.allow_vm_cache(),
        backend: request.flags.backend.selected,
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
            cache: DriverCacheConfig::new(
                &ctx.cache_layout,
                request.flags.cache.no_cache || !may_write_vm_cache(request.flags),
            ),
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

            // The fast path executes the program, so `build` / `check` must
            // not take it.
            if !request.flags.runtime.check_only && try_run_parallel_vm_fast_path(&mut ctx, request)
            {
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

            // `flux build` / `flux check`: every compile-time error has been
            // surfaced by this point, so stop before running `main`.
            if request.flags.runtime.check_only {
                return;
            }

            dispatch_backend(&mut ctx, request);
        }
        // A frontend failure here is an unreadable entry file or a broken
        // module graph. Printing it and exiting 0 would hide the failure from
        // scripts and CI (KI-019).
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

/// Runs an end-to-end program built from in-memory `source` instead of a file on
/// disk. This is the slim path behind `flux eval`: a single synthetic module, VM
/// backend only — no parallel fast-path, dump surfaces, or native dispatch. Parse
/// and type diagnostics still render and exit non-zero through the shared
/// `emit_compile_diagnostics_or_exit`, so a bad expression reports a diagnostic
/// rather than a Rust panic.
pub(crate) fn run_from_source(request: RunProgramRequest<'_>, source: String) {
    match prepare_run_context_from_source(request, source) {
        Ok(mut ctx) => {
            if has_error_diagnostics(&ctx.all_diagnostics) {
                emit_compile_diagnostics_or_exit(&ctx, request);
            }
            compile_modules_for_run(&mut ctx, request);
            emit_compile_diagnostics_or_exit(&ctx, request);
            dispatch_backend(&mut ctx, request);
        }
        // A frontend failure here is an unreadable entry file or a broken
        // module graph. Printing it and exiting 0 would hide the failure from
        // scripts and CI (KI-019).
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

/// The synthetic binding name the REPL wraps a `:type` query in. Shared with the
/// REPL loop, which assembles `let <REPL_TYPE_BINDING> = <expr>` so this side can
/// recover the queried expression's `ExprId` after inference.
#[cfg(feature = "repl")]
pub(crate) const REPL_TYPE_BINDING: &str = "__repl_type";

/// Infer and render the type of a REPL `:type` query **without** running it. The
/// `source` must bind the queried expression to [`REPL_TYPE_BINDING`] inside
/// `main` (the REPL's `assemble` does this). Returns the rendered type on success;
/// on a parse/type error it renders the errors to stderr (like the eval path) and
/// returns `None`.
#[cfg(feature = "repl")]
pub(crate) fn infer_repl_expr_type(
    request: RunProgramRequest<'_>,
    source: String,
) -> Option<String> {
    let mut ctx = match prepare_run_context_from_source(request, source) {
        Ok(ctx) => ctx,
        Err(message) => {
            eprintln!("{message}");
            return None;
        }
    };
    if has_error_diagnostics(&ctx.all_diagnostics) {
        render_repl_errors(&ctx, request);
        return None;
    }
    // Compile the session (the entry module is compiled last) so the prelude and
    // any session imports are preloaded into the compiler. That preloaded state is
    // what makes the re-inference below prelude-aware — the same arrangement the
    // LSP's hover path relies on.
    compile_modules_for_run(&mut ctx, request);
    if has_error_diagnostics(&ctx.all_diagnostics) {
        render_repl_errors(&ctx, request);
        return None;
    }
    // Re-run HM inference directly on the entry program rather than reading the
    // compile pass's residual types: `infer_program` keys `expr_types` by the
    // `ExprId`s in `ctx.program` (the compile pass may infer over a desugared
    // clone), so the queried binding's value can be looked back up.
    let config = ctx.compiler.build_infer_config(&ctx.program);
    let inferred = infer_program(&ctx.program, &ctx.compiler.interner, config);
    let expr_id = repl_type_query_expr_id(&ctx.program, &ctx.compiler.interner)?;
    let ty = inferred.expr_types.get(&expr_id)?;
    Some(display_infer_type(ty, &ctx.compiler.interner))
}

/// Find the `ExprId` of the value bound to [`REPL_TYPE_BINDING`] inside `main`.
#[cfg(feature = "repl")]
fn repl_type_query_expr_id(program: &Program, interner: &Interner) -> Option<ExprId> {
    for stmt in &program.statements {
        let Statement::Function { body, .. } = stmt else {
            continue;
        };
        for inner in &body.statements {
            if let Statement::Let { name, value, .. } = inner
                && interner.try_resolve(*name) == Some(REPL_TYPE_BINDING)
            {
                return Some(value.expr_id());
            }
        }
    }
    None
}

/// Render only the error diagnostics from a failed REPL candidate to stderr.
/// Warnings are intentionally dropped — a REPL session accumulates "unused
/// binding" noise on every line that would otherwise drown the prompt.
#[cfg(feature = "repl")]
fn render_repl_errors(ctx: &RunContext, request: RunProgramRequest<'_>) {
    render_repl_diagnostics(
        &ctx.all_diagnostics,
        request.path,
        ctx.source.as_str(),
        &DriverDiagnosticConfig::from(request.session),
    );
}

/// Render the error diagnostics from a REPL line to stderr (carets included),
/// dropping warnings. Shared by the Phase 1 source path and the Phase 2 engine,
/// which renders the `Err` of an incremental [`Compiler::compile_with_opts`]
/// against the line's wrapped source.
#[cfg(feature = "repl")]
pub(crate) fn render_repl_diagnostics(
    diagnostics: &[Diagnostic],
    path: &str,
    source: &str,
    config: &DriverDiagnosticConfig,
) {
    let errors: Vec<Diagnostic> = diagnostics
        .iter()
        .filter(|diag| diag.severity == Severity::Error)
        .cloned()
        .collect();
    if errors.is_empty() {
        return;
    }
    emit_diagnostics(DiagnosticRenderRequest {
        diagnostics: &errors,
        default_file: Some(path),
        default_source: Some(source),
        show_file_headers: false,
        max_errors: config.max_errors,
        format: config.diagnostics_format,
        all_errors: config.all_errors,
        text_to_stderr: true,
    });
}

/// A prelude-loaded compiler paired with a live VM whose `globals` already hold
/// the prelude's values — the persistent state the Phase 2 REPL engine
/// (proposal 0176) mutates incrementally, one line at a time.
#[cfg(feature = "repl")]
pub(crate) struct ReplBootstrap {
    pub(crate) compiler: Compiler,
    pub(crate) vm: VM,
    /// Optimization flags carried over from the session so per-line compiles
    /// match the level the prelude was compiled at.
    pub(crate) optimize: bool,
    pub(crate) analyze: bool,
    /// Diagnostic-rendering config + synthetic entry path for surfacing per-line
    /// compile errors.
    pub(crate) diagnostics: DriverDiagnosticConfig,
    pub(crate) path: String,
}

/// Build the persistent REPL session: compile a trivial entry through the normal
/// module pipeline (which loads + compiles the Flow prelude into one `Compiler`),
/// then run the resulting bytecode once on a fresh `VM` so the prelude's globals
/// are live. The returned compiler and VM are handed to the engine, which from
/// here compiles each entered line as a delta and runs it via [`VM::run_top_level`]
/// without ever recompiling the prelude or earlier lines.
#[cfg(feature = "repl")]
pub(crate) fn bootstrap_repl_session(
    request: RunProgramRequest<'_>,
) -> Result<ReplBootstrap, String> {
    // A minimal valid entry. Its `main` runs once during bootstrap (a no-op);
    // later expression lines define their own `main`, shadowing this one.
    let mut ctx = prepare_run_context_from_source(request, "fn main() {}\n".to_string())?;
    if has_error_diagnostics(&ctx.all_diagnostics) {
        render_repl_errors(&ctx, request);
        return Err("could not initialize the REPL (prelude failed to parse)".to_string());
    }
    compile_modules_for_run(&mut ctx, request);
    if has_error_diagnostics(&ctx.all_diagnostics) {
        render_repl_errors(&ctx, request);
        return Err("could not initialize the REPL (prelude failed to compile)".to_string());
    }

    let bytecode = ctx.compiler.bytecode();
    let mut vm = VM::new(bytecode);
    if let Err(err) = vm.run() {
        return Err(format!("could not initialize the REPL session: {err}"));
    }

    // Per-line programs are not whole modules and need no `main`.
    ctx.compiler.set_strict_require_main(false);
    // Accumulate each line's binding schemes so later lines can resolve the
    // types of earlier session globals.
    ctx.compiler.set_repl_mode(true);

    let compile = DriverCompileConfig::from(request.session);
    Ok(ReplBootstrap {
        optimize: compile.enable_optimize,
        analyze: compile.enable_analyze,
        diagnostics: DriverDiagnosticConfig::from(request.session),
        path: request.path.to_string(),
        compiler: ctx.compiler,
        vm,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        has_error_diagnostics, merge_programs, should_build_merged_program,
        should_dispatch_native_backend, should_try_parallel_vm_fast_path,
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
            ..Default::default()
        };
        let second = Program {
            statements: vec![import_statement(Symbol::new(7))],
            span: Span::default(),
            ..Default::default()
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
