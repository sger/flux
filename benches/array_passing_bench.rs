use std::fmt::Write;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::bytecode::{bytecode::Bytecode, compiler::Compiler};
use flux::diagnostics::render_diagnostics;
use flux::runtime::vm::VM;
use flux::syntax::{lexer::Lexer, parser::Parser};

struct Scenario {
    name: &'static str,
    source: String,
    key_ops: u64,
}

fn compile_program(source: &str) -> Bytecode {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<bench>", interner);

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

fn build_array_literal(size: usize) -> String {
    let mut out = String::with_capacity(size * 6 + 2);
    out.push('[');
    for i in 0..size {
        if i != 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{}", i);
    }
    out.push(']');
    out
}

fn build_identity_pass_program(size: usize, calls: usize) -> String {
    let mut src = String::with_capacity(size * 8 + calls * 24 + 256);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(src, "let id = fn(x) {{ x; }};");
    for _ in 0..calls {
        let _ = writeln!(src, "len(id(payload));");
    }
    src
}

fn build_chain_pass_program(size: usize, calls: usize) -> String {
    let mut src = String::with_capacity(size * 8 + calls * 24 + 384);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(src, "let f1 = fn(x) {{ x; }};");
    let _ = writeln!(src, "let f2 = fn(x) {{ f1(x); }};");
    let _ = writeln!(src, "let f3 = fn(x) {{ f2(x); }};");
    for _ in 0..calls {
        let _ = writeln!(src, "len(f3(payload));");
    }
    src
}

fn build_scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "array_pass_1k_x256",
            source: build_identity_pass_program(1_000, 256),
            key_ops: 256,
        },
        Scenario {
            name: "array_pass_2k_x256",
            source: build_identity_pass_program(2_000, 256),
            key_ops: 256,
        },
        Scenario {
            name: "array_pass_chain_1k_x256",
            source: build_chain_pass_program(1_000, 256),
            key_ops: 256,
        },
    ]
}

fn bench_array_passing(c: &mut Criterion) {
    let scenarios = build_scenarios();
    let mut group = c.benchmark_group("vm/array_passing");

    for scenario in scenarios {
        let bytecode = compile_program(&scenario.source);
        group.throughput(Throughput::Elements(scenario.key_ops));
        group.throughput(Throughput::Bytes(scenario.source.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(scenario.name),
            &bytecode,
            |b, bytecode| {
                b.iter(|| {
                    run_program(black_box(bytecode.clone()));
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_array_passing);
criterion_main!(benches);
