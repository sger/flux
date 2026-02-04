use crate::{bytecode::op_code::OpCode, runtime::object::Object};

use super::VM;

impl VM {
    pub(super) fn execute_comparison(&mut self, opcode: OpCode) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;

        match (&left, &right) {
            (Object::Integer(l), Object::Integer(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    OpCode::OpGreaterThan => l > r,
                    OpCode::OpLessThanOrEqual => l <= r,
                    OpCode::OpGreaterThanOrEqual => l >= r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::Float(l), Object::Float(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    OpCode::OpGreaterThan => l > r,
                    OpCode::OpLessThanOrEqual => l <= r,
                    OpCode::OpGreaterThanOrEqual => l >= r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::Integer(l), Object::Float(r)) => {
                let l = *l as f64;
                let result = match opcode {
                    OpCode::OpEqual => l == *r,
                    OpCode::OpNotEqual => l != *r,
                    OpCode::OpGreaterThan => l > *r,
                    OpCode::OpLessThanOrEqual => l <= *r,
                    OpCode::OpGreaterThanOrEqual => l >= *r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::Float(l), Object::Integer(r)) => {
                let r = *r as f64;
                let result = match opcode {
                    OpCode::OpEqual => *l == r,
                    OpCode::OpNotEqual => *l != r,
                    OpCode::OpGreaterThan => *l > r,
                    OpCode::OpLessThanOrEqual => *l <= r,
                    OpCode::OpGreaterThanOrEqual => *l >= r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::Boolean(l), Object::Boolean(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("unknown boolean comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::String(l), Object::String(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    OpCode::OpGreaterThan => l > r,
                    OpCode::OpLessThanOrEqual => l <= r,
                    OpCode::OpGreaterThanOrEqual => l >= r,
                    _ => return Err(format!("unknown string comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::None, Object::None) => {
                let result = match opcode {
                    OpCode::OpEqual => true,
                    OpCode::OpNotEqual => false,
                    _ => return Err(format!("cannot compare None with {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::None, _) | (_, Object::None) => {
                let result = match opcode {
                    OpCode::OpEqual => false,
                    OpCode::OpNotEqual => true,
                    _ => return Err(format!("cannot compare None with {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            // Some comparison
            (Object::Some(l), Object::Some(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("cannot compare Some with {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            // Left comparison
            (Object::Left(l), Object::Left(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("cannot compare Left with {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            // Right comparison
            (Object::Right(l), Object::Right(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("cannot compare Right with {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            // Left vs Right (always not equal)
            (Object::Left(_), Object::Right(_)) | (Object::Right(_), Object::Left(_)) => {
                let result = match opcode {
                    OpCode::OpEqual => false,
                    OpCode::OpNotEqual => true,
                    _ => return Err(format!("cannot compare Left with Right using {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            _ => Err(format!(
                "unsupported comparison: {} and {}",
                left.type_name(),
                right.type_name()
            )),
        }
    }
}
