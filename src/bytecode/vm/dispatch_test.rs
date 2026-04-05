use crate::{
    bytecode::{bytecode::Bytecode, op_code::OpCode, vm::VM},
    runtime::{
        closure::Closure,
        compiled_function::CompiledFunction,
        cons_cell::ConsCell,
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

fn make_test_closure(
    instructions: Vec<u8>,
    num_parameters: usize,
    num_locals: usize,
) -> Rc<Closure> {
    Rc::new(Closure::new(
        Rc::new(CompiledFunction::new(
            instructions,
            num_locals,
            num_parameters,
            None,
        )),
        vec![],
    ))
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
fn dispatch_op_aether_drop_local_preserves_immediate_values() {
    let mut vm = new_vm();
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 0);
    vm.stack_set(0, Value::Integer(20));
    vm.sp = 1;

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpAetherDropLocal as u8, 0],
            0,
            OpCode::OpAetherDropLocal,
        )
        .unwrap();

    assert_eq!(advance, 2);
    assert_eq!(vm.stack_get(0), Value::Integer(20));
}

#[test]
fn dispatch_op_aether_drop_local_clears_boxed_values() {
    let mut vm = new_vm();
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 0);
    vm.stack_set(0, Value::String(Rc::new("boxed".to_string())));
    vm.sp = 1;

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpAetherDropLocal as u8, 0],
            0,
            OpCode::OpAetherDropLocal,
        )
        .unwrap();

    assert_eq!(advance, 2);
    assert_eq!(vm.stack_get(0), Value::None);
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
fn dispatch_op_add_locals_pushes_sum() {
    let mut vm = new_vm();
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 0);
    vm.stack_set(0, Value::Integer(7));
    vm.stack_set(1, Value::Integer(5));
    vm.sp = 2;

    let advance = vm
        .dispatch_instruction(&[OpCode::OpAddLocals as u8, 0, 1], 0, OpCode::OpAddLocals)
        .unwrap();

    assert_eq!(advance, 3);
    assert_eq!(vm.pop().unwrap(), Value::Integer(12));
}

#[test]
fn dispatch_op_sub_locals_pushes_difference() {
    let mut vm = new_vm();
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 0);
    vm.stack_set(0, Value::Integer(9));
    vm.stack_set(1, Value::Integer(4));
    vm.sp = 2;

    let advance = vm
        .dispatch_instruction(&[OpCode::OpSubLocals as u8, 0, 1], 0, OpCode::OpSubLocals)
        .unwrap();

    assert_eq!(advance, 3);
    assert_eq!(vm.pop().unwrap(), Value::Integer(5));
}

#[test]
fn dispatch_op_get_local_get_local_preserves_order() {
    let mut vm = new_vm();
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 0);
    vm.stack_set(0, Value::Integer(3));
    vm.stack_set(1, Value::Integer(8));
    vm.sp = 2;

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpGetLocalGetLocal as u8, 1, 0],
            0,
            OpCode::OpGetLocalGetLocal,
        )
        .unwrap();

    assert_eq!(advance, 3);
    assert_eq!(vm.pop().unwrap(), Value::Integer(3));
    assert_eq!(vm.pop().unwrap(), Value::Integer(8));
}

#[test]
fn dispatch_op_constant_add_uses_existing_left_operand() {
    let mut vm = new_vm();
    vm.constants.push(super::slot::to_slot(Value::Integer(4)));
    vm.push(Value::Integer(6)).unwrap();

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpConstantAdd as u8, 0, 0],
            0,
            OpCode::OpConstantAdd,
        )
        .unwrap();

    assert_eq!(advance, 3);
    assert_eq!(vm.pop().unwrap(), Value::Integer(10));
}

#[test]
fn dispatch_op_get_local_index_reads_collection_from_local() {
    let mut vm = new_vm();
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 0);
    vm.stack_set(
        0,
        Value::Array(Rc::new(vec![Value::Integer(10), Value::Integer(20)])),
    );
    vm.sp = 1;
    vm.push(Value::Integer(1)).unwrap();

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpGetLocalIndex as u8, 0],
            0,
            OpCode::OpGetLocalIndex,
        )
        .unwrap();

    assert_eq!(advance, 2);
    assert_eq!(vm.pop().unwrap(), Value::Some(Rc::new(Value::Integer(20))));
}

