use std::path::PathBuf;
use std::time::Instant;

use crate as flux;
use crate::driver::shared::{
    DriverCacheConfig, DriverCompileConfig, DriverDiagnosticConfig, DriverRuntimeConfig,
};
#[cfg(feature = "llvm")]
use crate::driver::{
    backend_policy::{
        native_binary_up_to_date_banner, native_lir_lowering_banner, native_module_lowering_banner,
    },
    pipeline::native::{
        NativeParallelCompileRequest, compile_native_modules_parallel,
        compile_native_support_object, compile_program_hash_object, locate_runtime_lib_dir,
    },
    pipeline::program::merge_programs,
    reporting::{
        report::{
            AetherTraceContext, ArtifactStats, CompileStats, ExecuteStats, Phase, RunStats,
            TraceBackend, print_aether_trace, print_stats, write_stats_json,
        },
        runtime_errors::render_native_runtime_error,
    },
    shared::emit_diagnostics_or_exit,
    support::shared::{DiagnosticRenderRequest, emit_diagnostics},
};
#[cfg(feature = "llvm")]
use flux::syntax::{module_graph::ModuleKind, program::Program};
use flux::{diagnostics::Diagnostic, syntax::module_graph::ModuleGraph};
#[cfg(feature = "llvm")]
use std::collections::HashSet;
#[cfg(all(feature = "llvm", unix))]
use std::os::unix::process::ExitStatusExt;

#[cfg(feature = "llvm")]
/// Returns whether the cached native binary can be reused without relinking.
fn should_skip_native_relink(
    no_cache: bool,
    emit_binary: bool,
    any_native_recompiled: bool,
    binary_exists: bool,
) -> bool {
    !no_cache && !emit_binary && !any_native_recompiled && binary_exists
}

#[cfg(feature = "llvm")]
/// Formats a native child-process termination when no Flux runtime panic text is available.
fn native_exit_summary(status: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        if let Some(signal) = status.signal() {
            let (name, reason) = match signal {
                4 => ("SIGILL", "illegal instruction"),
                6 => ("SIGABRT", "process aborted"),
                8 => ("SIGFPE", "arithmetic exception"),
                9 => ("SIGKILL", "process killed"),
                11 => ("SIGSEGV", "invalid memory access"),
                13 => ("SIGPIPE", "broken pipe"),
                14 => ("SIGALRM", "alarm timer expired"),
                15 => ("SIGTERM", "termination requested"),
                _ => ("signal", "native program crashed"),
            };
            return format!("native program terminated by signal {signal} ({name}): {reason}");
        }
    }

    match status.code() {
        Some(code) => format!("native program exited with status {code}"),
        None => "native program terminated without an exit code".to_string(),
    }
}

#[cfg(feature = "llvm")]
/// Renders the native Aether trace header and ownership report, exiting on diagnostics.
fn emit_native_aether_trace(
    input: &mut NativeProgramInput<'_>,
    program: &Program,
    cache: DriverCacheConfig<'_>,
    compile: DriverCompileConfig,
    diagnostics: DriverDiagnosticConfig,
) {
    input.compiler.infer_expr_types_for_program(program);
    match input.compiler.render_aether_report(program, compile.enable_optimize, false) {
        Ok(report) => print_aether_trace(
            AetherTraceContext {
                path: input.path,
                backend: TraceBackend::Native,
                pipeline: "AST -> Core -> LIR -> LLVM -> native",
                cache: Some(if cache.no_cache { "disabled" } else { "enabled" }),
                optimize: compile.enable_optimize,
                analyze: compile.enable_analyze,
                strict: compile.strict_mode,
                module_count: Some(input.module_count),
            },
            &report,
        ),
        Err(diag) => emit_diagnostics_or_exit(
            &[diag],
            input.path,
            input.source,
            input.is_multimodule,
            diagnostics,
        ),
    }
}

#[cfg_attr(not(feature = "llvm"), allow(dead_code))]
/// Frontend state already prepared by the pipeline before native execution starts.
pub(crate) struct NativeProgramInput<'a> {
    pub(crate) graph: &'a ModuleGraph,
    pub(crate) compiler: &'a mut flux::compiler::Compiler,
    pub(crate) path: &'a str,
    pub(crate) source: &'a str,
    pub(crate) is_multimodule: bool,
    pub(crate) module_count: usize,
    pub(crate) parse_ms: f64,
    pub(crate) compile_start: Instant,
    pub(crate) all_diagnostics: &'a mut Vec<Diagnostic>,
}

