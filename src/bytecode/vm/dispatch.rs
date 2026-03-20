use std::{cell::RefCell, rc::Rc};

use crate::{
    bytecode::op_code::OpCode,
    runtime::{
        base::get_base_function_by_index,
        base::list_ops::format_value,
        closure::Closure,
        cons_cell::ConsCell,
        continuation::Continuation,
        handler_arm::HandlerArm,
        handler_frame::HandlerFrame,
        leak_detector,
        value::{AdtFields, AdtValue, Value},
    },
};

use super::VM;
use super::slot;

impl VM {
    #[inline(always)]
    fn read_u8_fast(instructions: &[u8], offset: usize) -> usize {
        debug_assert!(offset < instructions.len());
        // Safety: all callsites derive offsets from in-bounds instruction pointers and
        // operand widths defined by the opcode.
        unsafe { *instructions.get_unchecked(offset) as usize }
    }

    #[inline(always)]
    fn read_u16_fast(instructions: &[u8], offset: usize) -> usize {
        debug_assert!(offset + 1 < instructions.len());
        // Safety: all callsites derive offsets from in-bounds instruction pointers and
        // operand widths defined by the opcode.
        unsafe {
            let hi = *instructions.get_unchecked(offset) as u16;
            let lo = *instructions.get_unchecked(offset + 1) as u16;
            ((hi << 8) | lo) as usize
        }
    }

    #[inline(always)]
    fn read_u32_fast(instructions: &[u8], offset: usize) -> usize {
        debug_assert!(offset + 3 < instructions.len());
        // Safety: all callsites derive offsets from in-bounds instruction pointers and
        // operand widths defined by the opcode.
        unsafe {
            let b0 = *instructions.get_unchecked(offset) as u32;
            let b1 = *instructions.get_unchecked(offset + 1) as u32;
            let b2 = *instructions.get_unchecked(offset + 2) as u32;
            let b3 = *instructions.get_unchecked(offset + 3) as u32;
            ((b0 << 24) | (b1 << 16) | (b2 << 8) | b3) as usize
        }
    }

    #[cold]
    #[inline(never)]
    fn stack_underflow_err() -> String {
        "stack underflow".to_string()
    }

    #[cold]
    #[inline(never)]
    fn expected_some_err(found: &Value) -> String {
        format!("expected Some(..) but found {}", found.type_name())
    }

    #[cold]
    #[inline(never)]
    fn unwrap_left_err() -> String {
        "Cannot unwrap non-Left value".to_string()
    }

    #[cold]
    #[inline(never)]
    fn unwrap_right_err() -> String {
        "Cannot unwrap non-Right value".to_string()
    }

    #[cold]
    #[inline(never)]
    fn negation_type_err(found: &Value) -> String {
        format!("unsupported type for negation: {}", found.type_name())
    }

    #[cold]
    #[inline(never)]
    fn cons_head_type_err(found: &Value) -> String {
        format!("head: expected list, got {}", found.type_name())
    }

    #[cold]
    #[inline(never)]
    fn cons_tail_type_err(found: &Value) -> String {
        format!("tail: expected list, got {}", found.type_name())
    }

    #[cold]
    #[inline(never)]
    fn tuple_expected_err(found: &Value) -> String {
        format!(
            "tuple field access expected Tuple, got {}",
            found.type_name()
        )
    }

    #[cold]
    #[inline(never)]
    fn tuple_oob_err(index: usize, len: usize) -> String {
        format!(
            "tuple index {} out of bounds for tuple of length {}",
            index, len
        )
    }

    #[inline(always)]
    fn fused_cmp_base_opcode(op: OpCode) -> OpCode {
        match op {
            OpCode::OpCmpEqJumpNotTruthy => OpCode::OpEqual,
            OpCode::OpCmpNeJumpNotTruthy => OpCode::OpNotEqual,
            OpCode::OpCmpGtJumpNotTruthy => OpCode::OpGreaterThan,
            OpCode::OpCmpLeJumpNotTruthy => OpCode::OpLessThanOrEqual,
            OpCode::OpCmpGeJumpNotTruthy => OpCode::OpGreaterThanOrEqual,
            _ => unreachable!(),
        }
    }

    #[inline(always)]
    fn execute_cmp_jump_not_truthy(
        &mut self,
        instructions: &[u8],
        ip: usize,
        op: OpCode,
    ) -> Result<usize, String> {
        let jump_pos = Self::read_u16_fast(instructions, ip + 1);
        if self.sp < 2 {
            return Err(Self::stack_underflow_err());
        }

        let l_idx = self.sp - 2;
        let r_idx = self.sp - 1;
        let l_val = self.stack_get(l_idx);
        let r_val = self.stack_get(r_idx);
        let result = match (&l_val, &r_val) {
            (Value::Integer(l), Value::Integer(r)) => match op {
                OpCode::OpCmpEqJumpNotTruthy => l == r,
                OpCode::OpCmpNeJumpNotTruthy => l != r,
                OpCode::OpCmpGtJumpNotTruthy => l > r,
                OpCode::OpCmpLeJumpNotTruthy => l <= r,
                OpCode::OpCmpGeJumpNotTruthy => l >= r,
                _ => unreachable!(),
            },
            _ => {
                drop(l_val);
                drop(r_val);
                let (left, right) = self.pop_pair_untracked()?;
                let result = self.compare_values(&left, &right, Self::fused_cmp_base_opcode(op))?;
                if !result {
                    self.current_frame_mut().ip = jump_pos;
                    return Ok(0);
                }
                return Ok(3);
            }
        };

        self.stack[l_idx] = slot::uninit();
        self.stack[r_idx] = slot::uninit();
        self.sp -= 2;
        self.last_popped = slot::to_slot(Value::None);

        if !result {
            self.current_frame_mut().ip = jump_pos;
            Ok(0)
        } else {
            Ok(3)
        }
    }

