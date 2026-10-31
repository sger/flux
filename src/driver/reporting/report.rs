//! Driver-owned reporting helpers for analytics, backend contracts, and Aether traces.

use crate as flux;
use crate::driver::backend_policy::trace_backend_label;

/// Backend label used in Aether trace output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "llvm"), allow(dead_code))]
pub(crate) enum TraceBackend {
    Vm,
    Native,
}

/// Compile- and cache-related runtime statistics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CompileStats {
    pub(crate) parse_ms: Option<f64>,
    pub(crate) compile_ms: Option<f64>,
    pub(crate) compile_backend: Option<&'static str>,
    pub(crate) cached: bool,
    pub(crate) module_count: Option<usize>,
    pub(crate) cached_module_count: Option<usize>,
    pub(crate) compiled_module_count: Option<usize>,
}

/// Execution-related runtime statistics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ExecuteStats {
    pub(crate) execute_ms: f64,
    pub(crate) execute_backend: &'static str,
}

/// Program-size runtime statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArtifactStats {
    pub(crate) source_lines: usize,
    pub(crate) globals_count: Option<usize>,
    pub(crate) functions_count: Option<usize>,
    pub(crate) instruction_bytes: Option<usize>,
}

/// Full analytics payload printed by driver execution backends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RunStats {
    pub(crate) compile: CompileStats,
    pub(crate) execute: ExecuteStats,
    pub(crate) artifacts: ArtifactStats,
}

/// One timed step of a run, as `--stats-json` records it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Phase {
    pub(crate) name: &'static str,
    pub(crate) ms: f64,
}

impl RunStats {
    /// The parse, compile and execute steps the `--stats` table shows.
    pub(crate) fn phases(&self) -> Vec<Phase> {
        let mut phases = Vec::new();
        if let Some(ms) = self.compile.parse_ms {
            phases.push(Phase { name: "parse", ms });
        }
        if let Some(ms) = self.compile.compile_ms {
            phases.push(Phase { name: "compile", ms });
        }
        phases.push(Phase { name: "execute", ms: self.execute.execute_ms });
        phases
    }
}

/// Grouped Aether trace metadata printed before the ownership report itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AetherTraceContext<'a> {
    pub(crate) path: &'a str,
    pub(crate) backend: TraceBackend,
    pub(crate) pipeline: &'a str,
    pub(crate) cache: Option<&'a str>,
    pub(crate) optimize: bool,
    pub(crate) analyze: bool,
    pub(crate) strict: bool,
    pub(crate) module_count: Option<usize>,
}

/// Prints the backend representation contract used for debugging backend mismatches.
pub(crate) fn print_backend_representation_contract() {
    println!(
        "\
Flux Backend Representation Contract
===================================

family.none = sentinel:none
family.empty_list = sentinel:empty_list
family.list_cons = boxed:adt:ctor=Cons
family.tuple = boxed:tuple
family.user_adt = boxed:adt:user_ctor
family.array = boxed:array
family.string = boxed:string
family.float = boxed:float
family.closure = boxed:closure
family.hashmap = boxed:hamt

rule.match_ctor = decode_ctor_only_after_family_proof
rule.list_pattern = require_family:list_cons_or_empty_list
rule.tuple_pattern = require_family:tuple
rule.adt_pattern = require_family:user_adt_or_builtin_adt
rule.none_pattern = require_family:none
rule.empty_list_pattern = require_family:empty_list

vm.proof = shape_opcodes_only_accept_corresponding_Value_variants
native.proof = ctor_dispatch_requires_heap_obj_tag_before_layout_reads

debug.core = frontend_bug_if_mismatch
debug.aether = ownership_bug_if_mismatch
debug.repr = backend_representation_bug_if_core_and_aether_match
"
    );
}

