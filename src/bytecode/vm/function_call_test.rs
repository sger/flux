use std::rc::Rc;

use crate::{
    bytecode::{bytecode::Bytecode, vm::VM},
    runtime::{closure::Closure, compiled_function::CompiledFunction, value::Value},
};

fn new_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

#[test]
fn call_closure_updates_frame_and_stack() {
    let mut vm = new_vm();
    let function = CompiledFunction::new(vec![], 2, 0, None);
    let closure = Closure::new(Rc::new(function), vec![]);
    vm.push(Value::Closure(Rc::new(closure))).unwrap();

    let initial_frame_index = vm.frame_index;
    let initial_sp = vm.sp;

    vm.execute_call(0).unwrap();

    assert_eq!(vm.frame_index, initial_frame_index + 1);
    assert_eq!(vm.sp, initial_sp + 2);
}

#[test]
fn call_closure_wrong_arity_errors() {
    let mut vm = new_vm();
    let function = CompiledFunction::new(vec![], 0, 1, None);
    let closure = Closure::new(Rc::new(function), vec![]);
    vm.push(Value::Closure(Rc::new(closure))).unwrap();

    let err = vm.execute_call(0).unwrap_err();
    assert!(err.contains("wrong number of arguments"));
}

#[test]
fn call_self_uses_current_frame_closure_without_callee_slot() {
    let mut vm = new_vm();
    let function = CompiledFunction::new(vec![], 2, 1, None);
    let closure = Rc::new(Closure::new(Rc::new(function), vec![]));
    vm.frames[0] = crate::runtime::frame::Frame::new(closure, 0);
    vm.push(Value::Integer(5)).unwrap();

    let initial_frame_index = vm.frame_index;
    let initial_sp = vm.sp;

    vm.execute_call_self(1).unwrap();

    assert_eq!(vm.frame_index, initial_frame_index + 1);
    assert_eq!(vm.sp, initial_sp + 2);
}
