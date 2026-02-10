use crate::{
    bytecode::bytecode::Bytecode, bytecode::op_code::OpCode, runtime::value::Value, runtime::vm::VM,
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
    vm.push(Value::Integer(2)).unwrap();
    vm.push(Value::Integer(1)).unwrap();

    vm.execute_comparison(OpCode::OpGreaterThan).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn compare_floats() {
    let mut vm = new_vm();
    vm.push(Value::Float(1.0)).unwrap();
    vm.push(Value::Float(2.0)).unwrap();

    vm.execute_comparison(OpCode::OpLessThanOrEqual).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn compare_strings() {
    let mut vm = new_vm();
    vm.push(Value::String("b".to_string().into())).unwrap();
    vm.push(Value::String("a".to_string().into())).unwrap();

    vm.execute_comparison(OpCode::OpGreaterThan).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn compare_none() {
    let mut vm = new_vm();
    vm.push(Value::None).unwrap();
    vm.push(Value::None).unwrap();

    vm.execute_comparison(OpCode::OpEqual).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn compare_left_right_not_equal() {
    let mut vm = new_vm();
    vm.push(Value::Left(std::rc::Rc::new(Value::Integer(1))))
        .unwrap();
    vm.push(Value::Right(std::rc::Rc::new(Value::Integer(1))))
        .unwrap();

    vm.execute_comparison(OpCode::OpEqual).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::Boolean(false));
}

#[test]
fn invalid_comparison_errors() {
    let mut vm = new_vm();
    vm.push(Value::Integer(1)).unwrap();
    vm.push(Value::String("x".to_string().into())).unwrap();

    assert!(vm.execute_comparison(OpCode::OpGreaterThan).is_err());
}
