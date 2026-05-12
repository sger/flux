//! Microbenchmark for delimited-continuation capture/resume on the VM.
//!
//! Exercises the `OpPerform` → `capture_to_boundary` → `Continuation::compose_pieces`
//! → `execute_resume` round-trip — the same path the async fiber suspend/resume
//! hooks use. A non-tail-resumptive handler forces a real continuation capture
//! on every `perform`; the loop keeps each capture shallow (one frame), which is
//! the steady-state shape we want to stay allocation-free.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::bytecode::bytecode::Bytecode;
use flux::compiler::Compiler;
use flux::diagnostics::render_diagnostics;
use flux::syntax::{lexer::Lexer, parser::Parser};
use flux::vm::VM;

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

/// `rounds * DEPTH` perform/capture/resume cycles. Each `run_one()` does a
/// `DEPTH`-deep chain of performs handled by a non-tail-resumptive arm (so a
/// real continuation is captured on every `perform` and restored on `resume`);
/// the outer `batch` loops `rounds` times in tail position (TCO'd, so native
/// stack stays flat regardless of `rounds`).
const DEPTH: usize = 20;

fn perform_loop_program(rounds: usize) -> String {
    format!(
        r#"
effect Tick {{
    tick: Int -> Int
}}

fn loop_perform(n: Int, acc: Int) -> Int with Tick {{
    if n == 0 {{
        acc
    }} else {{
        let x = perform Tick.tick(n)
        loop_perform(n - 1, acc + x)
    }}
}}

fn run_one() -> Int {{
    loop_perform({DEPTH}, 0) handle Tick {{
        // `+ 0` after `resume` makes the arm non-tail-resumptive: each
        // `perform` captures, and each `resume` restores, a real continuation.
        tick(resume, v) -> resume(1) + 0
    }}
}}

fn batch(rounds: Int) -> Int {{
    if rounds == 0 {{
        0
    }} else {{
        let _ = run_one()
        batch(rounds - 1)
    }}
}}

batch({rounds});
"#
    )
}

fn bench_continuation_capture(c: &mut Criterion) {
    let round_counts = [50usize, 500, 2_500];
    let mut group = c.benchmark_group("vm/continuation_capture");
    for &rounds in &round_counts {
        let bytecode = compile_program(&perform_loop_program(rounds));
        let cycles = (rounds * DEPTH) as u64;
        group.throughput(Throughput::Elements(cycles));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("cycles_{cycles}")),
            &bytecode,
            |b, bytecode| {
                b.iter(|| run_program(black_box(bytecode.clone())));
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_continuation_capture);
criterion_main!(benches);