#[test]
fn dispatch_op_get_local_is_adt_pushes_match_result() {
    let mut vm = new_vm();
    vm.constants
        .push(super::slot::to_slot(Value::String(Rc::new(
            "Node".to_string(),
        ))));
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 0);
    vm.stack_set(
        0,
        Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("Node".to_string()),
            fields: AdtFields::from_vec(vec![Value::Integer(1)]),
        })),
    );
    vm.sp = 1;

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpGetLocalIsAdt as u8, 0, 0, 0],
            0,
            OpCode::OpGetLocalIsAdt,
        )
        .unwrap();

    assert_eq!(advance, 4);
    assert_eq!(vm.pop().unwrap(), Value::Boolean(true));
}

#[test]
fn dispatch_op_set_local_pop_sets_local_and_discards_previous_tos() {
    let mut vm = new_vm();
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 1);
    vm.stack_set(0, Value::Integer(999));
    vm.stack_set(1, Value::Integer(0));
    vm.sp = 2;
    vm.push(Value::Integer(10)).unwrap();
    vm.push(Value::Integer(42)).unwrap();

    let advance = vm
        .dispatch_instruction(&[OpCode::OpSetLocalPop as u8, 0], 0, OpCode::OpSetLocalPop)
        .unwrap();

    assert_eq!(advance, 2);
    assert_eq!(vm.sp, 2);
    assert_eq!(vm.stack_get(1), Value::Integer(42));
}

#[test]
fn dispatch_op_call_variants_push_new_frame() {
    let mut vm = new_vm();
    let zero = make_test_closure(vec![OpCode::OpReturn as u8], 0, 0);
    vm.push(Value::Closure(zero)).unwrap();
    let advance0 = vm
        .dispatch_instruction(&[OpCode::OpCall0 as u8], 0, OpCode::OpCall0)
        .unwrap();
    assert_eq!(advance0, 1);
    assert_eq!(vm.frame_index, 1);

    let mut vm = new_vm();
    let one = make_test_closure(vec![OpCode::OpReturnLocal as u8, 0], 1, 1);
    vm.push(Value::Closure(one)).unwrap();
    vm.push(Value::Integer(7)).unwrap();
    let advance1 = vm
        .dispatch_instruction(&[OpCode::OpCall1 as u8], 0, OpCode::OpCall1)
        .unwrap();
    assert_eq!(advance1, 1);
    assert_eq!(vm.frame_index, 1);

    let mut vm = new_vm();
    let two = make_test_closure(vec![OpCode::OpReturnLocal as u8, 0], 2, 2);
    vm.push(Value::Closure(two)).unwrap();
    vm.push(Value::Integer(7)).unwrap();
    vm.push(Value::Integer(8)).unwrap();
    let advance2 = vm
        .dispatch_instruction(&[OpCode::OpCall2 as u8], 0, OpCode::OpCall2)
        .unwrap();
    assert_eq!(advance2, 1);
    assert_eq!(vm.frame_index, 1);
}

#[test]
fn dispatch_op_get_local_call1_reorders_local_callee_and_existing_arg() {
    let mut vm = new_vm();
    let closure = make_test_closure(vec![OpCode::OpReturnLocal as u8, 0], 1, 1);
    vm.frames[0] = Frame::new(vm.frames[0].closure.clone(), 0);
    vm.stack_set(0, Value::Closure(closure));
    vm.sp = 1;
    vm.push(Value::Integer(11)).unwrap();

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpGetLocalCall1 as u8, 0],
            0,
            OpCode::OpGetLocalCall1,
        )
        .unwrap();

    assert_eq!(advance, 2);
    assert_eq!(vm.frame_index, 1);
}

