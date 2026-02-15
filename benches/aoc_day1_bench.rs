use std::fs;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::bytecode::{bytecode::Bytecode, compiler::Compiler};
use flux::diagnostics::render_diagnostics;
use flux::runtime::vm::VM;
use flux::syntax::{lexer::Lexer, parser::Parser};

const DAY1_SOURCE_PATH: &str = "examples/io/aoc_day1.flx";
const DAY1_INPUT_PATH: &str = "examples/io/aoc_day1.txt";

fn compile_program(source: &str) -> Bytecode {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let parser_errors = std::mem::take(&mut parser.errors);
    if !parser_errors.is_empty() {
        panic!("{}", render_diagnostics(&parser_errors, Some(source), None));
    }

    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<bench:aoc_day1>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(source), None)));
    compiler.bytecode()
}

fn run_program(bytecode: Bytecode) {
    let mut vm = VM::new(bytecode);
    vm.run().unwrap();
    black_box(vm.last_popped_stack_elem());
}

fn bench_aoc_day1(c: &mut Criterion) {
    let source = fs::read_to_string(DAY1_SOURCE_PATH).expect("read aoc day1 source");
    let input_size = fs::metadata(DAY1_INPUT_PATH)
        .map(|m| m.len())
        .unwrap_or(source.len() as u64);
    let bytecode = compile_program(&source);

    let mut group = c.benchmark_group("aoc/day1");
    group.throughput(Throughput::Bytes(input_size));

    group.bench_with_input(
        BenchmarkId::new("compile_execute", "day1"),
        &source,
        |b, src| {
            b.iter(|| {
                let bytecode = compile_program(black_box(src));
                run_program(bytecode);
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("execute_only", "day1"),
        &bytecode,
        |b, bc| {
            b.iter(|| {
                run_program(black_box(bc.clone()));
            });
        },
    );

    group.finish();
}

criterion_group!(benches, bench_aoc_day1);
criterion_main!(benches);
