use crate::{
    bytecode::op_code::OpCode,
    runtime::{builtins::get_builtin_by_index, gc::HeapObject, leak_detector, value::Value},
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
                let bp = self.pop_frame_bp();
                self.reset_sp(bp - 1)?;
                self.push(return_value)?;
                Ok(0)
            }
            OpCode::OpReturn => {
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
            OpCode::OpGetBuiltin => {
                let idx = Self::read_u8_fast(instructions, ip + 1);
                let _ = get_builtin_by_index(idx)
                    .ok_or_else(|| format!("invalid builtin index {}", idx))?;
                self.push(Value::Builtin(idx as u8))?;
                Ok(2)
            }
            OpCode::OpCall => {
                let num_args = Self::read_u8_fast(instructions, ip + 1);
                self.execute_call(num_args)?;
                Ok(2)
            }
            OpCode::OpTailCall => {
                let num_args = Self::read_u8_fast(instructions, ip + 1);
                let callee_idx = self.sp - 1 - num_args;
                let is_builtin = matches!(self.stack[callee_idx], Value::Builtin(_));
                self.execute_tail_call(num_args)?;
                if is_builtin { Ok(2) } else { Ok(0) }
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
        }
    }
}
