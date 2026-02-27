use std::fmt::Write;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::syntax::{lexer::Lexer, parser::Parser};

struct Corpus {
    name: &'static str,
    source: String,
}

fn build_declaration_heavy_corpus() -> String {
    let mut src = String::with_capacity(256_000);
    for i in 0..2500usize {
        let _ = writeln!(src, "let value_{i}: Int = {i};");
        let _ = writeln!(src, "fn f_{i}(x: Int) -> Int {{ x + value_{i} }}");
    }
    src
}

fn build_expression_heavy_corpus() -> String {
    let mut src = String::with_capacity(256_000);
    for i in 0..3000usize {
        let _ = writeln!(
            src,
            "let out_{i} = (a_{i} + b_{i} * c_{i}) |> normalize_{i}(1, 2) |> clamp_{i}(0, 10);"
        );
        let _ = writeln!(
            src,
            "if out_{i} > 10 && out_{i} != 42 {{ out_{i} }} else {{ out_{i} + 1 }};"
        );
    }
    src
}

fn build_string_interp_comment_corpus() -> String {
    let mut src = String::with_capacity(256_000);
    for i in 0..2000usize {
        let _ = writeln!(src, "/// parser bench doc comment {i}");
        let _ = writeln!(
            src,
            "let s_{i} = \"prefix #{{name_{i}}} middle #{{count_{i} + 1}} suffix\";"
        );
        let _ = writeln!(src, "let t_{i} = \"line\\nquote:\\\" hash:\\#{{ok}}\";");
        let _ = writeln!(src, "/* block parser bench {i} */");
    }
    src
}

fn build_malformed_recovery_corpus() -> String {
    let mut src = String::with_capacity(256_000);
    for i in 0..2200usize {
        let _ = writeln!(src, "let bad_{i} = (1 + ;");
        let _ = writeln!(src, "match x_{i} {{ 0 -> 1; 1 -> 2 }}");
        let _ = writeln!(src, "if x_{i} < {{ y_{i};");
    }
    src
}

fn build_corpora() -> Vec<Corpus> {
    vec![
        Corpus {
            name: "declaration_heavy",
            source: build_declaration_heavy_corpus(),
        },
        Corpus {
            name: "expression_operator_heavy",
            source: build_expression_heavy_corpus(),
        },
        Corpus {
            name: "string_interp_comment_heavy",
            source: build_string_interp_comment_corpus(),
        },
        Corpus {
            name: "malformed_recovery_heavy",
            source: build_malformed_recovery_corpus(),
        },
    ]
}

fn parse_program_stats(input: &str) -> (usize, usize) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    (program.statements.len(), parser.errors.len())
}

#[allow(clippy::needless_as_bytes)]
fn bench_parser(c: &mut Criterion) {
    let corpora = build_corpora();
    let mut group = c.benchmark_group("parser/parse_program");

    for corpus in &corpora {
        let input = corpus.source.as_str();
        group.throughput(Throughput::Bytes(input.as_bytes().len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(corpus.name),
            input,
            |b, input| {
                b.iter(|| {
                    let stats = parse_program_stats(black_box(input));
                    black_box(stats);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_parser);
criterion_main!(benches);
