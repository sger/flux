use std::fmt::Write;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::bytecode::{bytecode::Bytecode, compiler::Compiler};
use flux::runtime::vm::VM;
use flux::diagnostics::render_diagnostics;
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

fn build_local_access_program(size: usize, calls: usize) -> String {
    let mut src = String::with_capacity(size * 8 + calls * 28 + 256);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(src, "let use_local = fun(x) {{ len(x); }};");
    for _ in 0..calls {
        let _ = writeln!(src, "use_local(payload);");
    }
    src
}

fn build_global_access_program(size: usize, calls: usize) -> String {
    let mut src = String::with_capacity(size * 8 + calls * 16 + 256);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(src, "let read_global = fun() {{ len(payload); }};");
    for _ in 0..calls {
        let _ = writeln!(src, "read_global();");
    }
    src
}

fn build_free_access_program(size: usize, calls: usize) -> String {
    let mut src = String::with_capacity(size * 8 + calls * 16 + 320);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(
        src,
        "let make_reader = fun() {{ let captured = payload; fun() {{ len(captured); }}; }};"
    );
    let _ = writeln!(src, "let read_free = make_reader();");
    for _ in 0..calls {
        let _ = writeln!(src, "read_free();");
    }
    src
}

fn build_argument_passthrough_program(size: usize, calls: usize) -> String {
    let mut src = String::with_capacity(size * 8 + calls * 24 + 384);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(src, "let id = fun(x) {{ x; }};");
    let _ = writeln!(src, "let pass = fun(x) {{ id(x); }};");
    for _ in 0..calls {
        let _ = writeln!(src, "len(pass(payload));");
    }
    src
}

fn build_scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "op_get_local_array_1k_x512",
            source: build_local_access_program(1_000, 512),
            key_ops: 512,
        },
        Scenario {
            name: "op_get_global_array_1k_x512",
            source: build_global_access_program(1_000, 512),
            key_ops: 512,
        },
        Scenario {
            name: "op_get_free_array_1k_x512",
            source: build_free_access_program(1_000, 512),
            key_ops: 512,
        },
        Scenario {
            name: "arg_passthrough_array_1k_x512",
            source: build_argument_passthrough_program(1_000, 512),
            key_ops: 512,
        },
    ]
}

fn bench_zero_copy_value(c: &mut Criterion) {
    let scenarios = build_scenarios();
    let mut group = c.benchmark_group("vm/zero_copy_value");

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

criterion_group!(benches, bench_zero_copy_value);
criterion_main!(benches);
