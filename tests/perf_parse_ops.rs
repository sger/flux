use flux::frontend::{lexer::Lexer, parser::Parser};
use std::{hint::black_box, time::Instant};

fn make_operator_heavy_source(ops: usize) -> String {
    let tokens = [
        "+", "*", "-", "/", "%", "==", "!=", "<", ">", "<=", ">=", "&&", "||",
    ];

    let mut src = String::with_capacity(ops * 6);
    src.push('a');

    for i in 0..ops {
        let op = tokens[i % tokens.len()];
        src.push(' ');
        src.push_str(op);
        src.push(' ');
        src.push_str("a");
    }

    src.push(';');
    src
}

#[test]
#[ignore = "perf guard; run with `cargo test --test perf_parse_ops -- --ignored --nocapture`"]
fn parse_operator_heavy_expression_perf_guard() {
    let source = make_operator_heavy_source(20_000);
    let iterations = 40usize;
    let start = Instant::now();

    for _ in 0..iterations {
        let lexer = Lexer::new(&source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "unexpected parser diagnostics in perf guard: {:?}",
            parser.errors
        );
        black_box(program.statements.len());
    }

    let elapsed = start.elapsed();
    eprintln!(
        "parsed {} ops x {} iterations in {:?}",
        20_000, iterations, elapsed
    );
}
