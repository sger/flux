use std::fmt::Write;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::syntax::lexer::Lexer;
use flux::syntax::token_type::TokenType;

struct Corpus {
    name: &'static str,
    source: String,
}

fn build_mixed_syntax_corpus() -> String {
    let mut src = String::with_capacity(256_000);

    for i in 0..2_000usize {
        let _ = writeln!(src, "let value_{i} = {} + {} * ({} - 1);", i, i + 1, i + 2);
        let _ = writeln!(
            src,
            "if value_{i} >= 10 && value_{i} != 42 {{ value_{i}; }} else {{ 0; }}",
        );
        let _ = writeln!(
            src,
            "{{ let nested_{i} = [1, 2, 3, value_{i}]; nested_{i}[0]; }}"
        );
    }

    src
}

fn build_comment_heavy_corpus() -> String {
    let mut src = String::with_capacity(256_000);

    for i in 0..3_000usize {
        let _ = writeln!(src, "// line comment {i}");
        let _ = writeln!(src, "let x_{i} = {i}; // trailing comment");
        let _ = writeln!(src, "/* block comment {} {} */", i, i + 1);
        let _ = writeln!(src, "/// doc line comment {i}");
        let _ = writeln!(src, "/** doc block comment {i} */");
    }

    src
}

fn build_identifier_heavy_corpus() -> String {
    let mut src = String::with_capacity(256_000);

    for i in 0..4_000usize {
        let _ = writeln!(
            src,
            "let very_long_identifier_name_{i}_with_suffix = another_identifier_{i};",
        );
        let _ = writeln!(
            src,
            "let combined_identifier_{i} = very_long_identifier_name_{i}_with_suffix + another_identifier_{i};",
        );
    }

    src
}

fn build_string_heavy_corpus() -> String {
    let mut src = String::with_capacity(256_000);

    for i in 0..2_500usize {
        let _ = writeln!(
            src,
            "let s_{i} = \"line\\n\\tquote:\\\" slash:\\\\ hash:\\# value #{{user_{i}}} done\";",
        );
        let _ = writeln!(
            src,
            "let msg_{i} = \"prefix #{{name_{i}}} middle #{{count_{i} + 1}} suffix\";",
        );
    }

    src
}

fn build_corpora() -> Vec<Corpus> {
    vec![
        Corpus {
            name: "mixed_syntax",
            source: build_mixed_syntax_corpus(),
        },
        Corpus {
            name: "comment_heavy",
            source: build_comment_heavy_corpus(),
        },
        Corpus {
            name: "identifier_heavy",
            source: build_identifier_heavy_corpus(),
        },
        Corpus {
            name: "string_escape_interp_heavy",
            source: build_string_heavy_corpus(),
        },
    ]
}

fn lex_with_tokenize(input: &str) -> usize {
    let mut lexer = Lexer::new(input);
    let tokens = lexer.tokenize();
    tokens.len()
}

fn lex_with_next_token_loop(input: &str) -> usize {
    let mut lexer = Lexer::new(input);
    let mut token_count = 0usize;

    loop {
        let token = lexer.next_token();
        token_count += 1;
        if token.token_type == TokenType::Eof {
            break;
        }
    }

    token_count
}

#[allow(clippy::needless_as_bytes)]
fn bench_lexer_tokenize(c: &mut Criterion) {
    let corpora = build_corpora();
    let mut group = c.benchmark_group("lexer/tokenize");

    for corpus in &corpora {
        let input = corpus.source.as_str();
        group.throughput(Throughput::Bytes(input.as_bytes().len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(corpus.name),
            input,
            |b, input| {
                b.iter(|| {
                    let token_count = lex_with_tokenize(black_box(input));
                    black_box(token_count);
                });
            },
        );
    }

    group.finish();
}

#[allow(clippy::needless_as_bytes)]
fn bench_lexer_next_token_loop(c: &mut Criterion) {
    let corpora = build_corpora();
    let mut group = c.benchmark_group("lexer/next_token_loop");

    for corpus in &corpora {
        let input = corpus.source.as_str();
        group.throughput(Throughput::Bytes(input.as_bytes().len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(corpus.name),
            input,
            |b, input| {
                b.iter(|| {
                    let token_count = lex_with_next_token_loop(black_box(input));
                    black_box(token_count);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_lexer_tokenize, bench_lexer_next_token_loop);
criterion_main!(benches);
