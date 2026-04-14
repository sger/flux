#[cfg(feature = "llvm")]
use std::path::PathBuf;
use std::time::Instant;

use crate as flux;
use crate::driver::shared::{
    DriverCacheConfig, DriverCompileConfig, DriverDiagnosticConfig, DriverRuntimeConfig,
};
#[cfg(feature = "llvm")]
use crate::driver::{
    backend_policy::{
        native_binary_up_to_date_banner, native_lir_lowering_banner,
        native_module_lowering_banner,
    },
    pipeline::native::{
        NativeParallelCompileRequest, compile_native_modules_parallel,
        compile_native_support_object, locate_runtime_lib_dir,
    },
    reporting::{
        report::{
            AetherTraceContext, ArtifactStats, CompileStats, ExecuteStats, RunStats, TraceBackend,
            print_aether_trace, print_stats,
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
/// Builds a merged program view for native-only report and LLVM emission surfaces.
fn merged_native_program(graph: &ModuleGraph) -> Program {
    let mut program = Program::new();
    for node in graph.topo_order() {
        program.statements.extend(node.program.statements.clone());
    }
    program
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
    match input
        .compiler
        .render_aether_report(program, compile.enable_optimize, false)
    {
        Ok(report) => print_aether_trace(
            AetherTraceContext {
                path: input.path,
                backend: TraceBackend::Native,
                pipeline: "AST -> Core -> LIR -> LLVM -> native",
                cache: Some(if cache.no_cache {
                    "disabled"
                } else {
                    "enabled"
                }),
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
    pub(crate) compiler: &'a mut flux::bytecode::compiler::Compiler,
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
    pub(crate) output_path: Option<String>,
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
    request
        .output
        .output_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if request.output.emit_binary {
                PathBuf::from(
                    request
                        .program
                        .path
                        .strip_suffix(".flx")
                        .unwrap_or(request.program.path),
                )
            } else if !request.cache.no_cache {
                let bin_name = std::path::Path::new(request.program.path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("program");
                request
                    .cache
                    .cache_layout
                    .native_dir()
                    .join(format!("{bin_name}.bin"))
            } else {
                native_temp_dir().join("program")
            }
        })
}

#[cfg(feature = "llvm")]
/// Counts source lines across every module in the execution graph for analytics reporting.
fn total_graph_source_lines(graph: &ModuleGraph) -> usize {
    graph
        .topo_order()
        .iter()
        .map(|node| {
            std::fs::read_to_string(&node.path)
                .map(|s| s.lines().count())
                .unwrap_or(0)
        })
        .sum()
}

#[cfg_attr(not(feature = "llvm"), allow(dead_code))]
/// Runs the native backend from already-prepared frontend and pipeline state.
pub(crate) fn run_native_backend(request: NativeRunRequest<'_>) {
    #[cfg(feature = "llvm")]
    {
        let mut request = request;
        let native_program = if request.runtime.trace_aether || request.output.emit_llvm {
            Some(merged_native_program(request.program.graph))
        } else {
            None
        };

        if request.output.emit_llvm {
            let native_program = native_program
                .as_ref()
                .expect("native program should exist for LLVM emission");
            if request.runtime.trace_aether {
                emit_native_aether_trace(
                    &mut request.program,
                    native_program,
                    request.cache,
                    request.compile,
                    request.diagnostics,
                );
            } else {
                request
                    .program
                    .compiler
                    .infer_expr_types_for_program(native_program);
            }
            eprintln!(
                "{}",
                native_lir_lowering_banner()
            );
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
        let runtime_lib_dir = locate_runtime_lib_dir();
        if request.runtime.verbose {
            eprintln!(
                "[lir→llvm] toolchain: {}",
                flux::llvm::pipeline::toolchain_info()
            );
        }
        eprintln!(
            "{}",
            native_module_lowering_banner()
        );
        let native_modules_start = Instant::now();
        let (mut object_paths, any_native_recompiled) = match compile_native_modules_parallel(
            NativeParallelCompileRequest {
                graph: request.program.graph,
                cache_layout: request.cache.cache_layout,
                no_cache: request.cache.no_cache,
                strict_mode: request.compile.strict_mode,
                strict_types: request.compile.strict_types,
                enable_optimize: request.compile.enable_optimize,
                enable_analyze: request.compile.enable_analyze,
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
        let std_lib_objects: Vec<PathBuf> = object_paths
            .iter()
            .filter(|p| flow_object_paths.contains(*p))
            .cloned()
            .collect();

        let link_paths = if std_lib_objects.len() >= 2 && !request.cache.no_cache {
            let archive_name = if request.compile.enable_optimize {
                "libflux_std_O2.a"
            } else {
                "libflux_std_O0.a"
            };
            let archive_path = request.cache.cache_layout.native_dir().join(archive_name);
            let need_rebuild = !flux::llvm::pipeline::archive_is_up_to_date(
                &std_lib_objects,
                &archive_path,
            );
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
                    let mut paths: Vec<PathBuf> = object_paths
                        .iter()
                        .filter(|p| !std_set.contains(*p))
                        .cloned()
                        .collect();
                    paths.push(archive_path);
                    paths
                }
            } else {
                let std_set: HashSet<PathBuf> = std_lib_objects.iter().cloned().collect();
                let mut paths: Vec<PathBuf> = object_paths
                    .iter()
                    .filter(|p| !std_set.contains(*p))
                    .cloned()
                    .collect();
                paths.push(archive_path);
                paths
            }
        } else {
            object_paths.clone()
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
            let native_program = native_program
                .as_ref()
                .expect("native program should exist for Aether tracing");
            emit_native_aether_trace(
                &mut request.program,
                native_program,
                request.cache,
                request.compile,
                request.diagnostics,
            );
        }

        let out = native_output_path(&request);

        let link_start = Instant::now();
        let binary_up_to_date = should_skip_native_relink(
            request.cache.no_cache,
            request.output.emit_binary,
            any_native_recompiled,
            out.exists(),
        );

        if binary_up_to_date {
            if request.runtime.verbose {
                eprintln!(
                    "{}",
                    native_binary_up_to_date_banner()
                );
            }
        } else if let Err(e) = flux::llvm::pipeline::link_objects(
            &link_paths,
            &out,
            runtime_lib_dir.as_deref(),
        ) {
            eprintln!("llvm linker failed: {e}");
            std::process::exit(1);
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
        match std::process::Command::new(&out).output() {
            Ok(output) => {
                let exit_code = output.status.code().unwrap_or(1);
                let execute_ms = exec_start.elapsed().as_secs_f64() * 1000.0;
                let child_stdout = String::from_utf8_lossy(&output.stdout);
                let child_stderr = String::from_utf8_lossy(&output.stderr);
                if !child_stdout.is_empty() {
                    print!("{child_stdout}");
                }
                if exit_code == 0 {
                    if !child_stderr.is_empty() {
                        eprint!("{child_stderr}");
                    }
                } else if let Some(rendered) = if request.report.render_runtime_error {
                    render_native_runtime_error(request.program.path, &child_stderr)
                } else {
                    None
                } {
                    eprint!("{rendered}");
                } else if !child_stderr.is_empty() {
                    eprint!("{child_stderr}");
                }
                if request.runtime.show_stats {
                    let compile_ms =
                        request.program.compile_start.elapsed().as_secs_f64() * 1000.0 - execute_ms;
                    print_stats(&RunStats {
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
                    });
                }
                if request.cache.no_cache || request.output.emit_binary {
                    let _ = std::fs::remove_file(&out);
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
/// Allocates a unique temporary output directory for uncached native runs.
fn native_temp_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("flux_native_{pid}_{counter}"))
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
        let temp = std::env::temp_dir().join(format!(
            "flux_native_line_count_{}_{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
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
