use crate::{
    bytecode::bytecode::Bytecode,
    bytecode::op_code::OpCode,
    runtime::object::Object,
    runtime::vm::VM,
};

fn new_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

#[test]
fn add_integers() {
    let mut vm = new_vm();
    vm.push(Object::Integer(2)).unwrap();
    vm.push(Object::Integer(3)).unwrap();

    vm.execute_binary_operation(OpCode::OpAdd).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::Integer(5));
}

#[test]
fn add_mixed_numbers() {
    let mut vm = new_vm();
    vm.push(Object::Integer(2)).unwrap();
    vm.push(Object::Float(3.5)).unwrap();

    vm.execute_binary_operation(OpCode::OpAdd).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::Float(5.5));
}

#[test]
fn concat_strings() {
    let mut vm = new_vm();
    vm.push(Object::String("Hello, ".to_string())).unwrap();
    vm.push(Object::String("world".to_string())).unwrap();

    vm.execute_binary_operation(OpCode::OpAdd).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::String("Hello, world".to_string()));
}

#[test]
fn division_by_zero_errors() {
    let mut vm = new_vm();
    vm.push(Object::Integer(10)).unwrap();
    vm.push(Object::Integer(0)).unwrap();

    let err = vm.execute_binary_operation(OpCode::OpDiv).unwrap_err();
    assert!(err.to_lowercase().contains("division by zero"));
}

#[test]
fn invalid_operation_errors() {
    let mut vm = new_vm();
    vm.push(Object::String("oops".to_string())).unwrap();
    vm.push(Object::Integer(1)).unwrap();

    assert!(vm.execute_binary_operation(OpCode::OpSub).is_err());
}
