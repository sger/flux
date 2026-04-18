use std::fmt::Write;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::{
    compiler::Compiler,
    syntax::{interner::Interner, lexer::Lexer, parser::Parser, program::Program},
};

struct Corpus {
    name: &'static str,
    source: String,
}

fn build_typed_function_corpus() -> String {
    let mut src = String::with_capacity(256_000);
    for i in 0..260usize {
        let _ = writeln!(src, "fn f_{i}(x: Int) -> Int {{ x + 1 }}");
    }

    for i in 0..420usize {
        let idx = i % 260;
        let _ = writeln!(src, "let v_{i}: Int = f_{idx}({i});");
    }

    let _ = writeln!(src, "fn main() {{ v_0 + v_1 + v_2; }}");
    src
}

fn build_match_adt_like_corpus() -> String {
    let mut src = String::with_capacity(256_000);
    let _ = writeln!(src, "type Result<T, E> = Ok(T) | Err(E)");

    for i in 0..220usize {
        let _ = writeln!(src, "fn f_{i}(x: Int) -> Result<Int, String> {{");
        let _ = writeln!(src, "  if x > 0 {{ Ok(x) }} else {{ Err(\"bad\") }}");
        let _ = writeln!(src, "}}");
        let _ = writeln!(
            src,
            "let m_{i}: Int = match f_{i}({i}) {{ Ok(v) -> v, Err(_) -> 0 }};"
        );
    }

    let _ = writeln!(src, "fn main() {{ m_0 + m_1 + m_2; }}");
    src
}

fn build_corpora() -> Vec<Corpus> {
    vec![
        Corpus {
            name: "typed_function_heavy",
            source: build_typed_function_corpus(),
        },
        Corpus {
            name: "match_adt_heavy",
            source: build_match_adt_like_corpus(),
        },
    ]
}

fn parse_program(input: &str) -> (Program, Interner) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    assert!(
        parser.errors.is_empty(),
        "benchmark corpus must parse cleanly, errors: {:?}",
        parser.errors
    );
    (program, interner)
}

#[allow(clippy::needless_as_bytes)]
fn bench_compile_with_opts(c: &mut Criterion) {
    let corpora = build_corpora();

    let mut no_analyze_group = c.benchmark_group("compiler/compile_with_opts_no_analyze");
    for corpus in &corpora {
        let (program, interner) = parse_program(&corpus.source);
        no_analyze_group.throughput(Throughput::Bytes(corpus.source.as_bytes().len() as u64));
        no_analyze_group.bench_with_input(
            BenchmarkId::from_parameter(corpus.name),
            &(program, interner),
            |b, (program, interner)| {
                b.iter(|| {
                    let mut compiler =
                        Compiler::new_with_interner("<bench>", black_box(interner.clone()));
                    let result = compiler.compile_with_opts(black_box(program), false, false);
                    if let Err(diags) = &result {
                        panic!(
                            "compile_with_opts(false,false) failed, first diagnostic: {:?}",
                            diags.first()
                        );
                    }
                    let _ = black_box(result);
                });
            },
        );
    }
    no_analyze_group.finish();

    let mut analyze_group = c.benchmark_group("compiler/compile_with_opts_analyze");
    for corpus in &corpora {
        let (program, interner) = parse_program(&corpus.source);
        analyze_group.throughput(Throughput::Bytes(corpus.source.as_bytes().len() as u64));
        analyze_group.bench_with_input(
            BenchmarkId::from_parameter(corpus.name),
            &(program, interner),
            |b, (program, interner)| {
                b.iter(|| {
                    let mut compiler =
                        Compiler::new_with_interner("<bench>", black_box(interner.clone()));
                    let result = compiler.compile_with_opts(black_box(program), false, true);
                    if let Err(diags) = &result {
                        panic!(
                            "compile_with_opts(false,true) failed, first diagnostic: {:?}",
                            diags.first()
                        );
                    }
                    let _ = black_box(result);
                });
            },
        );
    }
    analyze_group.finish();
}

criterion_group!(benches, bench_compile_with_opts);
criterion_main!(benches);
