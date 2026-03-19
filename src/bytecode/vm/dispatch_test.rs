use crate::{
    bytecode::{bytecode::Bytecode, op_code::OpCode, vm::VM},
    runtime::{
        closure::Closure,
        compiled_function::CompiledFunction,
        frame::Frame,
        value::{AdtFields, AdtValue, Value},
    },
};
use std::rc::Rc;

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

    let advance = vm.dispatch_instruction(&[], 0, OpCode::OpTrue).unwrap();

    assert_eq!(advance, 1);
    let result = vm.pop().unwrap();
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn dispatch_fused_cmp_jump_false_jumps_and_consumes_operands() {
    let mut vm = new_vm();
    vm.push(Value::Integer(1)).unwrap();
    vm.push(Value::Integer(2)).unwrap();

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpCmpGtJumpNotTruthy as u8, 0, 9],
            0,
            OpCode::OpCmpGtJumpNotTruthy,
        )
        .unwrap();

    assert_eq!(advance, 0);
    assert_eq!(vm.current_frame().ip, 9);
    assert_eq!(vm.sp, 0);
}

#[test]
fn dispatch_fused_cmp_jump_true_falls_through_and_consumes_operands() {
    let mut vm = new_vm();
    vm.push(Value::Integer(2)).unwrap();
    vm.push(Value::Integer(1)).unwrap();

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpCmpGtJumpNotTruthy as u8, 0, 9],
            0,
            OpCode::OpCmpGtJumpNotTruthy,
        )
        .unwrap();

    assert_eq!(advance, 3);
    assert_eq!(vm.current_frame().ip, 0);
    assert_eq!(vm.sp, 0);
}

#[test]
fn dispatch_fused_cmp_jump_falls_back_for_strings() {
    let mut vm = new_vm();
    vm.push(Value::String(Rc::new("a".to_string()))).unwrap();
    vm.push(Value::String(Rc::new("a".to_string()))).unwrap();

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpCmpEqJumpNotTruthy as u8, 0, 9],
            0,
            OpCode::OpCmpEqJumpNotTruthy,
        )
        .unwrap();

    assert_eq!(advance, 3);
    assert_eq!(vm.sp, 0);
}

#[test]
fn dispatch_op_consume_local0_moves_from_first_local_slot() {
    let mut vm = new_vm();
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 0);
    vm.stack_set(0, Value::Integer(7));
    vm.sp = 1;

    let advance = vm
        .dispatch_instruction(&[OpCode::OpConsumeLocal0 as u8], 0, OpCode::OpConsumeLocal0)
        .unwrap();

    assert_eq!(advance, 1);
    assert_eq!(vm.pop().unwrap(), Value::Integer(7));
    assert!(matches!(vm.stack_get(0), Value::Uninit));
}

#[test]
fn dispatch_op_consume_local1_moves_from_second_local_slot() {
    let mut vm = new_vm();
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 0);
    vm.stack_set(0, Value::Integer(1));
    vm.stack_set(1, Value::Integer(9));
    vm.sp = 2;

    let advance = vm
        .dispatch_instruction(&[OpCode::OpConsumeLocal1 as u8], 0, OpCode::OpConsumeLocal1)
        .unwrap();

    assert_eq!(advance, 1);
    assert_eq!(vm.pop().unwrap(), Value::Integer(9));
    assert!(matches!(vm.stack_get(1), Value::Uninit));
}

#[test]
fn dispatch_op_return_local_moves_value_out_of_frame_slot() {
    let mut vm = new_vm();
    let function = CompiledFunction::new(vec![], 1, 0, None);
    let frame = Frame::new(Rc::new(Closure::new(Rc::new(function), vec![])), 1);
    vm.frames.push(frame);
    vm.frame_index = 1;
    vm.sp = 2;
    vm.stack_set(0, Value::Closure(vm.frames[0].closure.clone()));
    vm.stack_set(1, Value::String(Rc::new("moved".to_string())));

    let advance = vm
        .dispatch_instruction(&[OpCode::OpReturnLocal as u8, 0], 0, OpCode::OpReturnLocal)
        .unwrap();

    assert_eq!(advance, 0);
    assert_eq!(vm.frame_index, 0);
    assert_eq!(vm.sp, 1);
    assert_eq!(vm.stack_get(0), Value::String(Rc::new("moved".to_string())));
}

#[test]
fn dispatch_op_make_adt_moves_fields_from_stack() {
    let mut vm = new_vm();
    vm.constants
        .push(super::slot::to_slot(Value::String(Rc::new(
            "Node".to_string(),
        ))));
    vm.stack_set(0, Value::Integer(1));
    vm.stack_set(1, Value::Integer(2));
    vm.sp = 2;

    let advance = vm
        .dispatch_instruction(&[OpCode::OpMakeAdt as u8, 0, 0, 2], 0, OpCode::OpMakeAdt)
        .unwrap();

    assert_eq!(advance, 4);
    assert_eq!(vm.sp, 1);
    assert!(matches!(vm.stack_get(0), Value::Adt(_)));
    let adt_val = vm.stack_get(0);
    assert_eq!(adt_val.adt_constructor(&vm.gc_heap), Some("Node"));
    assert_eq!(
        adt_val.adt_clone_two_fields(&vm.gc_heap),
        Some((Value::Integer(1), Value::Integer(2)))
    );
    assert!(matches!(vm.stack_get(1), Value::Uninit));
}

#[test]
fn dispatch_op_adt_field_reuses_unshared_adt_payload() {
    let mut vm = new_vm();
    vm.push(Value::Adt(Rc::new(AdtValue {
        constructor: Rc::new("Node".to_string()),
        fields: AdtFields::from_vec(vec![Value::Integer(10), Value::Integer(20)]),
    })))
    .unwrap();

    let advance = vm
        .dispatch_instruction(&[OpCode::OpAdtField as u8, 1], 0, OpCode::OpAdtField)
        .unwrap();

    assert_eq!(advance, 2);
    assert_eq!(vm.pop().unwrap(), Value::Integer(20));
}