/// Prints the Aether ownership trace header plus the rendered ownership report.
pub(crate) fn print_aether_trace(context: AetherTraceContext<'_>, report: &str) {
    eprintln!();
    eprintln!("── Aether Trace ──");
    eprintln!("file: {}", context.path);
    eprintln!("backend: {}", trace_backend_label(context.backend));
    eprintln!("pipeline: {}", context.pipeline);
    if let Some(cache_mode) = context.cache {
        eprintln!("cache: {}", cache_mode);
    }
    eprintln!("optimize: {}", if context.optimize { "on" } else { "off" });
    eprintln!("analyze: {}", if context.analyze { "on" } else { "off" });
    eprintln!("strict: {}", if context.strict { "on" } else { "off" });
    if let Some(count) = context.module_count {
        eprintln!("modules: {}", count);
    }
    eprintln!("────────────────────────");
    eprintln!("{report}");
}

/// Counts the number of compiled function constants in a bytecode constant table.
pub(crate) fn count_bytecode_functions(constants: &[flux::runtime::value::Value]) -> usize {
    use flux::runtime::value::Value;
    constants.iter().filter(|v| matches!(v, Value::Function(_))).count()
}

/// Prints execution analytics collected by the driver.
pub(crate) fn print_stats(stats: &RunStats) {
    let total_ms = stats.compile.parse_ms.unwrap_or(0.0)
        + stats.compile.compile_ms.unwrap_or(0.0)
        + stats.execute.execute_ms;

    let w = 46usize;
    eprintln!();
    eprintln!("  ── Flux Analytics {}", "─".repeat(w - 19));

    if let Some(ms) = stats.compile.parse_ms {
        eprintln!("  {:<20} {:>8.2} ms", "parse", ms);
    }

    if stats.compile.cached {
        eprintln!("  {:<20} {:>12}", "compile", "(cached)");
    } else if let Some(ms) = stats.compile.compile_ms {
        eprintln!(
            "  {:<20} {:>8.2} ms  [{}]",
            "compile",
            ms,
            stats.compile.compile_backend.unwrap_or("unknown")
        );
    }

    eprintln!(
        "  {:<20} {:>8.2} ms  [{}]",
        "execute", stats.execute.execute_ms, stats.execute.execute_backend
    );
    eprintln!("  {:<20} {:>8.2} ms", "total", total_ms);
    eprintln!();

    if let Some(n) = stats.compile.module_count {
        match (stats.compile.cached_module_count, stats.compile.compiled_module_count) {
            (Some(cached), Some(compiled)) if cached > 0 => {
                eprintln!(
                    "  {:<20} {:>8}  ({} cached, {} compiled)",
                    "modules", n, cached, compiled
                );
            }
            _ => eprintln!("  {:<20} {:>8}", "modules", n),
        }
    }
    eprintln!("  {:<20} {:>8}", "source lines", stats.artifacts.source_lines);
    if let Some(n) = stats.artifacts.globals_count {
        eprintln!("  {:<20} {:>8}", "globals", n);
    }
    if let Some(n) = stats.artifacts.functions_count {
        eprintln!("  {:<20} {:>8}", "functions", n);
    }
    if let Some(n) = stats.artifacts.instruction_bytes {
        eprintln!("  {:<20} {:>8} bytes", "instructions", n);
    }
    eprintln!("  {}", "─".repeat(w - 2));
}

/// The `--stats-json` record for one run. Counts a backend does not have are
/// `null`, so the VM and native runs share one shape and a diff can line
/// them up.
pub(crate) fn stats_json(file: &str, stats: &RunStats, phases: &[Phase]) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "file": file,
        "backend": stats.execute.execute_backend,
        "phases": phases
            .iter()
            .map(|phase| serde_json::json!({ "phase": phase.name, "ms": phase.ms }))
            .collect::<Vec<_>>(),
        "modules": {
            "total": stats.compile.module_count,
            "cached": stats.compile.cached_module_count,
            "compiled": stats.compile.compiled_module_count,
        },
        "source_lines": stats.artifacts.source_lines,
        "globals": stats.artifacts.globals_count,
        "functions": stats.artifacts.functions_count,
        "instruction_bytes": stats.artifacts.instruction_bytes,
    })
}

