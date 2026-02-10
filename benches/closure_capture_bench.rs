use std::fmt::Write;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::bytecode::{bytecode::Bytecode, compiler::Compiler};
use flux::runtime::vm::VM;
use flux::syntax::{diagnostics::render_diagnostics, lexer::Lexer, parser::Parser};

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

fn build_hash_literal(size: usize) -> String {
    let mut out = String::with_capacity(size * 14 + 2);
    out.push('{');
    for i in 0..size {
        if i != 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "\"k{}\": {}", i, i);
    }
    out.push('}');
    out
}

fn build_array_capture_program(size: usize, calls: usize) -> String {
    let mut src = String::with_capacity(size * 8 + calls * 16 + 256);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(
        src,
        "let make = fun() {{ let captured = payload; fun(i) {{ captured[i]; }}; }};"
    );
    let _ = writeln!(src, "let f = make();");
    for i in 0..calls {
        let idx = i % size;
        let _ = writeln!(src, "f({});", idx);
    }
    src
}

fn build_hash_capture_program(size: usize, calls: usize) -> String {
    let mut src = String::with_capacity(size * 16 + calls * 24 + 256);
    let hash = build_hash_literal(size);
    let _ = writeln!(src, "let payload = {};", hash);
    let _ = writeln!(
        src,
        "let make = fun() {{ let captured = payload; fun(k) {{ captured[k]; }}; }};"
    );
    let _ = writeln!(src, "let f = make();");
    for i in 0..calls {
        let _ = writeln!(src, "f(\"k{}\");", i % size);
    }
    src
}

fn build_nested_capture_program(size: usize, calls: usize) -> String {
    let mut src = String::with_capacity(size * 8 + calls * 16 + 320);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(
        src,
        "let outer = fun() {{ let a = payload; fun() {{ let b = a; fun(i) {{ b[i]; }}; }}; }};"
    );
    let _ = writeln!(src, "let mid = outer();");
    let _ = writeln!(src, "let f = mid();");
    for i in 0..calls {
        let idx = i % size;
        let _ = writeln!(src, "f({});", idx);
    }
    src
}

fn build_string_capture_program(bytes: usize, calls: usize) -> String {
    let mut src = String::with_capacity(bytes + calls * 16 + 256);
    let payload = "x".repeat(bytes);
    let _ = writeln!(src, "let payload = \"{}\";", payload);
    let _ = writeln!(
        src,
        "let make = fun() {{ let captured = payload; fun() {{ len(captured); }}; }};"
    );
    let _ = writeln!(src, "let f = make();");
    for _ in 0..calls {
        let _ = writeln!(src, "f();");
    }
    src
}

fn build_array_capture_only_program(size: usize, creates: usize) -> String {
    let mut src = String::with_capacity(size * 8 + creates * 8 + 256);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(
        src,
        "let make = fun() {{ let captured = payload; fun() {{ captured[0]; }}; }};"
    );
    for _ in 0..creates {
        let _ = writeln!(src, "make();");
    }
    src
}

fn build_array_no_capture_only_program(creates: usize) -> String {
    let mut src = String::with_capacity(creates * 8 + 128);
    let _ = writeln!(src, "let make = fun() {{ fun() {{ 0; }}; }};");
    for _ in 0..creates {
        let _ = writeln!(src, "make();");
    }
    src
}

fn build_array_call_only_program(size: usize, calls: usize) -> String {
    let mut src = String::with_capacity(size * 8 + calls * 16 + 256);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(
        src,
        "let make = fun() {{ let captured = payload; fun(i) {{ captured[i]; }}; }};"
    );
    let _ = writeln!(src, "let f = make();");
    for i in 0..calls {
        let _ = writeln!(src, "f({});", i % size);
    }
    src
}

fn build_array_create_and_call_program(size: usize, runs: usize) -> String {
    let mut src = String::with_capacity(size * 8 + runs * 24 + 512);
    let array = build_array_literal(size);
    let _ = writeln!(src, "let payload = {};", array);
    let _ = writeln!(
        src,
        "let make = fun() {{ let captured = payload; fun(i) {{ captured[i]; }}; }};"
    );
    for i in 0..runs {
        let _ = writeln!(src, "let f{} = make();", i);
        let _ = writeln!(src, "f{}({});", i, i % size);
    }
    src
}

fn build_scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "array_capture_1k",
            source: build_array_capture_program(1_000, 512),
            key_ops: 512,
        },
        Scenario {
            name: "string_capture_64k",
            source: build_string_capture_program(64 * 1024, 512),
            key_ops: 512,
        },
        Scenario {
            name: "hash_capture_1k",
            source: build_hash_capture_program(1_000, 512),
            key_ops: 512,
        },
        Scenario {
            name: "nested_capture_array_1k",
            source: build_nested_capture_program(1_000, 512),
            key_ops: 512,
        },
        Scenario {
            name: "repeated_calls_captured_array",
            source: build_array_capture_program(2_000, 2_048),
            key_ops: 2_048,
        },
        Scenario {
            name: "capture_only_array_1k",
            source: build_array_capture_only_program(1_000, 512),
            key_ops: 512,
        },
        Scenario {
            name: "no_capture_only_baseline",
            source: build_array_no_capture_only_program(512),
            key_ops: 512,
        },
        Scenario {
            name: "call_only_captured_array_1k",
            source: build_array_call_only_program(1_000, 512),
            key_ops: 512,
        },
        Scenario {
            name: "create_and_call_captured_array_1k",
            source: build_array_create_and_call_program(1_000, 512),
            key_ops: 512,
        },
    ]
}

#[allow(clippy::needless_as_bytes)]
fn bench_closure_capture(c: &mut Criterion) {
    let scenarios = build_scenarios();
    let mut group = c.benchmark_group("vm/closure_capture");

    for scenario in scenarios {
        let bytecode = compile_program(&scenario.source);
        group.throughput(Throughput::Elements(scenario.key_ops));
        group.throughput(Throughput::Bytes(scenario.source.as_bytes().len() as u64));
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

criterion_group!(benches, bench_closure_capture);
criterion_main!(benches);