    pub(super) fn dispatch_instruction(
        &mut self,
        instructions: &[u8],
        ip: usize,
        op: OpCode,
    ) -> Result<usize, String> {
        match op {
            OpCode::OpCurrentClosure => {
                let closure = self.frames[self.frame_index].closure.clone();
                self.push(Value::Closure(closure))?;
                Ok(1)
            }
            OpCode::OpReturnValue => {
                let mut return_value = self.pop()?;
                // Internal sentinel values must not escape to user-observable results.
                if matches!(return_value, Value::Uninit) {
                    return_value = Value::None;
                }
                if let Some(contract) = self.current_frame().closure.function.contract.as_ref()
                    && let Some(expected) = contract.ret.as_ref()
                    && !expected.matches_value(&return_value, self)
                {
                    let expected_name = expected.type_name();
                    let actual_type = return_value.type_name();
                    let value_preview = format_value(self, &return_value);
                    if let Some((file, span)) = self.function_boundary_location() {
                        return Err(self.runtime_type_error_at_location(
                            &expected_name,
                            actual_type,
                            Some(&value_preview),
                            file,
                            span,
                        ));
                    }
                    return Err(self.runtime_type_error_enhanced(
                        &expected_name,
                        actual_type,
                        Some(&value_preview),
                    ));
                }
                let return_slot = self.pop_frame_return_slot();
                self.reset_sp(return_slot)?;
                self.push(return_value)?;
                Ok(0)
            }
            OpCode::OpReturn => {
                if let Some(contract) = self.current_frame().closure.function.contract.as_ref()
                    && let Some(expected) = contract.ret.as_ref()
                    && !expected.matches_value(&Value::None, self)
                {
                    let expected_name = expected.type_name();
                    let value_preview = format_value(self, &Value::None);
                    if let Some((file, span)) = self.function_boundary_location() {
                        return Err(self.runtime_type_error_at_location(
                            &expected_name,
                            Value::None.type_name(),
                            Some(&value_preview),
                            file,
                            span,
                        ));
                    }
                    return Err(self.runtime_type_error_enhanced(
                        &expected_name,
                        Value::None.type_name(),
                        Some(&value_preview),
                    ));
                }
                let return_slot = self.pop_frame_return_slot();
                self.reset_sp(return_slot)?;
                self.push(Value::None)?;
                Ok(0)
            }
            OpCode::OpGetLocal => {
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let frame_index = self.frame_index;
                let bp = self.frames[frame_index].base_pointer;
                let value = self.stack_get(bp + idx);
                self.push(value)?;
                Ok(2)
            }
            OpCode::OpGetLocal0 => {
                let bp = self.frames[self.frame_index].base_pointer;
                let value = self.stack_get(bp);
                self.push(value)?;
                Ok(1)
            }
            OpCode::OpGetLocal1 => {
                let bp = self.frames[self.frame_index].base_pointer;
                let value = self.stack_get(bp + 1);
                self.push(value)?;
                Ok(1)
            }
            OpCode::OpSetLocal => {
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let bp = self.current_frame().base_pointer;
                let val = self.pop()?;
                self.stack_set(bp + idx, val);
                Ok(2)
            }
            OpCode::OpConsumeLocal => {
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let bp = self.current_frame().base_pointer;
                let value = self.stack_take(bp + idx);
                self.push(value)?;
                Ok(2)
            }
            OpCode::OpConsumeLocal0 => {
                let bp = self.current_frame().base_pointer;
                let value = self.stack_take(bp);
                self.push(value)?;
                Ok(1)
            }
            OpCode::OpConsumeLocal1 => {
                let bp = self.current_frame().base_pointer;
                let value = self.stack_take(bp + 1);
                self.push(value)?;
                Ok(1)
            }
            OpCode::OpGetFree => {
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let value = self.frames[self.frame_index].closure.free[idx].clone();
                self.push(value)?;
                Ok(2)
            }
            OpCode::OpClosure => {
                let idx = Self::read_u16_fast(instructions, ip + 1);
                let num_free = Self::read_u8_fast(instructions, ip + 3);
                self.push_closure(idx, num_free)?;
                Ok(4)
            }
            OpCode::OpClosureLong => {
                let idx = Self::read_u32_fast(instructions, ip + 1);
                let num_free = Self::read_u8_fast(instructions, ip + 5);
                self.push_closure(idx, num_free)?;
                Ok(6)
            }
            OpCode::OpJump => {
                let pos = Self::read_u16_fast(instructions, ip + 1);
                self.current_frame_mut().ip = pos;
                Ok(0)
            }
            OpCode::OpJumpNotTruthy => {
                let pos = Self::read_u16_fast(instructions, ip + 1);
                // Peek instead of pop - value stays on stack for short-circuit operators
                if !self.peek(0)?.is_truthy() {
                    self.current_frame_mut().ip = pos;
                    Ok(0)
                } else {
                    // Only pop if we're NOT jumping (for && operator)
                    self.discard_top()?;
                    Ok(3)
                }
            }
            OpCode::OpJumpTruthy => {
                let pos = Self::read_u16_fast(instructions, ip + 1);
                // Peek instead of pop - value stays on stack for short-circuit operators
                if self.peek(0)?.is_truthy() {
                    self.current_frame_mut().ip = pos;
                    Ok(0)
                } else {
                    // Only pop if we're NOT jumping (for || operator)
                    self.discard_top()?;
                    Ok(3)
                }
            }
            OpCode::OpCmpEqJumpNotTruthy
            | OpCode::OpCmpNeJumpNotTruthy
            | OpCode::OpCmpGtJumpNotTruthy
            | OpCode::OpCmpLeJumpNotTruthy
            | OpCode::OpCmpGeJumpNotTruthy => {
                self.execute_cmp_jump_not_truthy(instructions, ip, op)
            }
            OpCode::OpGetGlobal => {
                let idx = Self::read_u16_fast(instructions, ip + 1);
                let value = self.global_get(idx);
                self.push(value)?;
                Ok(3)
            }
            OpCode::OpSetGlobal => {
                let idx = Self::read_u16_fast(instructions, ip + 1);
                let val = self.pop()?;
                self.global_set(idx, val);
                Ok(3)
            }
            OpCode::OpConstant => {
                let idx = Self::read_u16_fast(instructions, ip + 1);
                let value = self.const_get(idx);
                self.push(value)?;
                Ok(3)
            }
            OpCode::OpConstantLong => {
                let idx = Self::read_u32_fast(instructions, ip + 1);
                let value = self.const_get(idx);
                self.push(value)?;
                Ok(5)
            }
            OpCode::OpAdd | OpCode::OpSub | OpCode::OpMul | OpCode::OpDiv | OpCode::OpMod => {
                // Inline integer fast-path: avoid pop_pair + push overhead for the common case.
                if self.sp >= 2 {
                    let l_idx = self.sp - 2;
                    let r_idx = self.sp - 1;
                    let l_val = self.stack_get(l_idx);
                    let r_val = self.stack_get(r_idx);
                    let fast = match (&l_val, &r_val) {
                        (Value::Integer(l), Value::Integer(r)) => match op {
                            OpCode::OpAdd => Some(Value::Integer(l.wrapping_add(*r))),
                            OpCode::OpSub => Some(Value::Integer(l.wrapping_sub(*r))),
                            OpCode::OpMul => Some(Value::Integer(l.wrapping_mul(*r))),
                            OpCode::OpDiv => {
                                if *r == 0 {
                                    None // fall through to full handler for error
                                } else {
                                    Some(Value::Integer(l / r))
                                }
                            }
                            OpCode::OpMod => {
                                if *r == 0 {
                                    None
                                } else {
                                    Some(Value::Integer(l % r))
                                }
                            }
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(result) = fast {
                        // Overwrite left slot with result, clear right slot.
                        self.stack_set(l_idx, result);
                        self.stack[r_idx] = slot::uninit();
                        self.sp -= 1;
                        self.last_popped = slot::to_slot(Value::None);
                        return Ok(1);
                    }
                }
                self.execute_binary_operation(op)?;
                Ok(1)
            }
            OpCode::OpEqual
            | OpCode::OpNotEqual
            | OpCode::OpGreaterThan
            | OpCode::OpLessThanOrEqual
            | OpCode::OpGreaterThanOrEqual => {
                // Inline integer fast-path for comparisons.
                if self.sp >= 2 {
                    let l_idx = self.sp - 2;
                    let r_idx = self.sp - 1;
                    let l_val = self.stack_get(l_idx);
                    let r_val = self.stack_get(r_idx);
                    let fast = match (&l_val, &r_val) {
                        (Value::Integer(l), Value::Integer(r)) => {
                            let result = match op {
                                OpCode::OpEqual => l == r,
                                OpCode::OpNotEqual => l != r,
                                OpCode::OpGreaterThan => l > r,
                                OpCode::OpLessThanOrEqual => l <= r,
                                OpCode::OpGreaterThanOrEqual => l >= r,
                                _ => unreachable!(),
                            };
                            Some(result)
                        }
                        _ => None,
                    };
                    if let Some(result) = fast {
                        self.stack_set(l_idx, Value::Boolean(result));
                        self.stack[r_idx] = slot::uninit();
                        self.sp -= 1;
                        self.last_popped = slot::to_slot(Value::None);
                        return Ok(1);
                    }
                }
                self.execute_comparison(op)?;
                Ok(1)
            }
            OpCode::OpBang => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let result = !self.stack_get(idx).is_truthy();
                self.stack_set(idx, Value::Boolean(result));
                Ok(1)
            }
            OpCode::OpMinus => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let operand = self.stack_take(idx);
                match operand {
                    Value::Integer(val) => self.stack_set(idx, Value::Integer(-val)),
                    Value::Float(val) => self.stack_set(idx, Value::Float(-val)),
                    _ => {
                        return Err(Self::negation_type_err(&operand));
                    }
                }
                Ok(1)
            }
            OpCode::OpTrue => {
                self.push(Value::Boolean(true))?;
                Ok(1)
            }
            OpCode::OpFalse => {
                self.push(Value::Boolean(false))?;
                Ok(1)
            }
            // Note: OpNull was removed, use OpNone instead
            OpCode::OpIsSome => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let is_some = matches!(self.stack_get(idx), Value::Some(_));
                self.stack_set(idx, Value::Boolean(is_some));
                Ok(1)
            }
            OpCode::OpUnwrapSome => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = self.stack_take(idx);
                match value {
                    Value::Some(inner) => {
                        let v =
                            std::rc::Rc::try_unwrap(inner).unwrap_or_else(|v| v.as_ref().clone());
                        self.stack_set(idx, v);
                    }
                    _ => {
                        return Err(Self::expected_some_err(&value));
                    }
                }
                Ok(1)
            }
            OpCode::OpGetBase => {
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let _ = get_base_function_by_index(idx)
                    .ok_or_else(|| format!("invalid Base function index {}", idx))?;
                self.push(Value::BaseFunction(idx as u8))?;
                Ok(2)
            }
            OpCode::OpCall => {
                let num_args = Self::read_u8_fast(instructions, ip + 1);
                let callee_idx = self.sp - 1 - num_args;
                if matches!(self.stack_get(callee_idx), Value::Continuation(_)) {
                    // `resume(val)` call: restore the captured continuation.
                    // Returns ip_delta = 0 so apply_ip_delta leaves the
                    // newly-restored frame's IP untouched.
                    self.execute_resume(num_args)?;
                    Ok(0)
                } else {
                    self.execute_call(num_args)?;
                    Ok(2)
                }
            }
            OpCode::OpCallSelf => {
                let num_args = Self::read_u8_fast(instructions, ip + 1);
                self.execute_call_self(num_args)?;
                Ok(2)
            }
            OpCode::OpCallBase => {
                // Encoded as [OpCallBase, base_fn_idx, arity]; callee is implicit.
                // Stack before: [..., arg0, ..., argN]. After: [..., result].
                let base_fn_idx = Self::read_u8_fast(instructions, ip + 1);
                let arity = Self::read_u8_fast(instructions, ip + 2);
                self.execute_call_base_direct(base_fn_idx, arity)?;
                Ok(3)
            }
            OpCode::OpPrimOp => {
                // Encoded as [OpPrimOp, primop_id, arity].
                // Stack before: [..., arg0, ..., argN]. After: [..., result].
                let primop_id = Self::read_u8_fast(instructions, ip + 1);
                let arity = Self::read_u8_fast(instructions, ip + 2);
                self.execute_primop_opcode(primop_id, arity)?;
                Ok(3)
            }
            OpCode::OpTailCall => {
                let num_args = Self::read_u8_fast(instructions, ip + 1);
                let callee_idx = self.sp - 1 - num_args;
                let is_base_function = matches!(self.stack_get(callee_idx), Value::BaseFunction(_));
                self.execute_tail_call(num_args)?;
                if is_base_function { Ok(2) } else { Ok(0) }
            }
            OpCode::OpPop => {
                self.pop_and_track()?;
                Ok(1)
            }
            OpCode::OpArray => {
                let num_elements = Self::read_u16_fast(instructions, ip + 1);
                let array = self.build_array(self.sp - num_elements, self.sp);
                self.reset_sp(self.sp - num_elements)?;
                self.push(array)?;
                Ok(3)
            }
            OpCode::OpArrayLong => {
                let num_elements = Self::read_u32_fast(instructions, ip + 1);
                let array = self.build_array(self.sp - num_elements, self.sp);
                self.reset_sp(self.sp - num_elements)?;
                self.push(array)?;
                Ok(5)
            }
            OpCode::OpTuple => {
                let num_elements = Self::read_u16_fast(instructions, ip + 1);
                let tuple = self.build_tuple(self.sp - num_elements, self.sp);
                self.reset_sp(self.sp - num_elements)?;
                self.push(tuple)?;
                Ok(3)
            }
            OpCode::OpTupleLong => {
                let num_elements = Self::read_u32_fast(instructions, ip + 1);
                let tuple = self.build_tuple(self.sp - num_elements, self.sp);
                self.reset_sp(self.sp - num_elements)?;
                self.push(tuple)?;
                Ok(5)
            }
            OpCode::OpTupleIndex => {
                let index = Self::read_u8_fast(instructions, ip + 1);
                let tuple = self.pop_untracked()?;
                match tuple {
                    Value::Tuple(elements) => {
                        if index >= elements.len() {
                            return Err(Self::tuple_oob_err(index, elements.len()));
                        }
                        // Aether: try to move the element without cloning when tuple is
                        // uniquely owned (Rc::strong_count == 1).
                        let value = match Rc::try_unwrap(elements) {
                            Ok(mut vec) => vec.swap_remove(index),
                            Err(shared) => shared[index].clone(),
                        };
                        self.push(value)?;
                        Ok(2)
                    }
                    other => Err(Self::tuple_expected_err(&other)),
                }
            }
            OpCode::OpIsTuple => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let is_tuple = matches!(self.stack_get(idx), Value::Tuple(_));
                self.stack_set(idx, Value::Boolean(is_tuple));
                Ok(1)
            }
            OpCode::OpHash => {
                let num_elements = Self::read_u16_fast(instructions, ip + 1);
                let hash = self.build_hash(self.sp - num_elements, self.sp)?;
                self.reset_sp(self.sp - num_elements)?;
                self.push(hash)?;
                Ok(3)
            }
            OpCode::OpHashLong => {
                let num_elements = Self::read_u32_fast(instructions, ip + 1);
                let hash = self.build_hash(self.sp - num_elements, self.sp)?;
                self.reset_sp(self.sp - num_elements)?;
                self.push(hash)?;
                Ok(5)
            }
            OpCode::OpIndex => {
                let index = self.pop_untracked()?;
                let left = self.pop_untracked()?;
                self.execute_index_expression(left, index)?;
                Ok(1)
            }
            OpCode::OpNone => {
                self.push(Value::None)?;
                Ok(1)
            }
            OpCode::OpSome => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = self.stack_take(idx);
                leak_detector::record_some();
                self.stack_set(idx, Value::Some(std::rc::Rc::new(value)));
                Ok(1)
            }
            // Either type operations
            OpCode::OpLeft => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = self.stack_take(idx);
                self.stack_set(idx, Value::Left(std::rc::Rc::new(value)));
                Ok(1)
            }
            OpCode::OpRight => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = self.stack_take(idx);
                self.stack_set(idx, Value::Right(std::rc::Rc::new(value)));
                Ok(1)
            }
            OpCode::OpIsLeft => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let is_left = matches!(self.stack_get(idx), Value::Left(_));
                self.stack_set(idx, Value::Boolean(is_left));
                Ok(1)
            }
            OpCode::OpIsRight => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let is_right = matches!(self.stack_get(idx), Value::Right(_));
                self.stack_set(idx, Value::Boolean(is_right));
                Ok(1)
            }
            OpCode::OpUnwrapLeft => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = self.stack_take(idx);
                match value {
                    Value::Left(inner) => {
                        let v =
                            std::rc::Rc::try_unwrap(inner).unwrap_or_else(|v| v.as_ref().clone());
                        self.stack_set(idx, v);
                    }
                    _ => return Err(Self::unwrap_left_err()),
                }
                Ok(1)
            }
            OpCode::OpUnwrapRight => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = self.stack_take(idx);
                match value {
                    Value::Right(inner) => {
                        let v =
                            std::rc::Rc::try_unwrap(inner).unwrap_or_else(|v| v.as_ref().clone());
                        self.stack_set(idx, v);
                    }
                    _ => return Err(Self::unwrap_right_err()),
                }
                Ok(1)
            }
            OpCode::OpToString => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = self.stack_take(idx);
                self.stack_set(idx, Value::String(value.to_string_value().into()));
                Ok(1)
            }
            OpCode::OpCons => {
                let (head, tail) = self.pop_pair_untracked()?;
                self.push(ConsCell::cons(head, tail))?;
                Ok(1)
            }
            OpCode::OpIsCons => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let slot_val = self.stack_get(idx);
                let is_cons = matches!(&slot_val, Value::Cons(_));
                self.stack_set(idx, Value::Boolean(is_cons));
                Ok(1)
            }
            OpCode::OpIsEmptyList => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let is_empty = matches!(self.stack_get(idx), Value::None | Value::EmptyList);
                self.stack_set(idx, Value::Boolean(is_empty));
                Ok(1)
            }
            OpCode::OpConsHead => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = self.stack_take(idx);
                match value {
                    // Aether: try_unwrap to move head without cloning when unique.
                    Value::Cons(cell) => {
                        let head = match Rc::try_unwrap(cell) {
                            Ok(mut owned) => std::mem::replace(&mut owned.head, Value::EmptyList),
                            Err(shared) => shared.head.clone(),
                        };
                        self.stack_set(idx, head);
                    }
                    _ => return Err(Self::cons_head_type_err(&value)),
                }
                Ok(1)
            }
            OpCode::OpConsTail => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = self.stack_take(idx);
                match value {
                    // Aether: try_unwrap to move tail without cloning when unique.
                    Value::Cons(cell) => {
                        let tail = match Rc::try_unwrap(cell) {
                            Ok(mut owned) => std::mem::replace(&mut owned.tail, Value::EmptyList),
                            Err(shared) => shared.tail.clone(),
                        };
                        self.stack_set(idx, tail);
                    }
                    _ => return Err(Self::cons_tail_type_err(&value)),
                }
                Ok(1)
            }
            OpCode::OpReturnLocal => {
                // Superinstruction: GetLocal(n) + ReturnValue fused into one dispatch.
                // Avoids clone + push + pop cycle, and can move because the frame is discarded.
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let bp = self.frames[self.frame_index].base_pointer;
                let mut return_value = self.stack_take(bp + idx);
                if matches!(return_value, Value::Uninit) {
                    return_value = Value::None;
                }
                let return_slot = self.pop_frame_return_slot();
                self.reset_sp(return_slot)?;
                self.push(return_value)?;
                Ok(0)
            }
            OpCode::OpMakeAdt => {
                // Operands: const_idx: u16, arity: u8
                // Stack before: [..., field0, field1, ..., fieldN-1]
                // Stack after:  [..., Adt { constructor, fields }]
                let const_idx = Self::read_u16_fast(instructions, ip + 1);
                let arity = Self::read_u8_fast(instructions, ip + 3);
                let const_val = self.const_get(const_idx);
                let constructor_name = match const_val {
                    Value::String(s) => s,
                    other => {
                        return Err(format!(
                            "OpMakeAdt: expected string constant for constructor name, got {}",
                            other
                        ));
                    }
                };

                let mut fields = Vec::with_capacity(arity);

                for i in 0..arity {
                    let val = self.stack_take(self.sp - arity + i);
                    fields.push(val);
                }

                self.reset_sp(self.sp - arity)?;
                if arity == 0 {
                    self.push(Value::AdtUnit(constructor_name))?;
                } else {
                    self.push(Value::Adt(Rc::new(crate::runtime::value::AdtValue {
                        constructor: constructor_name,
                        fields: AdtFields::from_vec(fields),
                    })))?;
                }
                Ok(4) // 1 opcode + 2 const_idx + 1 arity
            }
            OpCode::OpIsAdt => {
                // Operands: const_idx: u16
                // Stack before: [..., value]
                // Stack after:  [..., bool]  (peek-and-replace, value stays for next ops)
                let const_idx = Self::read_u16_fast(instructions, ip + 1);
                let const_val = self.const_get(const_idx);
                let construct_name_owned;
                let construct_name = match &const_val {
                    Value::String(s) => {
                        construct_name_owned = s.as_ref().to_string();
                        construct_name_owned.as_str()
                    }
                    other => {
                        return Err(format!(
                            "OpIsAdt: expected string constant for constructor name, got {}",
                            other
                        ));
                    }
                };

                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }

                let idx = self.sp - 1;
                let slot_val = self.stack_get(idx);
                let is_adt = match &slot_val {
                    value if value.adt_constructor() == Some(construct_name) => true,
                    Value::AdtUnit(name) => name.as_ref() == construct_name,
                    _ => false,
                };

                self.stack_set(idx, Value::Boolean(is_adt));
                Ok(3) // 1 opcode + 2 const_idx
            }
            OpCode::OpAdtField => {
                // Operands: field_idx: u8
                // Stack before: [..., Adt { .. }]
                // Stack after:  [..., field_value]
                let field_idx = Self::read_u8_fast(instructions, ip + 1);
                let adt = self.pop_untracked()?;
                match adt {
                    Value::Adt(_) => {
                        let len = adt.adt_field_count().unwrap_or(0);
                        let value = adt.adt_clone_field(field_idx).ok_or_else(|| {
                            format!(
                                "OpAdtField: field index {} out of bounds (adt has {} fields)",
                                field_idx, len
                            )
                        })?;
                        self.push(value)?;
                        Ok(2) // 1 opcode + 1 field_idx
                    }
                    Value::AdtUnit(name) => Err(format!(
                        "OpAdtField: field index {} out of bounds (AdtUnit '{}' has 0 fields)",
                        field_idx, name
                    )),
                    other => Err(format!(
                        "OpAdtField: expected Adt value, got {}",
                        other.type_name()
                    )),
                }
            }

            OpCode::OpIsAdtJump => {
                // Operands: const_idx: u16, jump_offset: u16
                // Stack before: [..., value]
                // On match:    fall through (5 bytes), ADT stays on stack
                // On mismatch: jump to jump_offset, ADT stays on stack (caller must OpPop)
                let const_idx = Self::read_u16_fast(instructions, ip + 1);
                let jump_pos = Self::read_u16_fast(instructions, ip + 3);
                let const_val = self.const_get(const_idx);
                let constructor_name_owned;
                let constructor_name = match &const_val {
                    Value::String(s) => {
                        constructor_name_owned = s.as_ref().to_string();
                        constructor_name_owned.as_str()
                    }
                    other => {
                        return Err(format!(
                            "OpIsAdtJump: expected string constant for constructor name, got {}",
                            other
                        ));
                    }
                };
                let peek_val = self.peek(0)?;
                let is_match = match &peek_val {
                    value if value.adt_constructor() == Some(constructor_name) => true,
                    Value::AdtUnit(name) => name.as_ref() == constructor_name,
                    _ => false,
                };
                if is_match {
                    Ok(5) // 1 opcode + 2 const_idx + 2 jump_offset
                } else {
                    self.current_frame_mut().ip = jump_pos;
                    Ok(0)
                }
            }
            OpCode::OpIsAdtJumpLocal => {
                // Operands: local_idx: u8, const_idx: u16, jump_offset: u16
                // Peeks at local[local_idx] WITHOUT pushing it to the stack.
                // On constructor match: fall through (6 bytes), local slot unchanged.
                // On mismatch: jump to jump_offset, local slot unchanged.
                // The matching arm must emit OpConsumeLocal(local_idx) next to move the value
                // onto the stack with Rc strong_count == 1, enabling Rc::try_unwrap in OpAdtFields2.
                let local_idx = Self::read_u8_fast(instructions, ip + 1);
                let const_idx = Self::read_u16_fast(instructions, ip + 2);
                let jump_pos = Self::read_u16_fast(instructions, ip + 4);
                let const_val = self.const_get(const_idx);
                let constructor_name_owned;
                let constructor_name = match &const_val {
                    Value::String(s) => {
                        constructor_name_owned = s.as_ref().to_string();
                        constructor_name_owned.as_str()
                    }
                    other => {
                        return Err(format!(
                            "OpIsAdtJumpLocal: expected string constant for constructor name, got {}",
                            other
                        ));
                    }
                };
                let bp = self.frames[self.frame_index].base_pointer;
                let local_val = self.stack_get(bp + local_idx);
                let is_match = match &local_val {
                    value if value.adt_constructor() == Some(constructor_name) => true,
                    Value::AdtUnit(name) => name.as_ref() == constructor_name,
                    _ => false,
                };
                if is_match {
                    Ok(6) // 1 opcode + 1 local_idx + 2 const_idx + 2 jump_offset
                } else {
                    self.current_frame_mut().ip = jump_pos;
                    Ok(0)
                }
            }
            OpCode::OpAdtFields2 => {
                // No operands.
                // Stack before: [..., Adt { field0, field1, ... }]
                // Stack after:  [..., field0, field1]
                let adt = self.pop_untracked()?;
                match adt {
                    Value::Adt(_) => {
                        let (f0, f1) = adt.adt_clone_two_fields().ok_or_else(|| {
                            "OpAdtFields2: ADT has fewer than 2 fields".to_string()
                        })?;
                        self.push(f0)?;
                        self.push(f1)?;
                        Ok(1) // just the opcode byte
                    }
                    other => Err(format!(
                        "OpAdtFields2: expected Adt value, got {}",
                        other.type_name()
                    )),
                }
            }

            // ---------------------------------------------------------------
            // Algebraic effects
            // ---------------------------------------------------------------
            OpCode::OpHandle => {
                // Operands: const_idx: u8
                // Stack before: [..., closure_0, ..., closure_{n-1}]
                //   where n = descriptor.ops.len()
                // Stack after:  [...]  (closures consumed)
                // Side effect: HandlerFrame pushed onto handler_stack
                let const_idx = Self::read_u8_fast(instructions, ip + 1);
                let const_val = self.const_get(const_idx);
                let (effect, ops, desc_is_discard) = match &const_val {
                    Value::HandlerDescriptor(desc) => {
                        (desc.effect, desc.ops.clone(), desc.is_discard)
                    }
                    other => {
                        return Err(format!(
                            "OpHandle: expected HandlerDescriptor constant, got {}",
                            other.type_name()
                        ));
                    }
                };
                let n = ops.len();
                // Pop closures in reverse (last pushed = last arm)
                let mut arm_closures: Vec<Rc<Closure>> = Vec::with_capacity(n);
                for _ in 0..n {
                    let v = self.pop_untracked()?;
                    match v {
                        Value::Closure(c) => arm_closures.push(c),
                        other => {
                            return Err(format!(
                                "OpHandle: expected Closure for arm, got {}",
                                other.type_name()
                            ));
                        }
                    }
                }
                // Closures were pushed in order 0..n, popped in reverse → reverse to restore order
                arm_closures.reverse();
                let arms: Vec<HandlerArm> = ops
                    .into_iter()
                    .zip(arm_closures)
                    .map(|(op, closure)| HandlerArm { op, closure })
                    .collect();
                self.handler_stack.push(HandlerFrame {
                    effect,
                    arms,
                    entry_frame_index: self.frame_index,
                    entry_sp: self.sp,
                    entry_handler_stack_len: self.handler_stack.len(),
                    is_direct: false,
                    is_discard: desc_is_discard,
                });
                Ok(2)
            }
            OpCode::OpHandleDirect => {
                // Identical to OpHandle but marks the handler as tail-resumptive.
                let const_idx = Self::read_u8_fast(instructions, ip + 1);
                let const_val = self.const_get(const_idx);
                let (effect, ops) = match &const_val {
                    Value::HandlerDescriptor(desc) => (desc.effect, desc.ops.clone()),
                    other => {
                        return Err(format!(
                            "OpHandleDirect: expected HandlerDescriptor constant, got {}",
                            other.type_name()
                        ));
                    }
                };
                let n = ops.len();
                let mut arm_closures: Vec<Rc<Closure>> = Vec::with_capacity(n);
                for _ in 0..n {
                    let v = self.pop_untracked()?;
                    match v {
                        Value::Closure(c) => arm_closures.push(c),
                        other => {
                            return Err(format!(
                                "OpHandleDirect: expected Closure for arm, got {}",
                                other.type_name()
                            ));
                        }
                    }
                }
                arm_closures.reverse();
                let arms: Vec<HandlerArm> = ops
                    .into_iter()
                    .zip(arm_closures)
                    .map(|(op, closure)| HandlerArm { op, closure })
                    .collect();
                self.handler_stack.push(HandlerFrame {
                    effect,
                    arms,
                    entry_frame_index: self.frame_index,
                    entry_sp: self.sp,
                    entry_handler_stack_len: self.handler_stack.len(),
                    is_direct: true,
                    is_discard: false,
                });
                Ok(2)
            }
            OpCode::OpEndHandle => {
                // No operands. Pops the top handler from handler_stack.
                self.handler_stack.pop();
                Ok(1)
            }
            OpCode::OpPerform => {
                // Operands: const_idx: u8, arity: u8
                // Stack before: [..., arg_0, ..., arg_{arity-1}]
                // After:  captures continuation, unwinds to handler frame,
                //         calls arm closure(resume_cont, arg_0, ..., arg_{arity-1})
                // ip_delta = 0 (frame change)
                let const_idx = Self::read_u8_fast(instructions, ip + 1);
                let arity = Self::read_u8_fast(instructions, ip + 2);

                let const_val = self.const_get(const_idx);
                let (effect, op, effect_name, op_name) = match &const_val {
                    Value::PerformDescriptor(desc) => (
                        desc.effect,
                        desc.op,
                        desc.effect_name.clone(),
                        desc.op_name.clone(),
                    ),
                    other => {
                        return Err(format!(
                            "OpPerform: expected PerformDescriptor constant, got {}",
                            other.type_name()
                        ));
                    }
                };

                // Pop the perform arguments
                let mut perform_args: Vec<Value> = Vec::with_capacity(arity);
                for _ in 0..arity {
                    perform_args.push(self.pop_untracked()?);
                }
                perform_args.reverse();

                // Find matching handler frame (search from top)
                let handler_pos = self
                    .handler_stack
                    .iter()
                    .rposition(|h| h.effect == effect)
                    .ok_or_else(|| {
                        format!(
                            "unhandled effect: {} (no matching handle block)",
                            effect_name
                        )
                    })?;

                let is_direct = self.handler_stack[handler_pos].is_direct;

                let arm_closure = {
                    let handler = &self.handler_stack[handler_pos];
                    let arm = handler.arms.iter().find(|a| a.op == op).ok_or_else(|| {
                        format!("unhandled operation: {}.{}", effect_name, op_name)
                    })?;
                    arm.closure.clone()
                };

                let is_discard = self.handler_stack[handler_pos].is_discard;

                if is_direct {
                    // Tail-resumptive fast path: call the arm directly
                    // without capturing a continuation. resume(v) returns v
                    // via the identity closure passed as the resume parameter.
                    //
                    // Advance the caller's IP past OpPerform (3 bytes) BEFORE
                    // pushing the arm frame, so that when the arm returns,
                    // execution continues at the instruction after perform.
                    self.frames[self.frame_index].ip += 3;

                    self.push(Value::Closure(arm_closure))?;
                    let identity_fn = self.make_identity_closure();
                    self.push(identity_fn)?;
                    for arg in perform_args {
                        self.push(arg)?;
                    }
                    self.execute_call(1 + arity)?;
                    Ok(0)
                } else if is_discard {
                    // Discard handler: never resumes. Skip continuation capture
                    // entirely — just unwind to handler entry and call the arm.
                    // (Perceus Section 2.7.1: non-linear control flow safety.)
                    let entry_frame_index = self.handler_stack[handler_pos].entry_frame_index;
                    let entry_sp = self.handler_stack[handler_pos].entry_sp;

                    // Unwind: drop all values between handler entry and perform site.
                    // This is safe because the handler never resumes — those values
                    // are dead after the arm returns.
                    self.frame_index = entry_frame_index;
                    self.handler_stack.truncate(handler_pos + 1);
                    self.reset_sp(entry_sp)?;

                    // Call the arm with Value::None as the resume parameter
                    // (handler won't use it since it never resumes).
                    self.push(Value::Closure(arm_closure))?;
                    self.push(Value::None)?; // dummy resume
                    for arg in perform_args {
                        self.push(arg)?;
                    }
                    self.execute_call(1 + arity)?;
                    Ok(0)
                } else {
                    let entry_frame_index = self.handler_stack[handler_pos].entry_frame_index;
                    let entry_sp = self.handler_stack[handler_pos].entry_sp;

                    // Capture frames above the handler boundary.
                    let mut captured_frames: Vec<crate::runtime::frame::Frame> =
                        self.frames[entry_frame_index + 1..=self.frame_index].to_vec();
                    if let Some(last) = captured_frames.last_mut() {
                        last.ip += 3;
                    }

                    let captured_sp = self.sp;
                    let captured_stack: Vec<Value> = self.stack[entry_sp..captured_sp]
                        .iter()
                        .map(slot::from_slot_ref)
                        .collect();

                    let inner_handlers: Vec<HandlerFrame> =
                        self.handler_stack[handler_pos + 1..].to_vec();

                    let cont = Continuation {
                        frames: captured_frames,
                        stack: captured_stack,
                        sp: captured_sp,
                        entry_sp,
                        entry_frame_index,
                        inner_handlers,
                        used: false,
                    };
                    let cont_val = Value::Continuation(Rc::new(RefCell::new(cont)));

                    self.frame_index = entry_frame_index;
                    self.handler_stack.truncate(handler_pos + 1);
                    self.reset_sp(entry_sp)?;

                    self.push(Value::Closure(arm_closure))?;
                    self.push(cont_val)?;
                    for arg in perform_args {
                        self.push(arg)?;
                    }
                    self.execute_call(1 + arity)?;
                    Ok(0)
                }
            }
            OpCode::OpPerformDirect => {
                // Tail-resumptive perform: no continuation capture.
                // The arm closure is called directly; `resume(v)` inside the
                // arm simply returns `v` which becomes the perform result.
                let const_idx = Self::read_u8_fast(instructions, ip + 1);
                let arity = Self::read_u8_fast(instructions, ip + 2);

                let const_val2 = self.const_get(const_idx);
                let (effect, op, effect_name, op_name) = match &const_val2 {
                    Value::PerformDescriptor(desc) => (
                        desc.effect,
                        desc.op,
                        desc.effect_name.clone(),
                        desc.op_name.clone(),
                    ),
                    other => {
                        return Err(format!(
                            "OpPerformDirect: expected PerformDescriptor constant, got {}",
                            other.type_name()
                        ));
                    }
                };

                let mut perform_args: Vec<Value> = Vec::with_capacity(arity);
                for _ in 0..arity {
                    perform_args.push(self.pop_untracked()?);
                }
                perform_args.reverse();

                // Find matching handler (same search as OpPerform)
                let handler_pos = self
                    .handler_stack
                    .iter()
                    .rposition(|h| h.effect == effect)
                    .ok_or_else(|| {
                        format!(
                            "unhandled effect: {} (no matching handle block)",
                            effect_name
                        )
                    })?;

                let arm_closure = {
                    let handler = &self.handler_stack[handler_pos];
                    let arm = handler.arms.iter().find(|a| a.op == op).ok_or_else(|| {
                        format!("unhandled operation: {}.{}", effect_name, op_name)
                    })?;
                    arm.closure.clone()
                };

                // For a tail-resumptive arm: resume(v) just returns v.
                // We create a dummy "identity" closure as the resume parameter
                // so that `resume(v)` inside the arm body evaluates to `v`.
                // The arm closure signature is: fn(resume, arg0, ..., argN)
                // We push the arm closure, a resume-identity value, then args.
                self.push(Value::Closure(arm_closure))?;
                // Use Value::None as a sentinel — in a tail-resumptive arm,
                // resume(v) is compiled as a regular call. We intercept it by
                // providing the identity function from base functions.
                let identity_fn = self.make_identity_closure();
                self.push(identity_fn)?;
                for arg in perform_args {
                    self.push(arg)?;
                }
                self.execute_call(1 + arity)?;

                Ok(0)
            }

            OpCode::OpPerformDirectIndexed => {
                // Tail-resumptive perform with compile-time resolved handler.
                // Skips handler stack search — uses encoded depth and arm index.
                let handler_depth = Self::read_u8_fast(instructions, ip + 1);
                let arm_index = Self::read_u8_fast(instructions, ip + 2);
                let arity = Self::read_u8_fast(instructions, ip + 3);

                let mut perform_args: Vec<Value> = Vec::with_capacity(arity);
                for _ in 0..arity {
                    perform_args.push(self.pop_untracked()?);
                }
                perform_args.reverse();

                // Direct index into handler stack — no search.
                let handler_idx = self.handler_stack.len() - 1 - handler_depth;
                let arm_closure = self.handler_stack[handler_idx].arms[arm_index]
                    .closure
                    .clone();

                // Same direct dispatch as OpPerformDirect: push arm closure,
                // identity resume, then arguments.
                self.push(Value::Closure(arm_closure))?;
                let identity_fn = self.make_identity_closure();
                self.push(identity_fn)?;
                for arg in perform_args {
                    self.push(arg)?;
                }
                self.execute_call(1 + arity)?;

                Ok(0)
            }

            // ── Aether reuse opcodes ────────────────────────────────────
            OpCode::OpDropReuse => {
                // Test if TOS value is uniquely owned. If so, push it as a
                // reuse token; otherwise push None (= allocate fresh).
                let val = self.pop()?;
                let is_unique = match &val {
                    Value::Cons(rc) => Rc::strong_count(rc) == 1,
                    Value::Adt(rc) => Rc::strong_count(rc) == 1,
                    Value::Some(rc) | Value::Left(rc) | Value::Right(rc) => {
                        Rc::strong_count(rc) == 1
                    }
                    _ => false,
                };
                if is_unique {
                    self.push(val)?;
                } else {
                    self.push(Value::None)?;
                }
                Ok(1)
            }

            OpCode::OpReuseCons => {
                let field_mask = Self::read_u8_fast(instructions, ip + 1) as u8;
                // Stack order: token was pushed first, then head, then tail (TOS)
                let tail = self.pop()?;
                let head = self.pop()?;
                let token = self.pop()?;
                let result = match token {
                    Value::Cons(rc) => {
                        match Rc::try_unwrap(rc) {
                            Ok(mut cell) => {
                                // Unique — reuse allocation, only write changed fields
                                if field_mask == 0xFF || field_mask & 1 != 0 {
                                    cell.head = head;
                                }
                                if field_mask == 0xFF || field_mask & 2 != 0 {
                                    cell.tail = tail;
                                }
                                Value::Cons(Rc::new(cell))
                            }
                            Err(_) => {
                                // Shared — allocate fresh
                                ConsCell::cons(head, tail)
                            }
                        }
                    }
                    _ => ConsCell::cons(head, tail),
                };
                self.push(result)?;
                Ok(2) // 1 opcode + 1 byte field_mask
            }

            OpCode::OpReuseAdt => {
                let const_idx = Self::read_u16_fast(instructions, ip + 1);
                let arity = Self::read_u8_fast(instructions, ip + 3);
                let field_mask = Self::read_u8_fast(instructions, ip + 4) as u8;

                let const_val = self.const_get(const_idx);
                let constructor_name = match const_val {
                    Value::String(s) => s,
                    other => {
                        return Err(format!(
                            "OpReuseAdt: expected string constant, got {}",
                            other
                        ));
                    }
                };

                // Pop fields from stack (same order as OpMakeAdt)
                let mut fields = Vec::with_capacity(arity);
                for i in 0..arity {
                    let val = self.stack_take(self.sp - arity + i);
                    fields.push(val);
                }
                self.reset_sp(self.sp - arity)?;
                let token = self.pop()?;

                let result = match token {
                    Value::Adt(rc) => {
                        match Rc::try_unwrap(rc) {
                            Ok(mut adt) => {
                                adt.constructor = constructor_name;
                                if field_mask == 0xFF {
                                    adt.fields = AdtFields::from_vec(fields);
                                } else {
                                    for (i, val) in fields.into_iter().enumerate() {
                                        if field_mask as u64 & (1u64 << i) != 0 {
                                            adt.fields.set_field(i, val);
                                        }
                                    }
                                }
                                Value::Adt(Rc::new(adt))
                            }
                            Err(_) => Value::Adt(Rc::new(AdtValue {
                                constructor: constructor_name,
                                fields: AdtFields::from_vec(fields),
                            })),
                        }
                    }
                    _ => Value::Adt(Rc::new(AdtValue {
                        constructor: constructor_name,
                        fields: AdtFields::from_vec(fields),
                    })),
                };
                self.push(result)?;
                Ok(5) // 1 opcode + 2 const_idx + 1 arity + 1 field_mask
            }

            OpCode::OpReuseSome => {
                let inner = self.pop()?;
                let token = self.pop()?;
                let result = match token {
                    Value::Some(rc) if Rc::strong_count(&rc) == 1 => {
                        // Reuse — but Some only has one field, always write it
                        Value::Some(Rc::new(inner))
                    }
                    _ => Value::Some(Rc::new(inner)),
                };
                self.push(result)?;
                Ok(1)
            }

            OpCode::OpReuseLeft => {
                let inner = self.pop()?;
                let token = self.pop()?;
                let result = match token {
                    Value::Left(rc) if Rc::strong_count(&rc) == 1 => {
                        Value::Left(Rc::new(inner))
                    }
                    _ => Value::Left(Rc::new(inner)),
                };
                self.push(result)?;
                Ok(1)
            }

            OpCode::OpReuseRight => {
                let inner = self.pop()?;
                let token = self.pop()?;
                let result = match token {
                    Value::Right(rc) if Rc::strong_count(&rc) == 1 => {
                        Value::Right(Rc::new(inner))
                    }
                    _ => Value::Right(Rc::new(inner)),
                };
                self.push(result)?;
                Ok(1)
            }

            OpCode::OpIsUnique => {
                let val = self.pop()?;
                let unique = match &val {
                    Value::Cons(rc) => Rc::strong_count(rc) == 1,
                    Value::Adt(rc) => Rc::strong_count(rc) == 1,
                    Value::Some(rc) | Value::Left(rc) | Value::Right(rc) => {
                        Rc::strong_count(rc) == 1
                    }
                    Value::HashMap(rc) => Rc::strong_count(rc) == 1,
                    _ => true,
                };
                // Push the value back (IsUnique is non-destructive)
                self.push(val)?;
                self.push(Value::Boolean(unique))?;
                Ok(1)
            }
        }
    }
}
