use std::{path::PathBuf, time::Instant};

use crate as flux;
use crate::driver::{
    backend_policy::{compile_backend_label, execute_backend_label, vm_run_banner},
    flags::DriverFlags,
    pipeline::vm::{VmCompileRequest, compile_vm_modules_parallel},
    reporting::report::{
        AetherTraceContext, ArtifactStats, CompileStats, ExecuteStats, RunStats, TraceBackend,
        count_bytecode_functions, print_aether_trace, print_leak_stats, print_stats,
    },
    shared::{
        DriverCacheConfig, DriverCompileConfig, DriverDiagnosticConfig, DriverRuntimeConfig,
        emit_diagnostics_or_exit,
    },
};
use flux::{
    vm::VM,
    diagnostics::Diagnostic,
    syntax::{module_graph::ModuleGraph, program::Program},
};

/// Builds analytics output for a VM execution path.
struct VmRunStatsRequest<'a> {
    flags: &'a DriverFlags,
    parse_ms: f64,
    compile_ms: f64,
    module_count: usize,
    cached_module_count: Option<usize>,
    compiled_module_count: Option<usize>,
    source: &'a str,
    globals_count: usize,
    functions_count: usize,
    instruction_bytes: usize,
    execute_ms: f64,
}

/// Builds analytics output for a VM execution path.
fn vm_run_stats(request: VmRunStatsRequest<'_>) -> RunStats {
    RunStats {
        compile: CompileStats {
            parse_ms: Some(request.parse_ms),
            compile_ms: Some(request.compile_ms),
            compile_backend: Some(compile_backend_label(request.flags)),
            cached: false,
            module_count: Some(request.module_count),
            cached_module_count: request.cached_module_count,
            compiled_module_count: request.compiled_module_count,
        },
        execute: ExecuteStats {
            execute_ms: request.execute_ms,
            execute_backend: execute_backend_label(request.flags),
        },
        artifacts: ArtifactStats {
            source_lines: request.source.lines().count(),
            globals_count: Some(request.globals_count),
            functions_count: Some(request.functions_count),
            instruction_bytes: Some(request.instruction_bytes),
        },
    }
}

/// Emits the VM Aether trace if the current run requested it.
fn emit_vm_aether_trace(request: &mut VmRunRequest<'_>) {
    match request.compiler.render_aether_report(
        request.program,
        request.compile.enable_optimize,
        false,
    ) {
        Ok(report) => print_aether_trace(
            AetherTraceContext {
                path: request.path,
                backend: TraceBackend::Vm,
                pipeline: "AST -> Core -> CFG -> bytecode -> VM",
                cache: Some("disabled"),
                optimize: request.compile.enable_optimize,
                analyze: request.compile.enable_analyze,
                strict: request.compile.strict_mode,
                module_count: Some(request.module_count),
            },
            &report,
        ),
        Err(diag) => emit_diagnostics_or_exit(
            &[diag],
            request.path,
            request.source,
            request.is_multimodule,
            request.diagnostics,
        ),
    }
}

pub(crate) struct ParallelVmRunRequest<'a> {
    pub(crate) graph: &'a ModuleGraph,
    pub(crate) entry_canonical: Option<&'a PathBuf>,
    pub(crate) graph_interner: &'a flux::syntax::interner::Interner,
    pub(crate) cache: DriverCacheConfig<'a>,
    pub(crate) compile: DriverCompileConfig,
    pub(crate) diagnostics: DriverDiagnosticConfig,
    pub(crate) runtime: DriverRuntimeConfig,
    pub(crate) flags: &'a DriverFlags,
    pub(crate) all_diagnostics: &'a mut Vec<Diagnostic>,
    pub(crate) path: &'a str,
    pub(crate) source: &'a str,
    pub(crate) parse_ms: f64,
    pub(crate) compile_start: Instant,
    pub(crate) module_count: usize,
}

