use std::rc::Rc;

use crate::{bytecode::op_code::OpCode, runtime::value::Value};

use super::VM;

impl VM {
    pub(super) fn execute_comparison(&mut self, opcode: OpCode) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;

        // Fast path: pointer equality for Rc-wrapped types
        // If two values share the same Rc pointer, they're guaranteed equal
        if matches!(opcode, OpCode::OpEqual | OpCode::OpNotEqual) {
            let ptr_eq = match (&left, &right) {
                (Value::String(l), Value::String(r)) => Rc::ptr_eq(l, r),
                (Value::Array(l), Value::Array(r)) => Rc::ptr_eq(l, r),
                (Value::Some(l), Value::Some(r)) => Rc::ptr_eq(l, r),
                (Value::Left(l), Value::Left(r)) => Rc::ptr_eq(l, r),
                (Value::Right(l), Value::Right(r)) => Rc::ptr_eq(l, r),
                (Value::Function(l), Value::Function(r)) => Rc::ptr_eq(l, r),
                (Value::Closure(l), Value::Closure(r)) => Rc::ptr_eq(l, r),
                (Value::Gc(l), Value::Gc(r)) => l == r,
                _ => false,
            };
            if ptr_eq {
                let result = opcode == OpCode::OpEqual;
                return self.push(Value::Boolean(result));
            }
        }

        match (&left, &right) {
            (Value::Integer(l), Value::Integer(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    OpCode::OpGreaterThan => l > r,
                    OpCode::OpLessThanOrEqual => l <= r,
                    OpCode::OpGreaterThanOrEqual => l >= r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            (Value::Float(l), Value::Float(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    OpCode::OpGreaterThan => l > r,
                    OpCode::OpLessThanOrEqual => l <= r,
                    OpCode::OpGreaterThanOrEqual => l >= r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            (Value::Integer(l), Value::Float(r)) => {
                let l = *l as f64;
                let result = match opcode {
                    OpCode::OpEqual => l == *r,
                    OpCode::OpNotEqual => l != *r,
                    OpCode::OpGreaterThan => l > *r,
                    OpCode::OpLessThanOrEqual => l <= *r,
                    OpCode::OpGreaterThanOrEqual => l >= *r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            (Value::Float(l), Value::Integer(r)) => {
                let r = *r as f64;
                let result = match opcode {
                    OpCode::OpEqual => *l == r,
                    OpCode::OpNotEqual => *l != r,
                    OpCode::OpGreaterThan => *l > r,
                    OpCode::OpLessThanOrEqual => *l <= r,
                    OpCode::OpGreaterThanOrEqual => *l >= r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            (Value::Boolean(l), Value::Boolean(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("unknown boolean comparison: {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            (Value::String(l), Value::String(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    OpCode::OpGreaterThan => l > r,
                    OpCode::OpLessThanOrEqual => l <= r,
                    OpCode::OpGreaterThanOrEqual => l >= r,
                    _ => return Err(format!("unknown string comparison: {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            (Value::None, Value::None) => {
                let result = match opcode {
                    OpCode::OpEqual => true,
                    OpCode::OpNotEqual => false,
                    _ => return Err(format!("cannot compare None with {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            (Value::None, _) | (_, Value::None) => {
                let result = match opcode {
                    OpCode::OpEqual => false,
                    OpCode::OpNotEqual => true,
                    _ => return Err(format!("cannot compare None with {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            // Some comparison
            (Value::Some(l), Value::Some(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("cannot compare Some with {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            // Left comparison
            (Value::Left(l), Value::Left(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("cannot compare Left with {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            // Right comparison
            (Value::Right(l), Value::Right(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("cannot compare Right with {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            // Left vs Right (always not equal)
            (Value::Left(_), Value::Right(_)) | (Value::Right(_), Value::Left(_)) => {
                let result = match opcode {
                    OpCode::OpEqual => false,
                    OpCode::OpNotEqual => true,
                    _ => return Err(format!("cannot compare Left with Right using {:?}", opcode)),
                };
                self.push(Value::Boolean(result))
            }
            _ => Err(format!(
                "unsupported comparison: {} and {}",
                left.type_name(),
                right.type_name()
            )),
        }
    }
}
