//! Tests for `VM::run_chunk` — the persistent-REPL primitive (proposal 0176):
//! one live VM executes a sequence of independently-compiled top-level chunks
//! while preserving `globals` across them.

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
fn run_chunk_preserves_globals_across_chunks() {
    let mut vm = empty_vm();

    // Chunk 1: globals[0] = 5
    let mut c1 = make(OpCode::OpConstant, &[0]);
    c1.extend(make(OpCode::OpSetGlobal, &[0]));
    vm.run_chunk(chunk(c1, vec![Value::Integer(5)]))
        .expect("chunk 1 runs");
    assert_eq!(global_int(&vm, 0), 5);

    // Chunk 2: globals[1] = globals[0] + 1 — reads the earlier global WITHOUT
    // re-running chunk 1. Chunk 2's constant `1` is appended after chunk 1's
    // constant `5`, so it lands at absolute constant index 1.
    let mut c2 = make(OpCode::OpGetGlobal, &[0]);
    c2.extend(make(OpCode::OpConstant, &[1]));
    c2.extend(make(OpCode::OpAdd, &[]));
    c2.extend(make(OpCode::OpSetGlobal, &[1]));
    vm.run_chunk(chunk(c2, vec![Value::Integer(1)]))
        .expect("chunk 2 runs");

    // The earlier global survived; the new chunk read it and wrote a fresh slot.
    assert_eq!(global_int(&vm, 0), 5);
    assert_eq!(global_int(&vm, 1), 6);
}
