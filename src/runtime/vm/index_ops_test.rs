use crate::{
    bytecode::bytecode::Bytecode,
    runtime::{
        gc::hamt::{hamt_empty, hamt_insert},
        hash_key::HashKey,
        value::Value,
        vm::VM,
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
fn array_index_in_bounds() {
    let mut vm = new_vm();
    let array = Value::Array(vec![Value::Integer(1), Value::Integer(2)].into());
    let index = Value::Integer(1);

    vm.execute_index_expression(array, index).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::Some(std::rc::Rc::new(Value::Integer(2))));
}

#[test]
fn array_index_out_of_bounds() {
    let mut vm = new_vm();
    let array = Value::Array(vec![Value::Integer(1)].into());
    let index = Value::Integer(5);

    vm.execute_index_expression(array, index).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::None);
}

#[test]
fn array_index_negative() {
    let mut vm = new_vm();
    let array = Value::Array(vec![Value::Integer(1)].into());
    let index = Value::Integer(-1);

    vm.execute_index_expression(array, index).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::None);
}

#[test]
fn hash_index_missing_key() {
    let mut vm = new_vm();
    let mut root = hamt_empty(&mut vm.gc_heap);
    root = hamt_insert(
        &mut vm.gc_heap,
        root,
        HashKey::String("k".to_string()),
        Value::Integer(1),
    );
    let hash = Value::Gc(root);

    vm.execute_index_expression(hash, Value::String("missing".to_string().into()))
        .unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::None);
}

#[test]
fn invalid_index_errors() {
    let mut vm = new_vm();
    let left = Value::Integer(1);
    let index = Value::Integer(0);

    assert!(vm.execute_index_expression(left, index).is_err());
}
