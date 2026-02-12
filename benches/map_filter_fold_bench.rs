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

fn build_map_program(n: usize) -> String {
    format!("map({}, fun(x) {{ x * 2; }});", build_array_literal(n))
}

fn build_filter_program(n: usize) -> String {
    format!(
        "filter({}, fun(x) {{ x % 2 == 0; }});",
        build_array_literal(n)
    )
}

fn build_fold_program(n: usize) -> String {
    format!(
        "fold({}, 0, fun(acc, x) {{ acc + x; }});",
        build_array_literal(n)
    )
}

fn build_chain_program(n: usize) -> String {
    format!(
        r#"
let data = {};
let mapped = map(data, fun(x) {{ x * 2; }});
let filtered = filter(mapped, fun(x) {{ x % 3 == 0; }});
fold(filtered, 0, fun(acc, x) {{ acc + x; }});
"#,
        build_array_literal(n)
    )
}

fn build_scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "map_100",
            source: build_map_program(100),
            key_ops: 100,
        },
        Scenario {
            name: "map_1k",
            source: build_map_program(1_000),
            key_ops: 1_000,
        },
        Scenario {
            name: "map_2k",
            source: build_map_program(2_000),
            key_ops: 2_000,
        },
        Scenario {
            name: "filter_100",
            source: build_filter_program(100),
            key_ops: 100,
        },
        Scenario {
            name: "filter_1k",
            source: build_filter_program(1_000),
            key_ops: 1_000,
        },
        Scenario {
            name: "filter_2k",
            source: build_filter_program(2_000),
            key_ops: 2_000,
        },
        Scenario {
            name: "fold_100",
            source: build_fold_program(100),
            key_ops: 100,
        },
        Scenario {
            name: "fold_1k",
            source: build_fold_program(1_000),
            key_ops: 1_000,
        },
        Scenario {
            name: "fold_2k",
            source: build_fold_program(2_000),
            key_ops: 2_000,
        },
        Scenario {
            name: "map_filter_fold_chain_100",
            source: build_chain_program(100),
            key_ops: 300,
        },
        Scenario {
            name: "map_filter_fold_chain_1k",
            source: build_chain_program(1_000),
            key_ops: 3_000,
        },
        Scenario {
            name: "map_filter_fold_chain_2k",
            source: build_chain_program(2_000),
            key_ops: 6_000,
        },
    ]
}

fn bench_map_filter_fold(c: &mut Criterion) {
    let scenarios = build_scenarios();
    let mut group = c.benchmark_group("vm/map_filter_fold");

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

criterion_group!(benches, bench_map_filter_fold);
criterion_main!(benches);
