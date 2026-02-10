use std::rc::Rc;

use crate::{
    bytecode::bytecode::Bytecode,
    runtime::{
        builtins::get_builtin, closure::Closure, compiled_function::CompiledFunction,
        object::Object, vm::VM,
    },
};

fn new_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

#[test]
fn call_builtin_len() {
    let mut vm = new_vm();
    let builtin = get_builtin("len").expect("len builtin").clone();
    vm.push(Object::Builtin(builtin)).unwrap();
    vm.push(Object::Array(
        vec![Object::Integer(1), Object::Integer(2)].into(),
    ))
    .unwrap();

    vm.execute_call(1).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::Integer(2));
}

#[test]
fn call_closure_updates_frame_and_stack() {
    let mut vm = new_vm();
    let function = CompiledFunction::new(vec![], 2, 0, None);
    let closure = Closure::new(Rc::new(function), vec![]);
    vm.push(Object::Closure(Rc::new(closure))).unwrap();

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
    vm.push(Object::Closure(Rc::new(closure))).unwrap();

    let err = vm.execute_call(0).unwrap_err();
    assert!(err.contains("wrong number of arguments"));
}
