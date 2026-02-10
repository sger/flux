use crate::{
    bytecode::bytecode::Bytecode,
    bytecode::op_code::OpCode,
    runtime::{value::Value, vm::VM},
};

fn new_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

#[test]
fn dispatch_op_true_pushes_boolean() {
    let mut vm = new_vm();

    let advance = vm.dispatch_instruction(0, OpCode::OpTrue).unwrap();

    assert!(advance);
    let result = vm.pop().unwrap();
    assert_eq!(result, Value::Boolean(true));
}