#[cfg_attr(not(feature = "llvm"), allow(dead_code))]
#[derive(Debug, Clone)]
/// Output-target settings for the native backend.
pub(crate) struct NativeOutputConfig {
    pub(crate) emit_llvm: bool,
    pub(crate) emit_binary: bool,
    /// Link `libflux_rt_debug.a` (0188 S10c).
    pub(crate) debug_heap: bool,
    /// Ask the debug heap to report blocks live at exit (0190 S15).
    pub(crate) leak_check: bool,
    pub(crate) output_path: Option<String>,
    pub(crate) stats_json_path: Option<PathBuf>,
}

#[cfg_attr(not(feature = "llvm"), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
/// Driver reporting labels and runtime error rendering settings for native execution.
pub(crate) struct NativeReportConfig {
    pub(crate) render_runtime_error: bool,
    pub(crate) compile_backend_label: &'static str,
    pub(crate) execute_backend_label: &'static str,
}

#[cfg_attr(not(feature = "llvm"), allow(dead_code))]
/// Grouped request consumed by the native backend coordinator.
pub(crate) struct NativeRunRequest<'a> {
    pub(crate) program: NativeProgramInput<'a>,
    pub(crate) cache: DriverCacheConfig<'a>,
    pub(crate) diagnostics: DriverDiagnosticConfig,
    pub(crate) compile: DriverCompileConfig,
    pub(crate) runtime: DriverRuntimeConfig,
    pub(crate) output: NativeOutputConfig,
    pub(crate) report: NativeReportConfig,
}

