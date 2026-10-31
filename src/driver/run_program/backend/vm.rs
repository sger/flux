use std::{collections::BTreeMap, path::PathBuf, time::Instant};

use crate as flux;
use crate::driver::{
    backend_policy::{compile_backend_label, execute_backend_label, vm_run_banner},
    flags::DriverFlags,
    pipeline::vm::{VmCompileRequest, compile_vm_modules_parallel},
    reporting::report::{
        AetherTraceContext, ArtifactStats, CompileStats, ExecuteStats, RunStats, TraceBackend,
        count_bytecode_functions, print_aether_trace, print_leak_stats, print_stats,
        write_stats_json,
    },
    shared::{
        DriverCacheConfig, DriverCompileConfig, DriverDiagnosticConfig, DriverRuntimeConfig,
        emit_diagnostics_or_exit,
    },
};
use flux::{
    bytecode::bytecode::Bytecode,
    diagnostics::Diagnostic,
    runtime::value::Value,
    syntax::{module_graph::ModuleGraph, program::Program},
    vm::VM,
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

/// `--metrics` (0190 S15, S11): counts that are the same on every run of the
/// same compiler, keyed by name so the file comes out sorted.
type Metrics = BTreeMap<&'static str, u64>;

/// The counts known before the program runs: Aether's and the Core's over the
/// program's own functions, the bytecode compiled from the entry file, and the
/// compiler's own allocations so far.
fn compile_metrics(request: &mut VmRunRequest<'_>, bytecode: &Bytecode) -> Metrics {
    let program =
        match request.compiler.program_metrics(request.program, request.compile.enable_optimize) {
            Ok(program) => program,
            Err(diag) => {
                emit_diagnostics_or_exit(
                    &[diag],
                    request.path,
                    request.source,
                    request.is_multimodule,
                    request.diagnostics,
                );
                return Metrics::new();
            }
        };
    let mut metrics = Metrics::from([
        ("aether_allocs", program.aether.allocs as u64),
        ("aether_drops", program.aether.drops as u64),
        ("aether_dups", program.aether.dups as u64),
        ("aether_reuses", program.aether.reuses as u64),
        ("core_size", program.core_size as u64),
        ("bytecode_bytes", entry_bytecode_bytes(bytecode, request.path) as u64),
    ]);
    if let Some(allocations) = flux::alloc_count::allocations() {
        metrics.insert("compiler_allocs", allocations);
    }
    metrics
}

/// `<main>`'s instructions and those of every function compiled from the
/// entry file, so that, like the other counts, it leaves the library out.
fn entry_bytecode_bytes(bytecode: &Bytecode, entry: &str) -> usize {
    // Debug info names a module by the path the module graph gave it, which
    // is the entry's canonical path, as `compile_modules` compares it.
    let entry = std::fs::canonicalize(entry)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| entry.to_string());
    let own = bytecode.constants.iter().filter_map(|constant| match constant {
        Value::Function(function) => function
            .debug_info
            .as_ref()
            .filter(|info| info.files.contains(&entry))
            .map(|_| function.instructions.len()),
        _ => None,
    });
    bytecode.instructions.len() + own.sum::<usize>()
}

/// Adds the counts of the run itself, unless the program started worker VMs:
/// which VM runs which task depends on scheduling, so their counts would not
/// repeat.
fn add_run_metrics(metrics: &mut Metrics, vm: &VM, vms_before: usize, allocs_before: Option<u64>) {
    if VM::vms_created() - vms_before > 1 {
        return;
    }
    metrics.insert("vm_instructions", vm.instructions_executed());
    if let (Some(before), Some(after)) = (allocs_before, flux::alloc_count::allocations()) {
        metrics.insert("vm_allocs", after - before);
    }
}

fn format_metrics(metrics: &Metrics) -> String {
    metrics.iter().map(|(key, value)| format!("{key}\t{value}\n")).collect()
}

fn write_metrics(metrics: &Metrics, path: &std::path::Path) {
    if let Err(e) = std::fs::write(path, format_metrics(metrics)) {
        eprintln!("Error: could not write --metrics to {}: {e}", path.display());
        std::process::exit(1);
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
    crate::driver::replay_session::activate_in_process(request.path);
    crate::driver::debug_session::attach(&mut vm, request.path);
    let exec_start = Instant::now();
    let result = vm.run();
    vm.detach_debugger();
    crate::driver::replay_session::finish_vm_run(
        result.as_ref().err().map(String::as_str),
        request.path,
    );
    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
    let execute_ms = exec_start.elapsed().as_secs_f64() * 1000.0;
    if request.runtime.leak_detector {
        print_leak_stats();
    }
    let stats_json_path = request.flags.runtime.stats_json_path.as_deref();
    if request.runtime.show_stats || stats_json_path.is_some() {
        let stats = vm_run_stats(VmRunStatsRequest {
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
        });
        if request.runtime.show_stats {
            print_stats(&stats);
        }
        if let Some(path) = stats_json_path {
            write_stats_json(path, request.path, &stats, &stats.phases());
        }
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
    let metrics_path = request.flags.runtime.metrics_path.clone();
    let mut metrics = metrics_path.is_some().then(|| compile_metrics(&mut request, &bytecode));

    eprintln!("{}", vm_run_banner());
    let vms_before = VM::vms_created();
    let mut vm = VM::new(bytecode);
    vm.set_trace(request.runtime.trace);
    if request.runtime.profiling {
        vm.set_profiling(true, request.compiler.cost_centre_infos.clone());
    }
    crate::driver::replay_session::activate_in_process(request.path);
    crate::driver::debug_session::attach(&mut vm, request.path);
    let allocs_before = flux::alloc_count::allocations();
    let exec_start = Instant::now();
    let result = vm.run();
    vm.detach_debugger();
    crate::driver::replay_session::finish_vm_run(
        result.as_ref().err().map(String::as_str),
        request.path,
    );
    if let (Some(path), Some(metrics)) = (&metrics_path, &mut metrics) {
        // A program that fails keeps the counts it has, with the run's own
        // only when it finished.
        if result.is_ok() {
            add_run_metrics(metrics, &vm, vms_before, allocs_before);
        }
        write_metrics(metrics, path);
    }
    if let Err(err) = result {
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
    let stats_json_path = request.flags.runtime.stats_json_path.as_deref();
    if request.runtime.show_stats || stats_json_path.is_some() {
        let stats = vm_run_stats(VmRunStatsRequest {
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
        });
        if request.runtime.show_stats {
            print_stats(&stats);
        }
        if let Some(path) = stats_json_path {
            write_stats_json(path, request.path, &stats, &stats.phases());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_are_written_one_per_line_sorted_by_key() {
        let metrics =
            Metrics::from([("vm_instructions", 12), ("aether_dups", 3), ("core_size", 40)]);
        assert_eq!(
            format_metrics(&metrics),
            "aether_dups\t3\ncore_size\t40\nvm_instructions\t12\n"
        );
    }
}
