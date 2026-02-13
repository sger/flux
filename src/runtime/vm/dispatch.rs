use std::rc::Rc;

use crate::{
    bytecode::op_code::{OpCode, read_u8, read_u16, read_u32},
    runtime::{builtins::BUILTINS, gc::HeapObject, leak_detector, value::Value},
};

use super::VM;

impl VM {
    pub(super) fn dispatch_instruction(&mut self, ip: usize, op: OpCode) -> Result<bool, String> {
        match op {
            OpCode::OpCurrentClosure => {
                let closure = self.current_frame().closure.clone();
                self.push(Value::Closure(closure))?;
            }
            OpCode::OpReturnValue => {
                let return_value = self.pop()?;
                let bp = self.pop_frame().base_pointer;
                self.sp = bp - 1;
                self.push(return_value)?;
            }
            OpCode::OpReturn => {
                let bp = self.pop_frame().base_pointer;
                self.sp = bp - 1;
                self.push(Value::None)?;
            }
            OpCode::OpGetLocal => {
                let idx = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 1;
                let bp = self.current_frame().base_pointer;
                self.push(self.stack[bp + idx].clone())?;
            }
            OpCode::OpSetLocal => {
                let idx = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 1;
                let bp = self.current_frame().base_pointer;
                self.stack[bp + idx] = self.pop()?;
            }
            OpCode::OpConsumeLocal => {
                let idx = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 1;
                let bp = self.current_frame().base_pointer;
                let value = std::mem::replace(&mut self.stack[bp + idx], Value::None);
                self.push(value)?;
            }
            OpCode::OpGetFree => {
                let idx = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 1;
                let value = self.current_frame().closure.free[idx].clone();
                self.push(value)?;
            }
            OpCode::OpClosure => {
                let idx = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                let num_free = read_u8(self.current_frame().instructions(), ip + 3) as usize;
                self.current_frame_mut().ip += 3;
                self.push_closure(idx, num_free)?;
            }
            OpCode::OpClosureLong => {
                let idx = read_u32(self.current_frame().instructions(), ip + 1) as usize;
                let num_free = read_u8(self.current_frame().instructions(), ip + 5) as usize;
                self.current_frame_mut().ip += 5;
                self.push_closure(idx, num_free)?;
            }
            OpCode::OpJump => {
                let pos = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip = pos - 1;
            }
            OpCode::OpJumpNotTruthy => {
                let pos = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 2;
                // Peek instead of pop - value stays on stack for short-circuit operators
                let condition = self.stack[self.sp - 1].clone();
                if !condition.is_truthy() {
                    self.current_frame_mut().ip = pos - 1;
                } else {
                    // Only pop if we're NOT jumping (for && operator)
                    self.sp -= 1;
                }
            }
            OpCode::OpJumpTruthy => {
                let pos = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 2;
                // Peek instead of pop - value stays on stack for short-circuit operators
                let condition = self.stack[self.sp - 1].clone();
                if condition.is_truthy() {
                    self.current_frame_mut().ip = pos - 1;
                } else {
                    // Only pop if we're NOT jumping (for || operator)
                    self.sp -= 1;
                }
            }
            OpCode::OpGetGlobal => {
                let idx = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 2;
                self.push(self.globals[idx].clone())?;
            }
            OpCode::OpSetGlobal => {
                let idx = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 2;
                self.globals[idx] = self.pop()?;
            }
            OpCode::OpConstant => {
                let idx = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 2;
                self.push(self.constants[idx].clone())?;
            }
            OpCode::OpConstantLong => {
                let idx = read_u32(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 4;
                self.push(self.constants[idx].clone())?;
            }
            OpCode::OpAdd | OpCode::OpSub | OpCode::OpMul | OpCode::OpDiv | OpCode::OpMod => {
                self.execute_binary_operation(op)?;
            }
            OpCode::OpEqual
            | OpCode::OpNotEqual
            | OpCode::OpGreaterThan
            | OpCode::OpLessThanOrEqual
            | OpCode::OpGreaterThanOrEqual => {
                self.execute_comparison(op)?;
            }
            OpCode::OpBang => {
                let operand = self.pop()?;
                self.push(Value::Boolean(!operand.is_truthy()))?;
            }
            OpCode::OpMinus => {
                let operand = self.pop()?;
                match operand {
                    Value::Integer(val) => self.push(Value::Integer(-val))?,
                    Value::Float(val) => self.push(Value::Float(-val))?,
                    _ => {
                        return Err(format!(
                            "unsupported type for negation: {}",
                            operand.type_name()
                        ));
                    }
                }
            }
            OpCode::OpTrue => self.push(Value::Boolean(true))?,
            OpCode::OpFalse => self.push(Value::Boolean(false))?,
            // Note: OpNull was removed, use OpNone instead
            OpCode::OpIsSome => {
                let value = self.pop()?;
                self.push(Value::Boolean(matches!(value, Value::Some(_))))?;
            }
            OpCode::OpUnwrapSome => {
                let value = self.pop()?;
                match value {
                    Value::Some(inner) => self.push(inner.as_ref().clone())?,
                    _ => {
                        return Err(format!("expected Some(..) but found {}", value.type_name()));
                    }
                }
            }
            OpCode::OpGetBuiltin => {
                let idx = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 1;
                let builtin = BUILTINS[idx].clone();
                self.push(Value::Builtin(builtin))?;
            }
            OpCode::OpCall => {
                let num_args = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 1;
                self.execute_call(num_args)?;
                return Ok(false);
            }
            OpCode::OpTailCall => {
                let num_args = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 1;
                self.execute_tail_call(num_args)?;
                return Ok(false);
            }
            OpCode::OpPop => {
                self.pop()?;
            }
            OpCode::OpArray => {
                let num_elements = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 2;
                let array = self.build_array(self.sp - num_elements, self.sp);
                self.sp -= num_elements;
                self.push(array)?;
            }
            OpCode::OpArrayLong => {
                let num_elements = read_u32(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 4;
                let array = self.build_array(self.sp - num_elements, self.sp);
                self.sp -= num_elements;
                self.push(array)?;
            }
            OpCode::OpHash => {
                let num_elements = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 2;
                let hash = self.build_hash(self.sp - num_elements, self.sp)?;
                self.sp -= num_elements;
                self.push(hash)?;
            }
            OpCode::OpHashLong => {
                let num_elements = read_u32(self.current_frame().instructions(), ip + 1) as usize;
                self.current_frame_mut().ip += 4;
                let hash = self.build_hash(self.sp - num_elements, self.sp)?;
                self.sp -= num_elements;
                self.push(hash)?;
            }
            OpCode::OpIndex => {
                let index = self.pop()?;
                let left = self.pop()?;
                self.execute_index_expression(left, index)?;
            }
            OpCode::OpNone => self.push(Value::None)?,
            OpCode::OpSome => {
                let value = self.pop()?;
                leak_detector::record_some();
                self.push(Value::Some(Rc::new(value)))?;
            }
            // Either type operations
            OpCode::OpLeft => {
                let value = self.pop()?;
                self.push(Value::Left(Rc::new(value)))?;
            }
            OpCode::OpRight => {
                let value = self.pop()?;
                self.push(Value::Right(Rc::new(value)))?;
            }
            OpCode::OpIsLeft => {
                let value = self.pop()?;
                let result = matches!(value, Value::Left(_));
                self.push(Value::Boolean(result))?;
            }
            OpCode::OpIsRight => {
                let value = self.pop()?;
                let result = matches!(value, Value::Right(_));
                self.push(Value::Boolean(result))?;
            }
            OpCode::OpUnwrapLeft => {
                let value = self.pop()?;
                match value {
                    Value::Left(inner) => self.push(inner.as_ref().clone())?,
                    _ => return Err("Cannot unwrap non-Left value".to_string()),
                }
            }
            OpCode::OpUnwrapRight => {
                let value = self.pop()?;
                match value {
                    Value::Right(inner) => self.push(inner.as_ref().clone())?,
                    _ => return Err("Cannot unwrap non-Right value".to_string()),
                }
            }
            OpCode::OpToString => {
                let value = self.pop()?;
                self.push(Value::String(value.to_string_value().into()))?;
            }
            OpCode::OpCons => {
                let tail = self.pop()?;
                let head = self.pop()?;
                let handle = self.gc_alloc(HeapObject::Cons { head, tail });
                self.push(Value::Gc(handle))?;
            }
            OpCode::OpIsCons => {
                let value = self.pop()?;
                let is_cons = matches!(&value, Value::Gc(h) if matches!(self.gc_heap.get(*h), HeapObject::Cons { .. }));
                self.push(Value::Boolean(is_cons))?;
            }
            OpCode::OpIsEmptyList => {
                let value = self.pop()?;
                self.push(Value::Boolean(value == Value::None))?;
            }
            OpCode::OpConsHead => {
                let value = self.pop()?;
                match &value {
                    Value::Gc(h) => match self.gc_heap.get(*h) {
                        HeapObject::Cons { head, .. } => self.push(head.clone())?,
                        other => return Err(format!("head: expected list, got {:?}", other)),
                    },
                    _ => return Err(format!("head: expected list, got {}", value.type_name())),
                }
            }
            OpCode::OpConsTail => {
                let value = self.pop()?;
                match &value {
                    Value::Gc(h) => match self.gc_heap.get(*h) {
                        HeapObject::Cons { tail, .. } => self.push(tail.clone())?,
                        other => return Err(format!("tail: expected list, got {:?}", other)),
                    },
                    _ => return Err(format!("tail: expected list, got {}", value.type_name())),
                }
            }
        }

        Ok(true)
    }
}
