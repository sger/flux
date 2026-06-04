//! Tests for `VM::run_top_level` — the persistent-REPL primitive (proposal
//! 0176): one live VM runs the full top-level buffer from a given offset so each
//! REPL line executes only its freshly-appended tail while `globals` persist.

use crate::{
    bytecode::{
        bytecode::Bytecode,
        op_code::{OpCode, make},
    },
    runtime::value::Value,
    vm::VM,
};

fn empty_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

fn chunk(instructions: Vec<u8>, constants: Vec<Value>) -> Bytecode {
    Bytecode {
        instructions,
        constants,
        debug_info: None,
    }
}

fn global_int(vm: &VM, idx: usize) -> i64 {
    match super::slot::from_slot_ref(&vm.globals[idx]) {
        Value::Integer(n) => n,
        other => panic!("expected an integer global at slot {idx}, got {other:?}"),
    }
}

#[test]
fn run_top_level_preserves_globals_and_skips_earlier_code() {
    let mut vm = empty_vm();

    // Line 1: globals[0] = 5.
    let mut buffer = make(OpCode::OpConstant, &[0]);
    buffer.extend(make(OpCode::OpSetGlobal, &[0]));
    vm.run_top_level(chunk(buffer.clone(), vec![Value::Integer(5)]), 0)
        .expect("line 1 runs");
    assert_eq!(global_int(&vm, 0), 5);

    // Line 2 is appended to the same buffer: globals[1] = globals[0] + 1. Running
    // from `start` (the length before line 2) executes only line 2 — line 1 is
    // not re-run — yet it reads the global line 1 stored.
    let start = buffer.len();
    buffer.extend(make(OpCode::OpGetGlobal, &[0]));
    buffer.extend(make(OpCode::OpConstant, &[1]));
    buffer.extend(make(OpCode::OpAdd, &[]));
    buffer.extend(make(OpCode::OpSetGlobal, &[1]));
    vm.run_top_level(
        chunk(buffer, vec![Value::Integer(5), Value::Integer(1)]),
        start,
    )
    .expect("line 2 runs");

    assert_eq!(global_int(&vm, 0), 5);
    assert_eq!(global_int(&vm, 1), 6);
}
