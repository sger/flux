use crate::{
    bytecode::bytecode::Bytecode, bytecode::op_code::OpCode, runtime::object::Object,
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
fn compare_integers() {
    let mut vm = new_vm();
    vm.push(Object::Integer(2)).unwrap();
    vm.push(Object::Integer(1)).unwrap();

    vm.execute_comparison(OpCode::OpGreaterThan).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn compare_floats() {
    let mut vm = new_vm();
    vm.push(Object::Float(1.0)).unwrap();
    vm.push(Object::Float(2.0)).unwrap();

    vm.execute_comparison(OpCode::OpLessThanOrEqual).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn compare_strings() {
    let mut vm = new_vm();
    vm.push(Object::String("b".to_string())).unwrap();
    vm.push(Object::String("a".to_string())).unwrap();

    vm.execute_comparison(OpCode::OpGreaterThan).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn compare_none() {
    let mut vm = new_vm();
    vm.push(Object::None).unwrap();
    vm.push(Object::None).unwrap();

    vm.execute_comparison(OpCode::OpEqual).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn compare_left_right_not_equal() {
    let mut vm = new_vm();
    vm.push(Object::Left(Box::new(Object::Integer(1)))).unwrap();
    vm.push(Object::Right(Box::new(Object::Integer(1))))
        .unwrap();

    vm.execute_comparison(OpCode::OpEqual).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::Boolean(false));
}

#[test]
fn invalid_comparison_errors() {
    let mut vm = new_vm();
    vm.push(Object::Integer(1)).unwrap();
    vm.push(Object::String("x".to_string())).unwrap();

    assert!(vm.execute_comparison(OpCode::OpGreaterThan).is_err());
}
