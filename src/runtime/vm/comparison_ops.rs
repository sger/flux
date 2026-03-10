use std::rc::Rc;

use crate::{bytecode::op_code::OpCode, runtime::value::Value};

use super::VM;

impl VM {
    pub(super) fn compare_values(
        &self,
        left: &Value,
        right: &Value,
        opcode: OpCode,
    ) -> Result<bool, String> {
        if matches!(opcode, OpCode::OpEqual | OpCode::OpNotEqual) {
            let ptr_eq = match (left, right) {
                (Value::String(l), Value::String(r)) => Rc::ptr_eq(l, r),
                (Value::Array(l), Value::Array(r)) => Rc::ptr_eq(l, r),
                (Value::Tuple(l), Value::Tuple(r)) => Rc::ptr_eq(l, r),
                (Value::Some(l), Value::Some(r)) => Rc::ptr_eq(l, r),
                (Value::Left(l), Value::Left(r)) => Rc::ptr_eq(l, r),
                (Value::Right(l), Value::Right(r)) => Rc::ptr_eq(l, r),
                (Value::Function(l), Value::Function(r)) => Rc::ptr_eq(l, r),
                (Value::Closure(l), Value::Closure(r)) => Rc::ptr_eq(l, r),
                (Value::Gc(l), Value::Gc(r)) => l == r,
                _ => false,
            };
            if ptr_eq {
                return Ok(opcode == OpCode::OpEqual);
            }
        }

        match (left, right) {
            (Value::Integer(l), Value::Integer(r)) => match opcode {
                OpCode::OpEqual => Ok(l == r),
                OpCode::OpNotEqual => Ok(l != r),
                OpCode::OpGreaterThan => Ok(l > r),
                OpCode::OpLessThanOrEqual => Ok(l <= r),
                OpCode::OpGreaterThanOrEqual => Ok(l >= r),
                _ => Err(format!("unknown comparison: {:?}", opcode)),
            },
            (Value::Float(l), Value::Float(r)) => match opcode {
                OpCode::OpEqual => Ok(l == r),
                OpCode::OpNotEqual => Ok(l != r),
                OpCode::OpGreaterThan => Ok(l > r),
                OpCode::OpLessThanOrEqual => Ok(l <= r),
                OpCode::OpGreaterThanOrEqual => Ok(l >= r),
                _ => Err(format!("unknown comparison: {:?}", opcode)),
            },
            (Value::Integer(l), Value::Float(r)) => {
                let l = *l as f64;
                match opcode {
                    OpCode::OpEqual => Ok(l == *r),
                    OpCode::OpNotEqual => Ok(l != *r),
                    OpCode::OpGreaterThan => Ok(l > *r),
                    OpCode::OpLessThanOrEqual => Ok(l <= *r),
                    OpCode::OpGreaterThanOrEqual => Ok(l >= *r),
                    _ => Err(format!("unknown comparison: {:?}", opcode)),
                }
            }
            (Value::Float(l), Value::Integer(r)) => {
                let r = *r as f64;
                match opcode {
                    OpCode::OpEqual => Ok(*l == r),
                    OpCode::OpNotEqual => Ok(*l != r),
                    OpCode::OpGreaterThan => Ok(*l > r),
                    OpCode::OpLessThanOrEqual => Ok(*l <= r),
                    OpCode::OpGreaterThanOrEqual => Ok(*l >= r),
                    _ => Err(format!("unknown comparison: {:?}", opcode)),
                }
            }
            (Value::Boolean(l), Value::Boolean(r)) => match opcode {
                OpCode::OpEqual => Ok(l == r),
                OpCode::OpNotEqual => Ok(l != r),
                _ => Err(format!("unknown boolean comparison: {:?}", opcode)),
            },
            (Value::String(l), Value::String(r)) => match opcode {
                OpCode::OpEqual => Ok(l == r),
                OpCode::OpNotEqual => Ok(l != r),
                OpCode::OpGreaterThan => Ok(l > r),
                OpCode::OpLessThanOrEqual => Ok(l <= r),
                OpCode::OpGreaterThanOrEqual => Ok(l >= r),
                _ => Err(format!("unknown string comparison: {:?}", opcode)),
            },
            (Value::Tuple(l), Value::Tuple(r)) => match opcode {
                OpCode::OpEqual => Ok(l == r),
                OpCode::OpNotEqual => Ok(l != r),
                _ => Err(format!("cannot compare Tuple with {:?}", opcode)),
            },
            (Value::None, Value::None) => match opcode {
                OpCode::OpEqual => Ok(true),
                OpCode::OpNotEqual => Ok(false),
                _ => Err(format!("cannot compare None with {:?}", opcode)),
            },
            (Value::None, _) | (_, Value::None) => match opcode {
                OpCode::OpEqual => Ok(false),
                OpCode::OpNotEqual => Ok(true),
                _ => Err(format!("cannot compare None with {:?}", opcode)),
            },
            (Value::Some(l), Value::Some(r)) => match opcode {
                OpCode::OpEqual => Ok(l == r),
                OpCode::OpNotEqual => Ok(l != r),
                _ => Err(format!("cannot compare Some with {:?}", opcode)),
            },
            (Value::Left(l), Value::Left(r)) => match opcode {
                OpCode::OpEqual => Ok(l == r),
                OpCode::OpNotEqual => Ok(l != r),
                _ => Err(format!("cannot compare Left with {:?}", opcode)),
            },
            (Value::Right(l), Value::Right(r)) => match opcode {
                OpCode::OpEqual => Ok(l == r),
                OpCode::OpNotEqual => Ok(l != r),
                _ => Err(format!("cannot compare Right with {:?}", opcode)),
            },
            (Value::Left(_), Value::Right(_)) | (Value::Right(_), Value::Left(_)) => match opcode {
                OpCode::OpEqual => Ok(false),
                OpCode::OpNotEqual => Ok(true),
                _ => Err(format!("cannot compare Left with Right using {:?}", opcode)),
            },
            _ => Err(format!(
                "unsupported comparison: {} and {}",
                left.type_name(),
                right.type_name()
            )),
        }
    }

    pub(super) fn execute_comparison(&mut self, opcode: OpCode) -> Result<(), String> {
        let (left, right) = self.pop_pair_untracked()?;
        let result = self.compare_values(&left, &right, opcode)?;
        self.push(Value::Boolean(result))
    }
}
