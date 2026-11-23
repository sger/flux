use crate::{
    bytecode::bytecode::Bytecode, bytecode::op_code::OpCode, runtime::value::Value, vm::VM,
};

fn new_vm() -> VM {
    VM::new(Bytecode { instructions: vec![], constants: vec![], debug_info: None })
}

#[test]
fn add_integers() {
    let mut vm = new_vm();
    vm.push(Value::Integer(2)).unwrap();
    vm.push(Value::Integer(3)).unwrap();

    vm.execute_binary_operation(OpCode::OpAdd).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::Integer(5));
}

#[test]
fn add_mixed_numbers() {
    let mut vm = new_vm();
    vm.push(Value::Integer(2)).unwrap();
    vm.push(Value::Float(3.5)).unwrap();

    vm.execute_binary_operation(OpCode::OpAdd).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::Float(5.5));
}

#[test]
fn concat_strings() {
    let mut vm = new_vm();
    vm.push(Value::String("Hello, ".to_string().into())).unwrap();
    vm.push(Value::String("world".to_string().into())).unwrap();

    vm.execute_binary_operation(OpCode::OpAdd).unwrap();

    let result = vm.pop().unwrap();
    assert_eq!(result, Value::String("Hello, world".to_string().into()));
}

#[test]
fn division_by_zero_errors() {
    let mut vm = new_vm();
    vm.push(Value::Integer(10)).unwrap();
    vm.push(Value::Integer(0)).unwrap();

    let err = vm.execute_binary_operation(OpCode::OpDiv).unwrap_err();
    assert!(err.to_lowercase().contains("division by zero"));
}

#[test]
fn invalid_operation_errors() {
    let mut vm = new_vm();
    vm.push(Value::String("oops".to_string().into())).unwrap();
    vm.push(Value::Integer(1)).unwrap();

    assert!(vm.execute_binary_operation(OpCode::OpSub).is_err());
}

// KI-038: Int is 64-bit two's complement and `+ - *`, negation and `abs` wrap
// on every VM path. These pass in a build with overflow checks off whatever
// the code does; they catch an unchecked operation in a build with them on.

fn binary(op: OpCode, l: i64, r: i64) -> Value {
    let mut vm = new_vm();
    vm.push(Value::Integer(l)).unwrap();
    vm.push(Value::Integer(r)).unwrap();
    vm.execute_binary_operation(op).unwrap();
    vm.pop().unwrap()
}

#[test]
fn slow_path_add_wraps_ki038() {
    assert_eq!(binary(OpCode::OpAdd, i64::MAX, 1), Value::Integer(i64::MIN));
}

#[test]
fn slow_path_sub_wraps_ki038() {
    assert_eq!(binary(OpCode::OpSub, i64::MIN, 1), Value::Integer(i64::MAX));
}

#[test]
fn slow_path_mul_wraps_ki038() {
    assert_eq!(binary(OpCode::OpMul, i64::MAX, 2), Value::Integer(-2));
}

fn core(op: crate::core::CorePrimOp, args: Vec<i64>) -> Value {
    let mut vm = new_vm();
    let args = args.into_iter().map(Value::Integer).collect();
    super::core_dispatch::execute_core_primop(&mut vm, op, args).unwrap()
}

/// The VM holds every Int the same way, so the boxing primops of native raw
/// Ints return their argument.
#[test]
fn int_boxing_primops_are_the_identity_on_the_vm() {
    use crate::core::CorePrimOp::*;
    for op in [BoxInt, UnboxInt, UnboxIntMove] {
        assert_eq!(core(op, vec![i64::MIN]), Value::Integer(i64::MIN), "{op:?}");
    }
}

#[test]
fn core_primops_wrap_ki038() {
    use crate::core::CorePrimOp::*;
    assert_eq!(core(IAdd, vec![i64::MAX, 1]), Value::Integer(i64::MIN));
    assert_eq!(core(ISub, vec![i64::MIN, 1]), Value::Integer(i64::MAX));
    assert_eq!(core(IMul, vec![i64::MAX, 2]), Value::Integer(-2));
    assert_eq!(core(Neg, vec![i64::MIN]), Value::Integer(i64::MIN));
    assert_eq!(core(Abs, vec![i64::MIN]), Value::Integer(i64::MIN));
}

// KI-040: `MIN / -1` is MIN and `MIN % -1` is 0. Rust's `/` and `%` panic on
// those operands whatever the overflow-check setting.

#[test]
fn slow_path_min_div_neg_one_ki040() {
    assert_eq!(binary(OpCode::OpDiv, i64::MIN, -1), Value::Integer(i64::MIN));
    assert_eq!(binary(OpCode::OpMod, i64::MIN, -1), Value::Integer(0));
}

#[test]
fn core_primops_min_div_neg_one_ki040() {
    use crate::core::CorePrimOp::*;
    assert_eq!(core(IDiv, vec![i64::MIN, -1]), Value::Integer(i64::MIN));
    assert_eq!(core(IMod, vec![i64::MIN, -1]), Value::Integer(0));
    let some = |v| Value::Some(std::rc::Rc::new(Value::Integer(v)));
    assert_eq!(core(SafeDiv, vec![i64::MIN, -1]), some(i64::MIN));
    assert_eq!(core(SafeMod, vec![i64::MIN, -1]), some(0));
}
