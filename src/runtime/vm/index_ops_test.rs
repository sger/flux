use std::collections::HashMap;

use crate::{
    bytecode::bytecode::Bytecode,
    runtime::{hash_key::HashKey, object::Object, vm::VM},
};

fn new_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

#[test]
fn array_index_in_bounds() {
    let mut vm = new_vm();
    let array = Object::Array(vec![Object::Integer(1), Object::Integer(2)].into());
    let index = Object::Integer(1);

    vm.execute_index_expression(array, index).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::Some(std::rc::Rc::new(Object::Integer(2))));
}

#[test]
fn array_index_out_of_bounds() {
    let mut vm = new_vm();
    let array = Object::Array(vec![Object::Integer(1)].into());
    let index = Object::Integer(5);

    vm.execute_index_expression(array, index).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::None);
}

#[test]
fn array_index_negative() {
    let mut vm = new_vm();
    let array = Object::Array(vec![Object::Integer(1)].into());
    let index = Object::Integer(-1);

    vm.execute_index_expression(array, index).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::None);
}

#[test]
fn hash_index_missing_key() {
    let mut vm = new_vm();
    let mut map = HashMap::new();
    map.insert(HashKey::String("k".to_string()), Object::Integer(1));
    let hash = Object::Hash(map.into());

    vm.execute_index_expression(hash, Object::String("missing".to_string().into()))
        .unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Object::None);
}

#[test]
fn invalid_index_errors() {
    let mut vm = new_vm();
    let left = Object::Integer(1);
    let index = Object::Integer(0);

    assert!(vm.execute_index_expression(left, index).is_err());
}