/// Writes `stats_json` to `path`, exiting as `--metrics` does when it cannot.
pub(crate) fn write_stats_json(
    path: &std::path::Path,
    file: &str,
    stats: &RunStats,
    phases: &[Phase],
) {
    let mut text = serde_json::to_string_pretty(&stats_json(file, stats, phases))
        .expect("stats are plain JSON values");
    text.push('\n');
    if let Err(e) = std::fs::write(path, text) {
        eprintln!("Error: could not write --stats-json to {}: {e}", path.display());
        std::process::exit(1);
    }
}

/// Prints approximate runtime allocation counters from the VM leak detector.
pub(crate) fn print_leak_stats() {
    let stats = flux::runtime::leak_detector::snapshot();
    println!(
        "\nLeak stats (approx):\n  compiled_functions: {}\n  closures: {}\n  arrays: {}\n  hashes: {}\n  somes: {}",
        stats.compiled_functions, stats.closures, stats.arrays, stats.hashes, stats.somes
    );
}

#[cfg(test)]
mod tests {
    use super::{
        AetherTraceContext, ArtifactStats, CompileStats, ExecuteStats, Phase, RunStats,
        TraceBackend, stats_json,
    };

    fn native_stats() -> RunStats {
        RunStats {
            compile: CompileStats {
                parse_ms: Some(1.0),
                compile_ms: Some(20.0),
                compile_backend: Some("llvm"),
                cached: false,
                module_count: Some(18),
                cached_module_count: None,
                compiled_module_count: None,
            },
            execute: ExecuteStats { execute_ms: 3.0, execute_backend: "native" },
            artifacts: ArtifactStats {
                source_lines: 1943,
                globals_count: None,
                functions_count: None,
                instruction_bytes: None,
            },
        }
    }

    #[test]
    fn the_stats_table_gives_parse_compile_and_execute_phases() {
        let names: Vec<_> = native_stats().phases().iter().map(|p| p.name).collect();
        assert_eq!(names, ["parse", "compile", "execute"]);
    }

    #[test]
    fn stats_json_keeps_phase_order_and_writes_missing_counts_as_null() {
        let phases = [
            Phase { name: "parse", ms: 1.0 },
            Phase { name: "link", ms: 5.5 },
            Phase { name: "execute", ms: 3.0 },
        ];

        let json = stats_json("main.flx", &native_stats(), &phases);

        assert_eq!(json["version"], 1);
        assert_eq!(json["file"], "main.flx");
        assert_eq!(json["backend"], "native");
        let names: Vec<_> = json["phases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["phase"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["parse", "link", "execute"]);
        assert_eq!(json["phases"][1]["ms"], 5.5);
        assert_eq!(json["modules"]["total"], 18);
        assert!(json["modules"]["cached"].is_null());
        assert!(json["globals"].is_null());
        assert_eq!(json["source_lines"], 1943);
    }

    #[test]
    fn run_stats_groups_compile_execute_and_artifact_data() {
        let stats = RunStats {
            compile: CompileStats {
                parse_ms: Some(1.0),
                compile_ms: Some(2.0),
                compile_backend: Some("bytecode"),
                cached: false,
                module_count: Some(1),
                cached_module_count: None,
                compiled_module_count: Some(1),
            },
            execute: ExecuteStats { execute_ms: 3.0, execute_backend: "vm" },
            artifacts: ArtifactStats {
                source_lines: 10,
                globals_count: Some(1),
                functions_count: Some(2),
                instruction_bytes: Some(42),
            },
        };

        assert_eq!(stats.compile.module_count, Some(1));
        assert_eq!(stats.execute.execute_backend, "vm");
        assert_eq!(stats.artifacts.instruction_bytes, Some(42));
    }

    #[test]
    fn aether_trace_context_keeps_trace_metadata_grouped() {
        let context = AetherTraceContext {
            path: "main.flx",
            backend: TraceBackend::Native,
            pipeline: "AST -> Core -> LIR -> LLVM -> native",
            cache: Some("enabled"),
            optimize: true,
            analyze: false,
            strict: true,
            module_count: Some(2),
        };

        assert_eq!(context.backend, TraceBackend::Native);
        assert_eq!(context.cache, Some("enabled"));
        assert_eq!(context.module_count, Some(2));
    }
}
