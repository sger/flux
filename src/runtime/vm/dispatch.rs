use std::{cell::RefCell, rc::Rc};

use crate::{
    bytecode::op_code::OpCode,
    runtime::{
        base::get_base_function_by_index,
        base::list_ops::format_value,
        closure::Closure,
        continuation::Continuation,
        gc::HeapObject,
        handler_arm::HandlerArm,
        handler_frame::HandlerFrame,
        leak_detector,
        value::{AdtValue, Value},
    },
};

use super::VM;

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
    fn cons_head_heap_err(other: &HeapObject) -> String {
        format!("head: expected list, got {:?}", other)
    }

    #[cold]
    #[inline(never)]
    fn cons_tail_heap_err(other: &HeapObject) -> String {
        format!("tail: expected list, got {:?}", other)
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
                    return Err(self.runtime_type_error_enhanced(
                        &expected_name,
                        actual_type,
                        Some(&value_preview),
                    ));
                }
                let bp = self.pop_frame_bp();
                self.reset_sp(bp - 1)?;
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
                    return Err(self.runtime_type_error_enhanced(
                        &expected_name,
                        Value::None.type_name(),
                        Some(&value_preview),
                    ));
                }
                let bp = self.pop_frame_bp();
                self.reset_sp(bp - 1)?;
                self.push(Value::None)?;
                Ok(0)
            }
            OpCode::OpGetLocal => {
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let frame_index = self.frame_index;
                let bp = self.frames[frame_index].base_pointer;
                let value = self.stack[bp + idx].clone();
                self.push(value)?;
                Ok(2)
            }
            OpCode::OpGetLocal0 => {
                let bp = self.frames[self.frame_index].base_pointer;
                let value = self.stack[bp].clone();
                self.push(value)?;
                Ok(1)
            }
            OpCode::OpGetLocal1 => {
                let bp = self.frames[self.frame_index].base_pointer;
                let value = self.stack[bp + 1].clone();
                self.push(value)?;
                Ok(1)
            }
            OpCode::OpSetLocal => {
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let bp = self.current_frame().base_pointer;
                self.stack[bp + idx] = self.pop()?;
                Ok(2)
            }
            OpCode::OpConsumeLocal => {
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let bp = self.current_frame().base_pointer;
                let value = std::mem::replace(&mut self.stack[bp + idx], Value::Uninit);
                self.push(value)?;
                Ok(2)
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
            OpCode::OpGetGlobal => {
                let idx = Self::read_u16_fast(instructions, ip + 1);
                let value = self.globals[idx].clone();
                self.push(value)?;
                Ok(3)
            }
            OpCode::OpSetGlobal => {
                let idx = Self::read_u16_fast(instructions, ip + 1);
                self.globals[idx] = self.pop()?;
                Ok(3)
            }
            OpCode::OpConstant => {
                let idx = Self::read_u16_fast(instructions, ip + 1);
                let value = self.constants[idx].clone();
                self.push(value)?;
                Ok(3)
            }
            OpCode::OpConstantLong => {
                let idx = Self::read_u32_fast(instructions, ip + 1);
                let value = self.constants[idx].clone();
                self.push(value)?;
                Ok(5)
            }
            OpCode::OpAdd | OpCode::OpSub | OpCode::OpMul | OpCode::OpDiv | OpCode::OpMod => {
                // Inline integer fast-path: avoid pop_pair + push overhead for the common case.
                if self.sp >= 2 {
                    let l_idx = self.sp - 2;
                    let r_idx = self.sp - 1;
                    let fast = match (&self.stack[l_idx], &self.stack[r_idx]) {
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
                        // Old values are Integer (trivially droppable).
                        self.stack[l_idx] = result;
                        self.stack[r_idx] = Value::Uninit;
                        self.sp -= 1;
                        self.last_popped = Value::None;
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
                    let fast = match (&self.stack[l_idx], &self.stack[r_idx]) {
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
                        self.stack[l_idx] = Value::Boolean(result);
                        self.stack[r_idx] = Value::Uninit;
                        self.sp -= 1;
                        self.last_popped = Value::None;
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
                let result = !self.stack[idx].is_truthy();
                self.stack[idx] = Value::Boolean(result);
                Ok(1)
            }
            OpCode::OpMinus => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let operand = std::mem::replace(&mut self.stack[idx], Value::Uninit);
                match operand {
                    Value::Integer(val) => self.stack[idx] = Value::Integer(-val),
                    Value::Float(val) => self.stack[idx] = Value::Float(-val),
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
                let is_some = matches!(self.stack[idx], Value::Some(_));
                self.stack[idx] = Value::Boolean(is_some);
                Ok(1)
            }
            OpCode::OpUnwrapSome => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = std::mem::replace(&mut self.stack[idx], Value::Uninit);
                match value {
                    Value::Some(inner) => {
                        let value =
                            std::rc::Rc::try_unwrap(inner).unwrap_or_else(|v| v.as_ref().clone());
                        self.stack[idx] = value;
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
                if matches!(self.stack[callee_idx], Value::Continuation(_)) {
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
                let is_base_function = matches!(self.stack[callee_idx], Value::BaseFunction(_));
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
                        let value = elements
                            .get(index)
                            .cloned()
                            .ok_or_else(|| Self::tuple_oob_err(index, elements.len()))?;
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
                let is_tuple = matches!(self.stack[idx], Value::Tuple(_));
                self.stack[idx] = Value::Boolean(is_tuple);
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
                let value = std::mem::replace(&mut self.stack[idx], Value::Uninit);
                leak_detector::record_some();
                self.stack[idx] = Value::Some(std::rc::Rc::new(value));
                Ok(1)
            }
            // Either type operations
            OpCode::OpLeft => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = std::mem::replace(&mut self.stack[idx], Value::Uninit);
                self.stack[idx] = Value::Left(std::rc::Rc::new(value));
                Ok(1)
            }
            OpCode::OpRight => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = std::mem::replace(&mut self.stack[idx], Value::Uninit);
                self.stack[idx] = Value::Right(std::rc::Rc::new(value));
                Ok(1)
            }
            OpCode::OpIsLeft => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let is_left = matches!(self.stack[idx], Value::Left(_));
                self.stack[idx] = Value::Boolean(is_left);
                Ok(1)
            }
            OpCode::OpIsRight => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let is_right = matches!(self.stack[idx], Value::Right(_));
                self.stack[idx] = Value::Boolean(is_right);
                Ok(1)
            }
            OpCode::OpUnwrapLeft => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = std::mem::replace(&mut self.stack[idx], Value::Uninit);
                match value {
                    Value::Left(inner) => {
                        let value =
                            std::rc::Rc::try_unwrap(inner).unwrap_or_else(|v| v.as_ref().clone());
                        self.stack[idx] = value;
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
                let value = std::mem::replace(&mut self.stack[idx], Value::Uninit);
                match value {
                    Value::Right(inner) => {
                        let value =
                            std::rc::Rc::try_unwrap(inner).unwrap_or_else(|v| v.as_ref().clone());
                        self.stack[idx] = value;
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
                let value = std::mem::replace(&mut self.stack[idx], Value::Uninit);
                self.stack[idx] = Value::String(value.to_string_value().into());
                Ok(1)
            }
            OpCode::OpCons => {
                let (head, tail) = self.pop_pair_untracked()?;
                let handle = self.gc_alloc(HeapObject::Cons { head, tail });
                self.push(Value::Gc(handle))?;
                Ok(1)
            }
            OpCode::OpIsCons => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let is_cons = matches!(&self.stack[idx], Value::Gc(h) if matches!(self.gc_heap.get(*h), HeapObject::Cons { .. }));
                self.stack[idx] = Value::Boolean(is_cons);
                Ok(1)
            }
            OpCode::OpIsEmptyList => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let is_empty = matches!(self.stack[idx], Value::None | Value::EmptyList);
                self.stack[idx] = Value::Boolean(is_empty);
                Ok(1)
            }
            OpCode::OpConsHead => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = std::mem::replace(&mut self.stack[idx], Value::Uninit);
                match &value {
                    Value::Gc(h) => match self.gc_heap.get(*h) {
                        HeapObject::Cons { head, .. } => self.stack[idx] = head.clone(),
                        other => return Err(Self::cons_head_heap_err(other)),
                    },
                    _ => return Err(Self::cons_head_type_err(&value)),
                }
                Ok(1)
            }
            OpCode::OpConsTail => {
                if self.sp == 0 {
                    return Err(Self::stack_underflow_err());
                }
                let idx = self.sp - 1;
                let value = std::mem::replace(&mut self.stack[idx], Value::Uninit);
                match &value {
                    Value::Gc(h) => match self.gc_heap.get(*h) {
                        HeapObject::Cons { tail, .. } => self.stack[idx] = tail.clone(),
                        other => return Err(Self::cons_tail_heap_err(other)),
                    },
                    _ => return Err(Self::cons_tail_type_err(&value)),
                }
                Ok(1)
            }
            OpCode::OpReturnLocal => {
                // Superinstruction: GetLocal(n) + ReturnValue fused into one dispatch.
                // Avoids clone + push + pop cycle.
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let bp = self.frames[self.frame_index].base_pointer;
                let mut return_value = self.stack[bp + idx].clone();
                if matches!(return_value, Value::Uninit) {
                    return_value = Value::None;
                }
                let frame_bp = self.pop_frame_bp();
                self.reset_sp(frame_bp - 1)?;
                self.push(return_value)?;
                Ok(0)
            }
            OpCode::OpMakeAdt => {
                // Operands: const_idx: u16, arity: u8
                // Stack before: [..., field0, field1, ..., fieldN-1]
                // Stack after:  [..., Adt { constructor, fields }]
                let const_idx = Self::read_u16_fast(instructions, ip + 1);
                let arity = Self::read_u8_fast(instructions, ip + 3);
                let constructor_name = match &self.constants[const_idx] {
                    Value::String(s) => std::rc::Rc::clone(s),
                    other => {
                        return Err(format!(
                            "OpMakeAdt: expected string constant for constructor name, got {}",
                            other
                        ));
                    }
                };

                let mut fields = Vec::with_capacity(arity);

                for i in 0..arity {
                    let val = self.stack[self.sp - arity + i].clone();
                    fields.push(val);
                }

                self.reset_sp(self.sp - arity)?;
                self.push(Value::Adt(std::rc::Rc::new(AdtValue {
                    constructor: constructor_name,
                    fields,
                })))?;
                Ok(4) // 1 opcode + 2 const_idx + 1 arity
            }
            OpCode::OpIsAdt => {
                // Operands: const_idx: u16
                // Stack before: [..., value]
                // Stack after:  [..., bool]  (peek-and-replace, value stays for next ops)
                let const_idx = Self::read_u16_fast(instructions, ip + 1);
                let construct_name = match &self.constants[const_idx] {
                    Value::String(s) => s.as_ref().to_owned(),
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
                let is_adt = match &self.stack[idx] {
                    Value::Adt(adt) => adt.constructor.as_ref() == construct_name,
                    _ => false,
                };

                self.stack[idx] = Value::Boolean(is_adt);
                Ok(3) // 1 opcode + 2 const_idx
            }
            OpCode::OpAdtField => {
                // Operands: field_idx: u8
                // Stack before: [..., Adt { .. }]
                // Stack after:  [..., field_value]
                let field_idx = Self::read_u8_fast(instructions, ip + 1);
                let adt = self.pop_untracked()?;
                match adt {
                    Value::Adt(adt) => {
                        let value = adt.fields.get(field_idx).cloned().ok_or_else(|| {
                            format!(
                                "OpAdtField: field index {} out of bounds (adt has {} fields)",
                                field_idx,
                                adt.fields.len()
                            )
                        })?;
                        self.push(value)?;
                        Ok(2) // 1 opcode + 1 field_idx
                    }
                    other => Err(format!(
                        "OpAdtField: expected Adt value, got {}",
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
                let (effect, ops) = match &self.constants[const_idx] {
                    Value::HandlerDescriptor(desc) => (desc.effect, desc.ops.clone()),
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

                let (effect, op, effect_name, op_name) = match &self.constants[const_idx] {
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

                let arm_closure = {
                    let handler = &self.handler_stack[handler_pos];
                    let arm = handler.arms.iter().find(|a| a.op == op).ok_or_else(|| {
                        format!("unhandled operation: {}.{}", effect_name, op_name)
                    })?;
                    arm.closure.clone()
                };

                let entry_frame_index = self.handler_stack[handler_pos].entry_frame_index;
                let entry_sp = self.handler_stack[handler_pos].entry_sp;

                // Capture frames above the handler boundary.
                // Advance the perform frame's IP past OpPerform (3 bytes) so that
                // when the continuation is resumed, execution continues correctly.
                let mut captured_frames: Vec<super::super::frame::Frame> =
                    self.frames[entry_frame_index + 1..=self.frame_index].to_vec();
                if let Some(last) = captured_frames.last_mut() {
                    last.ip += 3; // skip past OpPerform (opcode:1 + const_idx:1 + arity:1)
                }

                // Capture value stack slice (between entry_sp and sp before perform args).
                let captured_sp = self.sp; // sp after args were already popped above
                let captured_stack = self.stack[entry_sp..captured_sp].to_vec();

                // Capture inner handlers (those nested inside our handler boundary)
                let inner_handlers: Vec<HandlerFrame> =
                    self.handler_stack[handler_pos + 1..].to_vec();

                // Build continuation
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

                // Unwind back to handler entry frame
                self.frame_index = entry_frame_index;
                // Keep our handler frame on handler_stack (OpEndHandle will pop it after arm returns).
                // Truncate inner handlers that were captured.
                self.handler_stack.truncate(handler_pos + 1);
                self.reset_sp(entry_sp)?;

                // Call the arm: push closure, then resume (cont_val), then perform args
                self.push(Value::Closure(arm_closure))?;
                self.push(cont_val)?;
                for arg in perform_args {
                    self.push(arg)?;
                }
                self.execute_call(1 + arity)?;

                Ok(0)
            }
        }
    }
}
