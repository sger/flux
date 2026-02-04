use crate::{
    bytecode::op_code::{read_u8, read_u16, OpCode},
    runtime::{builtins::BUILTINS, leak_detector, object::Object},
};

use super::VM;

impl VM {
    pub(super) fn dispatch_instruction(&mut self, ip: usize, op: OpCode) -> Result<bool, String> {
        match op {
        OpCode::OpCurrentClosure => {
            let closure = self.current_frame().closure.clone();
            self.push(Object::Closure(closure))?;
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
            self.push(Object::None)?;
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
            self.push(Object::Boolean(!operand.is_truthy()))?;
        }
        OpCode::OpMinus => {
            let operand = self.pop()?;
            match operand {
                Object::Integer(val) => self.push(Object::Integer(-val))?,
                Object::Float(val) => self.push(Object::Float(-val))?,
                _ => {
                    return Err(format!(
                        "unsupported type for negation: {}",
                        operand.type_name()
                    ));
                }
            }
        }
        OpCode::OpTrue => self.push(Object::Boolean(true))?,
        OpCode::OpFalse => self.push(Object::Boolean(false))?,
        // Note: OpNull was removed, use OpNone instead
        OpCode::OpIsSome => {
            let value = self.pop()?;
            self.push(Object::Boolean(matches!(value, Object::Some(_))))?;
        }
        OpCode::OpUnwrapSome => {
            let value = self.pop()?;
            match value {
                Object::Some(inner) => self.push(*inner)?,
                _ => {
                    return Err(format!(
                        "expected Some(..) but found {}",
                        value.type_name()
                    ));
                }
            }
        }
        OpCode::OpGetBuiltin => {
            let idx = read_u8(self.current_frame().instructions(), ip + 1) as usize;
            self.current_frame_mut().ip += 1;
            let builtin = BUILTINS[idx].clone();
            self.push(Object::Builtin(builtin))?;
        }
        OpCode::OpCall => {
            let num_args = read_u8(self.current_frame().instructions(), ip + 1) as usize;
            self.current_frame_mut().ip += 1;
            self.execute_call(num_args)?;
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
        OpCode::OpHash => {
            let num_elements = read_u16(self.current_frame().instructions(), ip + 1) as usize;
            self.current_frame_mut().ip += 2;
            let hash = self.build_hash(self.sp - num_elements, self.sp)?;
            self.sp -= num_elements;
            self.push(hash)?;
        }
        OpCode::OpIndex => {
            let index = self.pop()?;
            let left = self.pop()?;
            self.execute_index_expression(left, index)?;
        }
        OpCode::OpNone => self.push(Object::None)?,
        OpCode::OpSome => {
            let value = self.pop()?;
            leak_detector::record_some();
            self.push(Object::Some(Box::new(value)))?;
        }
        // Either type operations
        OpCode::OpLeft => {
            let value = self.pop()?;
            self.push(Object::Left(Box::new(value)))?;
        }
        OpCode::OpRight => {
            let value = self.pop()?;
            self.push(Object::Right(Box::new(value)))?;
        }
        OpCode::OpIsLeft => {
            let value = self.pop()?;
            let result = matches!(value, Object::Left(_));
            self.push(Object::Boolean(result))?;
        }
        OpCode::OpIsRight => {
            let value = self.pop()?;
            let result = matches!(value, Object::Right(_));
            self.push(Object::Boolean(result))?;
        }
        OpCode::OpUnwrapLeft => {
            let value = self.pop()?;
            match value {
                Object::Left(inner) => self.push(*inner)?,
                _ => return Err("Cannot unwrap non-Left value".to_string()),
            }
        }
        OpCode::OpUnwrapRight => {
            let value = self.pop()?;
            match value {
                Object::Right(inner) => self.push(*inner)?,
                _ => return Err("Cannot unwrap non-Right value".to_string()),
            }
        }
        OpCode::OpToString => {
            let value = self.pop()?;
            self.push(Object::String(value.to_string_value()))?;
        }
        }

        Ok(true)
    }
}