/// Attempts the cached parallel VM fast path and returns whether it handled the run.
pub(crate) fn try_run_parallel_vm(request: ParallelVmRunRequest<'_>) -> bool {
    let build = match compile_vm_modules_parallel(
        VmCompileRequest {
            graph: request.graph,
            entry_canonical: request.entry_canonical,
            graph_interner: request.graph_interner,
            cache: request.cache,
            compile: request.compile,
            runtime: request.runtime,
        },
        request.all_diagnostics,
    ) {
        Ok(build) => build,
        Err(err) => {
            eprintln!("parallel VM compilation failed: {err}");
            std::process::exit(1);
        }
    };

    emit_diagnostics_or_exit(
        request.all_diagnostics,
        request.path,
        request.source,
        true,
        request.diagnostics,
    );

    let compile_ms = request.compile_start.elapsed().as_secs_f64() * 1000.0;
    let bytecode = build.bytecode;
    let globals_count = build.symbol_table.num_definitions;
    let functions_count = count_bytecode_functions(&bytecode.constants);
    let instruction_bytes = bytecode.instructions.len();

    eprintln!("{}", vm_run_banner());
    let mut vm = VM::new(bytecode);
    vm.set_trace(request.runtime.trace);
    let exec_start = Instant::now();
    if let Err(err) = vm.run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
    let execute_ms = exec_start.elapsed().as_secs_f64() * 1000.0;
    if request.runtime.leak_detector {
        print_leak_stats();
    }
    if request.runtime.show_stats {
        print_stats(&vm_run_stats(VmRunStatsRequest {
            flags: request.flags,
            parse_ms: request.parse_ms,
            compile_ms,
            module_count: request.module_count,
            cached_module_count: Some(build.cached_count),
            compiled_module_count: Some(build.compiled_count),
            source: request.source,
            globals_count,
            functions_count,
            instruction_bytes,
            execute_ms,
        }));
    }

    true
}

pub(crate) struct VmRunRequest<'a> {
    pub(crate) compiler: &'a mut flux::compiler::Compiler,
    pub(crate) program: &'a Program,
    pub(crate) path: &'a str,
    pub(crate) source: &'a str,
    pub(crate) is_multimodule: bool,
    pub(crate) module_count: usize,
    pub(crate) parse_ms: f64,
    pub(crate) compile_ms: f64,
    pub(crate) flags: &'a DriverFlags,
    pub(crate) compile: DriverCompileConfig,
    pub(crate) diagnostics: DriverDiagnosticConfig,
    pub(crate) runtime: DriverRuntimeConfig,
}

/// Runs the single-program VM backend after compilation has completed.
pub(crate) fn run_vm(mut request: VmRunRequest<'_>) {
    let bytecode = request.compiler.bytecode();
    let globals_count = request.compiler.symbol_table.num_definitions;
    let functions_count = count_bytecode_functions(&bytecode.constants);
    let instruction_bytes = bytecode.instructions.len();
    if request.flags.runtime.trace_aether {
        emit_vm_aether_trace(&mut request);
    }

    eprintln!("{}", vm_run_banner());
    let mut vm = VM::new(bytecode);
    vm.set_trace(request.runtime.trace);
    if request.runtime.profiling {
        vm.set_profiling(true, request.compiler.cost_centre_infos.clone());
    }
    let exec_start = Instant::now();
    if let Err(err) = vm.run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
    let execute_ns = exec_start.elapsed().as_nanos() as u64;
    let execute_ms = execute_ns as f64 / 1_000_000.0;
    if request.runtime.profiling {
        vm.print_profile_report(execute_ns);
    }
    if request.runtime.leak_detector {
        print_leak_stats();
    }
    if request.runtime.show_stats {
        print_stats(&vm_run_stats(VmRunStatsRequest {
            flags: request.flags,
            parse_ms: request.parse_ms,
            compile_ms: request.compile_ms,
            module_count: request.module_count,
            cached_module_count: None,
            compiled_module_count: None,
            source: request.source,
            globals_count,
            functions_count,
            instruction_bytes,
            execute_ms,
        }));
    }
}
