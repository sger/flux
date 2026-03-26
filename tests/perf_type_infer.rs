use std::{
    collections::{HashMap, HashSet},
    hint::black_box,
    time::Instant,
};

use flux::{
    ast::type_infer::{InferProgramConfig, infer_program},
    syntax::{lexer::Lexer, parser::Parser},
    types::{infer_type::InferType, type_constructor::TypeConstructor, type_subst::TypeSubst},
};

/// Build a call-chain heavy source program for HM inference perf sampling.
fn make_infer_stress_source(depth: usize) -> String {
    let mut source = String::new();
    source.push_str("fn f0(x) { x }\n");
    for index in 1..=depth {
        source.push_str(&format!("fn f{index}(x) {{ f{}(x) }}\n", index - 1));
    }
    source.push_str(&format!(
        "fn main() -> Unit {{ let _v = f{depth}(1); let _w = f{depth}(2); }}"
    ));
    source
}

#[test]
#[ignore = "perf guard; run with `cargo test --test perf_type_infer -- --ignored --nocapture`"]
fn infer_program_perf_guard() {
    let source = make_infer_stress_source(300);
    let iterations = 30usize;
    let start = Instant::now();

    for _ in 0..iterations {
        let lexer = Lexer::new(&source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "unexpected parser diagnostics in infer perf guard: {:?}",
            parser.errors
        );

        let mut interner_for_base = parser.take_interner();
        let base_symbol = interner_for_base.intern("Flow");
        let result = infer_program(
            &program,
            &interner_for_base,
            InferProgramConfig {
                file_path: Some("<perf_guard>".into()),
                preloaded_base_schemes: HashMap::new(),
                preloaded_module_member_schemes: HashMap::new(),
                known_flow_names: HashSet::new(),
                flow_module_symbol: base_symbol,
                preloaded_effect_op_signatures: HashMap::new(),
            },
        );
        black_box(result.diagnostics.len());
    }

    let elapsed = start.elapsed();
    eprintln!(
        "inferred call-chain depth {} x {} iterations in {:?}",
        300, iterations, elapsed
    );
}

/// Micro-benchmark: compose() cost with growing substitution size.
///
/// Simulates the Algorithm W pattern: each unification produces a small
/// substitution that is composed into the accumulator. With lazy normalization,
/// compose() should be O(|other|) instead of O(|self| * |other|).
#[test]
#[ignore = "perf guard; run with `cargo test --test perf_type_infer -- --ignored --nocapture`"]
fn compose_scaling_perf_guard() {
    let var_count = 2000usize;
    let iterations = 10usize;

    let start = Instant::now();

    for _ in 0..iterations {
        // Build a chain: ?0 → ?1, ?1 → ?2, ..., ?(n-1) → Int
        let mut accumulated = TypeSubst::empty();

        for i in 0..var_count {
            let mut single = TypeSubst::empty();
            if i + 1 < var_count {
                single.insert(i as u32, InferType::Var((i + 1) as u32));
            } else {
                single.insert(i as u32, InferType::Con(TypeConstructor::Int));
            }
            accumulated = accumulated.compose(&single);
        }

        // Verify observable correctness: first var resolves to Int via chain.
        let resolved = InferType::Var(0).apply_type_subst(&accumulated);
        assert_eq!(resolved, InferType::Con(TypeConstructor::Int));
        black_box(&accumulated);
    }

    let elapsed = start.elapsed();
    let per_compose_ns = elapsed.as_nanos() / (var_count as u128 * iterations as u128);
    eprintln!(
        "compose chain length {} x {} iterations in {:?} ({} ns/compose)",
        var_count, iterations, elapsed, per_compose_ns
    );
    // With lazy normalization, per-compose cost should be roughly constant
    // (not proportional to accumulated size). Fail if > 10µs per compose.
    assert!(
        per_compose_ns < 10_000,
        "compose() took {per_compose_ns} ns per call — expected < 10µs with lazy normalization"
    );
}
