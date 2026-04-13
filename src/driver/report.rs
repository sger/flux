use crate::{
    driver::backend_policy,
    runtime::{leak_detector, value::Value},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraceBackend {
    Vm,
}

pub(crate) struct RunStats {
    pub(crate) parse_ms: Option<f64>,
    pub(crate) compile_ms: Option<f64>,
    pub(crate) compile_backend: Option<&'static str>,
    pub(crate) execute_ms: f64,
    pub(crate) execute_backend: &'static str,
    pub(crate) cached: bool,
    pub(crate) module_count: Option<usize>,
    pub(crate) cached_module_count: Option<usize>,
    pub(crate) compiled_module_count: Option<usize>,
    pub(crate) source_lines: usize,
    pub(crate) globals_count: Option<usize>,
    pub(crate) functions_count: Option<usize>,
    pub(crate) instruction_bytes: Option<usize>,
}

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

pub(crate) fn print_aether_trace(
    path: &str,
    backend: TraceBackend,
    pipeline: &str,
    cache: Option<&str>,
    optimize: bool,
    analyze: bool,
    strict: bool,
    module_count: Option<usize>,
    report: &str,
) {
    let backend_name = backend_policy::trace_backend_label(backend);

    eprintln!();
    eprintln!("--- Aether Trace ---");
    eprintln!("file: {}", path);
    eprint!("backend: {}", backend_name);
    eprintln!("pipeline: {}", pipeline);
    if let Some(cache_mode) = cache {
        eprintln!("cache: {}", cache_mode);
    }
    eprintln!("optimize: {}", if optimize { "on" } else { "off" });
    eprintln!("analyze: {}", if analyze { "on" } else { "off" });
    eprintln!("strict: {}", if strict { "on" } else { "off" });
    if let Some(count) = module_count {
        eprintln!("modules: {}", count);
    }
    eprintln!("────────────────────────");
    eprintln!("{report}");
}

pub(crate) fn count_bytecode_functions(constants: &[Value]) -> usize {
    constants
        .iter()
        .filter(|v| matches!(v, Value::Function(_)))
        .count()
}

pub(crate) fn print_stats(stats: &RunStats) {
    let total_ms =
        stats.parse_ms.unwrap_or(0.0) + stats.compile_ms.unwrap_or(0.0) + stats.execute_ms;

    let w = 46usize;
    eprintln!();
    eprintln!("  ── Flux Analytics {}", "--".repeat(w - 19));

    if let Some(ms) = stats.parse_ms {
        eprintln!("  {:<20} {:>8.2} ms", "parse", ms);
    }

    if stats.cached {
        eprintln!("  {:<20} {:>12}", "compile", "(cached)");
    } else if let Some(ms) = stats.compile_ms {
        eprintln!(
            "  {:<20} {:>8.2} ms  [{}]",
            "compile",
            ms,
            stats.compile_backend.unwrap_or("unknown")
        );
    }

    eprintln!(
        "  {:<20} {:>8.2} ms  [{}]",
        "execute", stats.execute_ms, stats.execute_backend
    );
    eprintln!("  {:<20} {:>8.2} ms", "total", total_ms);
    eprintln!();

    if let Some(n) = stats.module_count {
        match (stats.cached_module_count, stats.compiled_module_count) {
            (Some(cached), Some(compiled)) if cached > 0 => {
                eprintln!(
                    "  {:<20} {:>8}  ({} cached, {} compiled)",
                    "modules", n, cached, compiled
                );
            }
            _ => eprintln!("  {:<20} {:>8}", "modules", n),
        }
    }
    eprintln!("  {:<20} {:>8}", "source lines", stats.source_lines);
    if let Some(n) = stats.globals_count {
        eprintln!("  {:<20} {:>8}", "globals", n);
    }
    if let Some(n) = stats.functions_count {
        eprintln!("  {:<20} {:>8}", "functions", n);
    }
    if let Some(n) = stats.instruction_bytes {
        eprintln!("  {:<20} {:>8} bytes", "instructions", n);
    }
    eprintln!("  {}", "─".repeat(w - 2));
}

pub(crate) fn print_leak_stats() {
    let stats = leak_detector::snapshot();
    println!(
        "\nLeak stats (approx):\n  compiled_functions: {}\n  closures: {}\n  arrays: {}\n  hashes: {}\n  somes: {}",
        stats.compiled_functions, stats.closures, stats.arrays, stats.hashes, stats.somes
    );
}