#[test]
fn dispatch_op_tail_call1_reuses_current_frame() {
    let mut vm = new_vm();
    let current = make_test_closure(vec![OpCode::OpReturnLocal as u8, 0], 1, 1);
    let replacement = make_test_closure(vec![OpCode::OpReturnLocal as u8, 0], 1, 1);
    vm.frames[0] = Frame::new(current, 0);
    vm.frame_index = 0;
    vm.stack_set(0, Value::Integer(1));
    vm.sp = 1;
    vm.push(Value::Closure(replacement.clone())).unwrap();
    vm.push(Value::Integer(99)).unwrap();

    let advance = vm
        .dispatch_instruction(&[OpCode::OpTailCall1 as u8], 0, OpCode::OpTailCall1)
        .unwrap();

    assert_eq!(advance, 0);
    assert_eq!(vm.frame_index, 0);
    assert!(Rc::ptr_eq(&vm.current_frame().closure, &replacement));
    assert_eq!(vm.stack_get(0), Value::Integer(99));
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
    assert_eq!(adt_val.adt_constructor(), Some("Node"));
    assert_eq!(
        adt_val.adt_clone_two_fields(),
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

#[test]
fn dispatch_op_reuse_some_reuses_unique_wrapper_allocation() {
    let mut vm = new_vm();
    let original = Rc::new(Value::Integer(10));
    let original_ptr = Rc::as_ptr(&original);
    vm.push(Value::Some(original)).unwrap();
    vm.push(Value::Integer(42)).unwrap();

    let advance = vm
        .dispatch_instruction(&[OpCode::OpReuseSome as u8], 0, OpCode::OpReuseSome)
        .unwrap();

    assert_eq!(advance, 1);
    let result = vm.pop().unwrap();
    match result {
        Value::Some(rc) => {
            assert_eq!(Rc::as_ptr(&rc), original_ptr);
            assert_eq!(*rc, Value::Integer(42));
        }
        other => panic!("expected Some result, got {other:?}"),
    }
}

#[test]
fn dispatch_op_reuse_some_allocates_fresh_when_wrapper_is_shared() {
    let mut vm = new_vm();
    let original = Rc::new(Value::Integer(10));
    let _shared = original.clone();
    vm.push(Value::Some(original.clone())).unwrap();
    vm.push(Value::Integer(42)).unwrap();

    let advance = vm
        .dispatch_instruction(&[OpCode::OpReuseSome as u8], 0, OpCode::OpReuseSome)
        .unwrap();

    assert_eq!(advance, 1);
    let result = vm.pop().unwrap();
    match result {
        Value::Some(rc) => {
            assert!(!Rc::ptr_eq(&rc, &original));
            assert_eq!(*rc, Value::Integer(42));
        }
        other => panic!("expected Some result, got {other:?}"),
    }
}

#[test]
fn dispatch_op_reuse_left_reuses_unique_wrapper_allocation() {
    let mut vm = new_vm();
    let original = Rc::new(Value::Integer(10));
    let original_ptr = Rc::as_ptr(&original);
    vm.push(Value::Left(original)).unwrap();
    vm.push(Value::Integer(42)).unwrap();

    let advance = vm
        .dispatch_instruction(&[OpCode::OpReuseLeft as u8], 0, OpCode::OpReuseLeft)
        .unwrap();

    assert_eq!(advance, 1);
    let result = vm.pop().unwrap();
    match result {
        Value::Left(rc) => {
            assert_eq!(Rc::as_ptr(&rc), original_ptr);
            assert_eq!(*rc, Value::Integer(42));
        }
        other => panic!("expected Left result, got {other:?}"),
    }
}

#[test]
fn dispatch_op_reuse_right_reuses_unique_wrapper_allocation() {
    let mut vm = new_vm();
    let original = Rc::new(Value::Integer(10));
    let original_ptr = Rc::as_ptr(&original);
    vm.push(Value::Right(original)).unwrap();
    vm.push(Value::Integer(42)).unwrap();

    let advance = vm
        .dispatch_instruction(&[OpCode::OpReuseRight as u8], 0, OpCode::OpReuseRight)
        .unwrap();

    assert_eq!(advance, 1);
    let result = vm.pop().unwrap();
    match result {
        Value::Right(rc) => {
            assert_eq!(Rc::as_ptr(&rc), original_ptr);
            assert_eq!(*rc, Value::Integer(42));
        }
        other => panic!("expected Right result, got {other:?}"),
    }
}

#[test]
fn dispatch_op_reuse_cons_reuses_unique_allocation() {
    let mut vm = new_vm();
    let original = Rc::new(ConsCell {
        head: Value::Integer(1),
        tail: Value::None,
    });
    let original_ptr = Rc::as_ptr(&original);
    vm.push(Value::Cons(original)).unwrap();
    vm.push(Value::Integer(10)).unwrap();
    vm.push(Value::Integer(20)).unwrap();

    let advance = vm
        .dispatch_instruction(&[OpCode::OpReuseCons as u8, 0xFF], 0, OpCode::OpReuseCons)
        .unwrap();

    assert_eq!(advance, 2);
    let result = vm.pop().unwrap();
    match result {
        Value::Cons(rc) => {
            assert_eq!(Rc::as_ptr(&rc), original_ptr);
            assert_eq!(rc.head, Value::Integer(10));
            assert_eq!(rc.tail, Value::Integer(20));
        }
        other => panic!("expected Cons result, got {other:?}"),
    }
}

#[test]
fn dispatch_op_reuse_adt_reuses_unique_allocation() {
    let mut vm = new_vm();
    vm.constants
        .push(super::slot::to_slot(Value::String(Rc::new(
            "Node".to_string(),
        ))));

    let original = Rc::new(AdtValue {
        constructor: Rc::new("Old".to_string()),
        fields: AdtFields::from_vec(vec![Value::Integer(1), Value::Integer(2)]),
    });
    let original_ptr = Rc::as_ptr(&original);
    vm.push(Value::Adt(original)).unwrap();
    vm.push(Value::Integer(10)).unwrap();
    vm.push(Value::Integer(20)).unwrap();

    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpReuseAdt as u8, 0, 0, 2, 0xFF],
            0,
            OpCode::OpReuseAdt,
        )
        .unwrap();

    assert_eq!(advance, 5);
    let result = vm.pop().unwrap();
    match result {
        Value::Adt(rc) => {
            assert_eq!(Rc::as_ptr(&rc), original_ptr);
            assert_eq!(rc.constructor.as_ref(), "Node");
            assert_eq!(
                rc.fields.clone().into_two(),
                Some((Value::Integer(10), Value::Integer(20)))
            );
        }
        other => panic!("expected Adt result, got {other:?}"),
    }
}

