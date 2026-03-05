use std::{
    collections::{HashMap, HashSet},
    hint::black_box,
    time::Instant,
};

use flux::{
    ast::type_infer::{InferProgramConfig, infer_program},
    syntax::{lexer::Lexer, parser::Parser},
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
        let base_symbol = interner_for_base.intern("Base");
        let result = infer_program(
            &program,
            &interner_for_base,
            InferProgramConfig {
                file_path: Some("<perf_guard>".to_string()),
                preloaded_base_schemes: HashMap::new(),
                preloaded_module_member_schemes: HashMap::new(),
                known_base_names: HashSet::new(),
                base_module_symbol: base_symbol,
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
