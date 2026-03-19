use crate::{
    bytecode::bytecode::Bytecode,
    bytecode::vm::VM,
    runtime::{hamt as rc_hamt, value::Value},
};

use super::type_check::{
    base_is_array, base_is_bool, base_is_float, base_is_hash, base_is_int, base_is_none,
    base_is_some, base_is_string, base_type_of,
};

fn test_vm() -> VM {
    VM::new(Bytecode {
        instructions: vec![],
        constants: vec![],
        debug_info: None,
    })
}

#[test]
fn type_of_returns_type_name() {
    let result = base_type_of(&mut test_vm(), vec![Value::Integer(1)]).unwrap();
    assert_eq!(result, Value::String("Int".to_string().into()));
}

#[test]
fn is_type_checks_values() {
    assert_eq!(
        base_is_int(&mut test_vm(), vec![Value::Integer(1)]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        base_is_float(&mut test_vm(), vec![Value::Float(1.0)]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        base_is_string(&mut test_vm(), vec![Value::String("s".to_string().into())]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        base_is_bool(&mut test_vm(), vec![Value::Boolean(true)]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        base_is_array(&mut test_vm(), vec![Value::Array(vec![].into())]).unwrap(),
        Value::Boolean(true)
    );
    {
        let root = rc_hamt::hamt_empty();
        assert_eq!(
            base_is_hash(&mut test_vm(), vec![Value::HashMap(root)]).unwrap(),
            Value::Boolean(true)
        );
    }
    assert_eq!(
        base_is_none(&mut test_vm(), vec![Value::None]).unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        base_is_some(
            &mut test_vm(),
            vec![Value::Some(std::rc::Rc::new(Value::Integer(1)))]
        )
        .unwrap(),
        Value::Boolean(true)
    );
}