#[cfg(feature = "llvm")]
/// Resolves the binary or temporary output path used by the native backend.
fn native_output_path(request: &NativeRunRequest<'_>) -> PathBuf {
    request.output.output_path.as_ref().map(PathBuf::from).unwrap_or_else(|| {
        if request.output.emit_binary {
            PathBuf::from(request.program.path.strip_suffix(".flx").unwrap_or(request.program.path))
        } else if !request.cache.no_cache {
            let bin_name = std::path::Path::new(request.program.path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("program");
            // A debug-heap binary links another runtime: a plain run
            // must not reuse it, nor the other way round (0188 S10c).
            let kind =
                match flux::llvm::pipeline::RuntimeVariant::from_env(request.output.debug_heap) {
                    flux::llvm::pipeline::RuntimeVariant::DebugHeap => ".debug-heap",
                    flux::llvm::pipeline::RuntimeVariant::Sanitize => ".sanitize",
                    flux::llvm::pipeline::RuntimeVariant::Tsan => ".tsan",
                    flux::llvm::pipeline::RuntimeVariant::Plain => "",
                };
            request.cache.cache_layout.native_dir().join(format!("{bin_name}{kind}.bin"))
        } else {
            native_temp_dir(request.cache.cache_layout).join("program")
        }
    })
}

#[cfg(feature = "llvm")]
/// Whether the program's debug heap was asked for its counts
/// (`FLUX_HEAP_STATS`, read by the runtime the same way).
fn heap_stats_wanted() -> bool {
    std::env::var("FLUX_HEAP_STATS").is_ok_and(|value| !value.is_empty() && value != "0")
}

#[cfg(feature = "llvm")]
/// Counts source lines across every module in the execution graph for analytics reporting.
fn total_graph_source_lines(graph: &ModuleGraph) -> usize {
    graph
        .topo_order()
        .iter()
        .map(|node| std::fs::read_to_string(&node.path).map(|s| s.lines().count()).unwrap_or(0))
        .sum()
}

#[cfg_attr(not(feature = "llvm"), allow(dead_code))]
/// Runs the native backend from already-prepared frontend and pipeline state.
pub(crate) fn run_native_backend(request: NativeRunRequest<'_>) {
    #[cfg(feature = "llvm")]
    {
        let mut request = request;
        let native_program = if request.runtime.trace_aether || request.output.emit_llvm {
            Some(merge_programs(
                request.program.graph.topo_order().into_iter().map(|node| &node.program),
            ))
        } else {
            None
        };

        if request.output.emit_llvm {
            let native_program =
                native_program.as_ref().expect("native program should exist for LLVM emission");
            if request.runtime.trace_aether {
                emit_native_aether_trace(
                    &mut request.program,
                    native_program,
                    request.cache,
                    request.compile,
                    request.diagnostics,
                );
            } else {
                request.program.compiler.infer_expr_types_for_program(native_program);
            }
            eprintln!("{}", native_lir_lowering_banner());
            let mut llvm_module = match request
                .program
                .compiler
                .lower_to_lir_llvm_module(native_program, request.compile.enable_optimize)
            {
                Ok(m) => m,
                Err(diag) => {
                    emit_diagnostics(DiagnosticRenderRequest {
                        diagnostics: &[diag],
                        default_file: Some(request.program.path),
                        default_source: Some(request.program.source),
                        show_file_headers: request.program.is_multimodule,
                        max_errors: request.diagnostics.max_errors,
                        format: request.diagnostics.diagnostics_format,
                        all_errors: request.diagnostics.all_errors,
                        text_to_stderr: true,
                    });
                    std::process::exit(1);
                }
            };
            llvm_module.target_triple = Some(flux::llvm::target::host_triple());
            llvm_module.data_layout = flux::llvm::target::host_data_layout();
            let ll_text = flux::llvm::render_module(&llvm_module);
            if let Some(ref out) = request.output.output_path {
                if let Err(e) = std::fs::write(out, &ll_text) {
                    eprintln!("Failed to write LLVM IR: {e}");
                    std::process::exit(1);
                }
                println!("Emitted LLVM IR: {out}");
            } else {
                println!("{ll_text}");
            }
            return;
        }

        let frontend_ms = request.program.compile_start.elapsed().as_secs_f64() * 1000.0;
        let runtime = flux::llvm::pipeline::RuntimeVariant::from_env(request.output.debug_heap);
        let runtime_lib_dir = locate_runtime_lib_dir(runtime);
        if request.runtime.verbose {
            eprintln!("[lir→llvm] toolchain: {}", flux::llvm::pipeline::toolchain_info());
        }
        eprintln!("{}", native_module_lowering_banner());
        let native_modules_start = Instant::now();
        let (mut object_paths, any_native_recompiled) = match compile_native_modules_parallel(
            NativeParallelCompileRequest {
                graph: request.program.graph,
                cache_layout: request.cache.cache_layout,
                no_cache: request.cache.no_cache,
                strict_mode: request.compile.strict_mode,
                enable_optimize: request.compile.enable_optimize,
                enable_analyze: request.compile.enable_analyze,
                debug_level: request.compile.debug_level,
                verbose: request.runtime.verbose,
                base_interner: &request.program.compiler.interner,
            },
            request.program.all_diagnostics,
        ) {
            Ok(paths) => paths,
            Err(err) => {
                emit_diagnostics(DiagnosticRenderRequest {
                    diagnostics: request.program.all_diagnostics,
                    default_file: Some(request.program.path),
                    default_source: Some(request.program.source),
                    show_file_headers: request.program.is_multimodule,
                    max_errors: request.diagnostics.max_errors,
                    format: request.diagnostics.diagnostics_format,
                    all_errors: request.diagnostics.all_errors,
                    text_to_stderr: true,
                });
                eprintln!("llvm module pipeline failed: {err}");
                std::process::exit(1);
            }
        };
        let native_modules_ms = native_modules_start.elapsed().as_secs_f64() * 1000.0;
        let support_start = Instant::now();
        match compile_native_support_object(
            request.cache.cache_layout,
            request.cache.no_cache,
            request.compile.enable_optimize,
        ) {
            Ok(support_object) => {
                object_paths.insert(0, support_object);
            }
            Err(err) => {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        let support_ms = support_start.elapsed().as_secs_f64() * 1000.0;

        let archive_start = Instant::now();
        let flow_object_paths: HashSet<PathBuf> = request
            .program
            .graph
            .topo_order()
            .iter()
            .filter(|node| node.kind == ModuleKind::FlowStdlib)
            .filter_map(|node| {
                let module_stem = node.path.file_stem()?.to_str()?;
                object_paths
                    .iter()
                    .find(|obj| {
                        obj.file_name()
                            .and_then(|f| f.to_str())
                            .is_some_and(|f| f.starts_with(&format!("{module_stem}-")))
                    })
                    .cloned()
            })
            .collect();
        let std_lib_objects: Vec<PathBuf> =
            object_paths.iter().filter(|p| flow_object_paths.contains(*p)).cloned().collect();

        let link_paths = if std_lib_objects.len() >= 2 && !request.cache.no_cache {
            let archive_name = if request.compile.enable_optimize {
                "libflux_std_O2.a"
            } else {
                "libflux_std_O0.a"
            };
            let archive_path = request.cache.cache_layout.native_dir().join(archive_name);
            let need_rebuild =
                !flux::llvm::pipeline::archive_is_up_to_date(&std_lib_objects, &archive_path);
            if need_rebuild {
                if let Err(err) =
                    flux::llvm::pipeline::create_archive(&std_lib_objects, &archive_path)
                {
                    eprintln!(
                        "warning: failed to create libflux_std.a: {err}, falling back to individual .o files"
                    );
                    object_paths.clone()
                } else {
                    let std_set: HashSet<PathBuf> = std_lib_objects.iter().cloned().collect();
                    let mut paths: Vec<PathBuf> =
                        object_paths.iter().filter(|p| !std_set.contains(*p)).cloned().collect();
                    paths.push(archive_path);
                    paths
                }
            } else {
                let std_set: HashSet<PathBuf> = std_lib_objects.iter().cloned().collect();
                let mut paths: Vec<PathBuf> =
                    object_paths.iter().filter(|p| !std_set.contains(*p)).cloned().collect();
                paths.push(archive_path);
                paths
            }
        } else {
            object_paths.clone()
        };
        // The hash a tape names this program by, so the program records and
        // replays on its own too: under a debugger, or from an editor.
        let mut link_paths = link_paths;
        let program_hash = crate::driver::replay_session::native_program_hash(
            request.program.graph,
            std::path::Path::new(request.program.path),
        );
        let hash_object_built = match compile_program_hash_object(
            request.cache.cache_layout,
            request.cache.no_cache,
            &program_hash,
        ) {
            Ok((object, built)) => {
                link_paths.push(object);
                built
            }
            Err(err) => {
                eprintln!("{err}");
                std::process::exit(1);
            }
        };
        let archive_ms = archive_start.elapsed().as_secs_f64() * 1000.0;

        emit_diagnostics_or_exit(
            request.program.all_diagnostics,
            request.program.path,
            request.program.source,
            true,
            request.diagnostics,
        );

        if request.runtime.trace_aether {
            let native_program =
                native_program.as_ref().expect("native program should exist for Aether tracing");
            emit_native_aether_trace(
                &mut request.program,
                native_program,
                request.cache,
                request.compile,
                request.diagnostics,
            );
        }

        let out = native_output_path(&request);
        // The uncached scratch path now lives under the project build tree, so make
        // sure its directory exists before the linker writes the binary.
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let link_start = Instant::now();
        // A run with a tape reuses the binary: it reads the tape at run time,
        // and every binary in the cache was linked by a compiler that writes
        // tapes (an older one's objects are recompiled at the next epoch). A
        // new program hash relinks, since the binary names its program by
        // it. A `-g` binary on Windows also needs its PDB (0188 S11). A
        // debug-heap run always relinks, so it never runs with a debug
        // runtime older than the sources (0188 S10c).
        let debug = request.compile.debug_level.is_enabled();
        let pdb_ready = !(debug && cfg!(windows)) || flux::llvm::pipeline::pdb_path(&out).exists();
        let binary_exists = out.exists() && pdb_ready;
        let binary_up_to_date = should_skip_native_relink(
            request.cache.no_cache,
            request.output.emit_binary,
            any_native_recompiled || hash_object_built,
            binary_exists,
        ) && runtime == flux::llvm::pipeline::RuntimeVariant::Plain;

        if binary_up_to_date {
            if request.runtime.verbose {
                eprintln!("{}", native_binary_up_to_date_banner());
            }
        } else if let Err(e) = flux::llvm::pipeline::link_objects(
            &link_paths,
            &out,
            runtime_lib_dir.as_deref(),
            runtime,
            debug,
        ) {
            eprintln!("llvm linker failed: {e}");
            std::process::exit(1);
        } else if debug && (request.output.emit_binary || request.output.output_path.is_some()) {
            // A binary the user keeps carries its debug information with
            // it: on macOS that means a .dSYM, since otherwise it lives in
            // object files the cache may remove (proposal 0188 S5).
            flux::llvm::pipeline::write_debug_symbols(&out);
        }
        let link_ms = link_start.elapsed().as_secs_f64() * 1000.0;
        if request.runtime.verbose {
            eprintln!(
                "[lir→llvm] frontend: {frontend_ms:.1}ms, modules: {native_modules_ms:.1}ms, support: {support_ms:.1}ms, archive: {archive_ms:.1}ms, link: {link_ms:.1}ms"
            );
        }

        if request.output.emit_binary {
            println!("Emitted binary: {}", out.display());
            return;
        }

        let exec_start = Instant::now();
        let fiber_trace = std::env::var("FLUX_FIBER_TRACE").ok();
        // `-o app` names a file here, not a command to look up on PATH.
        let runnable =
            if out.is_relative() { std::path::Path::new(".").join(&out) } else { out.clone() };
        let mut native_cmd = std::process::Command::new(&runnable);
        // Forward the program's own arguments (everything the driver saw
        // after `--`). The child is a separate process, so argv[0] would
        // otherwise be the cache path of the compiled binary; arg0 is
        // overridden to the script path so `Env.args()` reads the same on
        // both backends.
        let argv = crate::vm::program_args_snapshot();
        if let Some((script, rest)) = argv.split_first() {
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt as _;
                native_cmd.arg0(script);
            }
            // `arg0` is a Unix extension. Windows fixes the child's argv[0] at
            // spawn, so the override above cannot run there and the script path
            // has no consumer — which also means `Env.args()` still reads the
            // cache path on the native backend on Windows, where the VM reads
            // the script. Binding it away keeps the gate's `-D warnings` clean
            // without hiding that the divergence is unaddressed.
            #[cfg(not(unix))]
            let _ = script;
            native_cmd.args(rest);
        }
        native_cmd.stdout(std::process::Stdio::inherit());
        if fiber_trace.as_deref() == Some("1") {
            // Inherit stderr so trace events written by the child binary appear
            // directly on the terminal. Error output will also be visible but
            // cannot be captured for render_runtime_error post-processing.
            native_cmd.stderr(std::process::Stdio::inherit());
        } else {
            native_cmd.stderr(std::process::Stdio::piped());
        }
        if let Some(val) = fiber_trace {
            native_cmd.env("FLUX_FIBER_TRACE", val);
        }
        if let Some((key, tape)) = crate::driver::replay_session::native_child_env() {
            native_cmd.env(key, tape);
            // This driver renders the program's tape errors itself.
            native_cmd.env(flux::runtime::replay::native_abi::WIRE_ENV, "1");
        }
        // Read by the debug heap at exit: one runtime build serves both
        // `--debug-heap` and `--leak-check`.
        if request.output.leak_check {
            native_cmd.env("FLUX_LEAK_CHECK", "1");
        } else {
            native_cmd.env_remove("FLUX_LEAK_CHECK");
        }
        match native_cmd.spawn() {
            Ok(child) => {
                let output = match child.wait_with_output() {
                    Ok(output) => output,
                    Err(e) => {
                        eprintln!("failed to wait for native program {}: {e}", out.display());
                        return;
                    }
                };

                let exit_code = output.status.code().unwrap_or(1);
                let execute_ms = exec_start.elapsed().as_secs_f64() * 1000.0;
                let child_stderr = String::from_utf8_lossy(&output.stderr);
                crate::driver::replay_session::check_native_recording();
                if exit_code != 0
                    && let Some(tape_error) =
                        flux::runtime::replay::TapeError::from_stderr(&child_stderr)
                {
                    // The tape stopped the program: everything else it wrote to
                    // stderr first, then the tape's diagnostic.
                    for line in child_stderr.lines() {
                        if !line.starts_with(flux::runtime::replay::error::NATIVE_WIRE_PREFIX) {
                            eprintln!("{line}");
                        }
                    }
                    crate::driver::replay_session::report(&tape_error, request.program.path);
                    std::process::exit(exit_code);
                }
                // A program that ends normally has its stderr dropped; the
                // debug heap's counts are the one line asked for by name.
                if exit_code == 0 && heap_stats_wanted() {
                    for line in child_stderr.lines().filter(|line| line.starts_with("heap: ")) {
                        eprintln!("{line}");
                    }
                }
                if exit_code != 0 {
                    if let Some(rendered) = if request.report.render_runtime_error {
                        render_native_runtime_error(request.program.path, &child_stderr)
                    } else {
                        None
                    } {
                        eprint!("{rendered}");
                    } else if !child_stderr.is_empty() {
                        eprint!("{child_stderr}");
                    } else {
                        eprintln!("{}", native_exit_summary(&output.status));
                    }
                }
                if request.runtime.show_stats || request.output.stats_json_path.is_some() {
                    let compile_ms =
                        request.program.compile_start.elapsed().as_secs_f64() * 1000.0 - execute_ms;
                    let stats = RunStats {
                        compile: CompileStats {
                            parse_ms: Some(request.program.parse_ms),
                            compile_ms: Some(compile_ms),
                            compile_backend: Some(request.report.compile_backend_label),
                            cached: false,
                            module_count: Some(request.program.module_count),
                            cached_module_count: None,
                            compiled_module_count: None,
                        },
                        execute: ExecuteStats {
                            execute_ms,
                            execute_backend: request.report.execute_backend_label,
                        },
                        artifacts: ArtifactStats {
                            source_lines: total_graph_source_lines(request.program.graph),
                            globals_count: None,
                            functions_count: None,
                            instruction_bytes: None,
                        },
                    };
                    if request.runtime.show_stats {
                        print_stats(&stats);
                    }
                    // The steps timed above for `--verbose`, in place of one
                    // compile time, so a diff shows which of them moved.
                    if let Some(path) = &request.output.stats_json_path {
                        let phases = [
                            Phase { name: "parse", ms: request.program.parse_ms },
                            Phase { name: "frontend", ms: frontend_ms },
                            Phase { name: "llvm_modules", ms: native_modules_ms },
                            Phase { name: "support", ms: support_ms },
                            Phase { name: "archive", ms: archive_ms },
                            Phase { name: "link", ms: link_ms },
                            Phase { name: "execute", ms: execute_ms },
                        ];
                        write_stats_json(path, request.program.path, &stats, &phases);
                    }
                }
                // A binary written with `-o` is the user's to keep. On Windows
                // the linker wrote `out` with `.exe` added, and a PDB beside it.
                if request.output.output_path.is_none()
                    && (request.cache.no_cache || request.output.emit_binary)
                {
                    let _ = std::fs::remove_file(flux::llvm::pipeline::linked_exe_path(&out));
                    let _ = std::fs::remove_file(flux::llvm::pipeline::pdb_path(&out));
                }
                if exit_code != 0 {
                    std::process::exit(exit_code);
                }
            }
            Err(e) => {
                eprintln!("llvm execution failed: {e}");
                std::process::exit(1);
            }
        }
    }

    #[cfg(not(feature = "llvm"))]
    {
        let _ = request;
    }
}

#[cfg(feature = "llvm")]
/// A unique scratch output directory for uncached native runs, **inside the project
/// build tree** (`<cache_root>/native/scratch/…`) rather than the system temp dir.
/// A freshly-built, unsigned executable launched from `%TEMP%` is a heavily-weighted
/// malware-dropper heuristic that makes Windows Defender flag native builds
/// (`Trojan:Win32/Wacatac.B!ml`); building under the project avoids that signal.
fn native_temp_dir(cache_layout: &crate::shared::cache_paths::CacheLayout) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    cache_layout.native_dir().join("scratch").join(format!("flux_native_{pid}_{counter}"))
}

#[cfg(all(test, feature = "llvm"))]
mod tests {
    use super::{should_skip_native_relink, total_graph_source_lines};
    use crate::syntax::{lexer::Lexer, module_graph::ModuleGraph, parser::Parser};

    #[test]
    fn native_relink_skip_requires_cached_non_emitted_unchanged_binary() {
        assert!(should_skip_native_relink(false, false, false, true));
        assert!(!should_skip_native_relink(true, false, false, true));
        assert!(!should_skip_native_relink(false, true, false, true));
        assert!(!should_skip_native_relink(false, false, true, true));
        assert!(!should_skip_native_relink(false, false, false, false));
    }

    #[test]
    fn total_graph_source_lines_counts_reachable_module_sources() {
        let safe_name = std::thread::current()
            .name()
            .unwrap_or("test")
            .replace(|c: char| !c.is_ascii_alphanumeric(), "_");
        let temp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!("flux_native_line_count_{}_{}", std::process::id(), safe_name));
        std::fs::create_dir_all(&temp).expect("create temp dir");
        let first = temp.join("Main.flx");
        std::fs::write(&first, "fn main = 1\nfn next = 2\n").expect("write first");

        let source = std::fs::read_to_string(&first).expect("read first");
        let mut parser = Parser::new(Lexer::new(&source));
        let program = parser.parse_program();
        let interner = parser.take_interner();
        let graph = ModuleGraph::build_with_entry_and_roots(&first, &program, interner, &[]).graph;

        assert_eq!(total_graph_source_lines(&graph), 2);
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_dir(&temp);
    }
}
