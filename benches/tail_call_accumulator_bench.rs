use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::bytecode::{bytecode::Bytecode, compiler::Compiler};
use flux::runtime::vm::VM;
use flux::diagnostics::render_diagnostics;
use flux::syntax::{lexer::Lexer, parser::Parser};

struct Scenario {
    name: &'static str,
    source: String,
    n: u64,
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

fn build_tail_accumulator_program(n: usize) -> String {
    format!(
        r#"
fun build(n, acc) {{
    if n == 0 {{
        acc;
    }} else {{
        build(n - 1, push(acc, n));
    }}
}}

len(build({}, []));
"#,
        n
    )
}

fn build_scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "build_1k",
            source: build_tail_accumulator_program(1_000),
            n: 1_000,
        },
        Scenario {
            name: "build_5k",
            source: build_tail_accumulator_program(5_000),
            n: 5_000,
        },
        Scenario {
            name: "build_10k",
            source: build_tail_accumulator_program(10_000),
            n: 10_000,
        },
    ]
}

fn bench_tail_call_accumulator(c: &mut Criterion) {
    let scenarios = build_scenarios();
    let mut group = c.benchmark_group("vm/tail_call_accumulator");

    for scenario in scenarios {
        let bytecode = compile_program(&scenario.source);
        group.throughput(Throughput::Elements(scenario.n));
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

criterion_group!(benches, bench_tail_call_accumulator);
criterion_main!(benches);