#[test]
fn dispatch_op_reuse_cons_mask_preserves_unchanged_fields() {
    let mut vm = new_vm();
    let original = Rc::new(ConsCell {
        head: Value::Integer(1),
        tail: Value::Integer(2),
    });
    let original_ptr = Rc::as_ptr(&original);
    vm.push(Value::Cons(original)).unwrap();
    vm.push(Value::Integer(10)).unwrap();
    vm.push(Value::Integer(20)).unwrap();

    // Update head only; tail should remain unchanged from the reused allocation.
    let advance = vm
        .dispatch_instruction(&[OpCode::OpReuseCons as u8, 0b01], 0, OpCode::OpReuseCons)
        .unwrap();

    assert_eq!(advance, 2);
    let result = vm.pop().unwrap();
    match result {
        Value::Cons(rc) => {
            assert_eq!(Rc::as_ptr(&rc), original_ptr);
            assert_eq!(rc.head, Value::Integer(10));
            assert_eq!(rc.tail, Value::Integer(2));
        }
        other => panic!("expected Cons result, got {other:?}"),
    }
}

#[test]
fn dispatch_op_reuse_adt_mask_preserves_unchanged_fields() {
    let mut vm = new_vm();
    vm.constants
        .push(super::slot::to_slot(Value::String(Rc::new(
            "Node".to_string(),
        ))));

    let original = Rc::new(AdtValue {
        constructor: Rc::new("Node".to_string()),
        fields: AdtFields::from_vec(vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
        ]),
    });
    let original_ptr = Rc::as_ptr(&original);
    vm.push(Value::Adt(original)).unwrap();
    vm.push(Value::Integer(10)).unwrap();
    vm.push(Value::Integer(20)).unwrap();
    vm.push(Value::Integer(30)).unwrap();

    // Update fields 0 and 2 only; field 1 should remain unchanged.
    let advance = vm
        .dispatch_instruction(
            &[OpCode::OpReuseAdt as u8, 0, 0, 3, 0b101],
            0,
            OpCode::OpReuseAdt,
        )
        .unwrap();

    assert_eq!(advance, 5);
    let result = vm.pop().unwrap();
    match result {
        Value::Adt(rc) => {
            assert_eq!(Rc::as_ptr(&rc), original_ptr);
            assert_eq!(rc.constructor.as_ref(), "Node");
            assert_eq!(
                rc.fields.clone().into_iter().collect::<Vec<_>>(),
                vec![Value::Integer(10), Value::Integer(2), Value::Integer(30)]
            );
        }
        other => panic!("expected Adt result, got {other:?}"),
    }
}

#[test]
fn dispatch_op_is_unique_reports_heap_sharing_correctly() {
    let mut vm = new_vm();

    let unique = Rc::new(Value::Integer(7));
    vm.push(Value::Some(unique)).unwrap();
    let advance = vm
        .dispatch_instruction(&[OpCode::OpIsUnique as u8], 0, OpCode::OpIsUnique)
        .unwrap();
    assert_eq!(advance, 1);
    assert_eq!(vm.pop().unwrap(), Value::Boolean(true));
    assert!(matches!(vm.pop().unwrap(), Value::Some(_)));

    let shared = Rc::new(Value::Integer(9));
    let _extra_ref = shared.clone();
    vm.push(Value::Some(shared)).unwrap();
    let advance = vm
        .dispatch_instruction(&[OpCode::OpIsUnique as u8], 0, OpCode::OpIsUnique)
        .unwrap();
    assert_eq!(advance, 1);
    assert_eq!(vm.pop().unwrap(), Value::Boolean(false));
    assert!(matches!(vm.pop().unwrap(), Value::Some(_)));
}
