//! What the effect tape (proposal 0188 S1) costs when it is off.
//!
//! Every primop the VM runs passes the tape's gate, so these loops time the
//! gate on its two kinds of traffic: an effect primop (`now_ms`) and the
//! arithmetic primops a loop is made of. Compare against a baseline saved
//! before the tape existed:
//!
//! ```text
//! cargo bench --bench effect_tape_off_bench -- --save-baseline pre-tape
//! cargo bench --bench effect_tape_off_bench -- --baseline pre-tape
//! ```

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use flux::bytecode::bytecode::Bytecode;
use flux::compiler::Compiler;
use flux::diagnostics::render_diagnostics;
use flux::syntax::{lexer::Lexer, parser::Parser};
use flux::vm::VM;

const CLOCK_LOOP: &str = r#"
fn spin(n, acc) with Clock {
    if n == 0 { acc } else { spin(n - 1, acc + now_ms() % 2) }
}

fn main() with Clock {
    spin(200000, 0)
}
"#;

const ARITH_LOOP: &str = r#"
fn spin(n, acc) {
    if n == 0 { acc } else { spin(n - 1, (acc * 31 + n) % 1000003) }
}

fn main() {
    spin(200000, 0)
}
"#;

fn compile_program(source: &str) -> Bytecode {
    let mut parser = Parser::new(Lexer::new(source));
    let program = parser.parse_program();
    let parser_errors = std::mem::take(&mut parser.errors);
    if !parser_errors.is_empty() {
        panic!("{}", render_diagnostics(&parser_errors, Some(source), None));
    }
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<bench:effect_tape_off>", interner);
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

fn bench_tape_off(c: &mut Criterion) {
    let mut group = c.benchmark_group("effect_tape_off");
    for (name, source) in [("clock_loop", CLOCK_LOOP), ("arith_loop", ARITH_LOOP)] {
        let bytecode = compile_program(source);
        group.bench_function(name, |b| b.iter(|| run_program(black_box(bytecode.clone()))));
    }
    group.finish();
}

criterion_group!(benches, bench_tape_off);
criterion_main!(benches);
