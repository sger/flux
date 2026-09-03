use std::{rc::Rc, sync::Arc};

use crate::{bytecode::op_code::OpCode, runtime::value::Value};

use super::VM;

impl VM {
    fn adt_values_equal(&self, left: &Value, right: &Value) -> bool {
        match (left.as_adt(), right.as_adt()) {
            (Some(left_adt), Some(right_adt)) => {
                if left_adt.constructor() != right_adt.constructor() {
                    return false;
                }
                let left_fields = left_adt.fields();
                let right_fields = right_adt.fields();
                if left_fields.len() != right_fields.len() {
                    return false;
                }
                for i in 0..left_fields.len() {
                    if !self.adt_or_value_equal(&left_fields[i], &right_fields[i]) {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn adt_or_value_equal(&self, left: &Value, right: &Value) -> bool {
        if left.type_name() == "Adt" && right.type_name() == "Adt" {
            match (left, right) {
                (Value::AdtUnit(l), Value::AdtUnit(r)) => l == r,
                (Value::AdtUnit(_), _) | (_, Value::AdtUnit(_)) => false,
                _ => self.adt_values_equal(left, right),
            }
        } else {
            left == right
        }
    }

    /// Whether two cons lists hold equal elements in the same order.
    ///
    /// Iterative over the spine: a list is as long as the program made it, and
    /// one stack frame per cell would put a bound on that.
    fn cons_lists_equal(&self, left: &Value, right: &Value) -> bool {
        let mut left = left;
        let mut right = right;
        loop {
            match (left, right) {
                (Value::EmptyList, Value::EmptyList) => return true,
                (Value::Cons(l), Value::Cons(r)) => {
                    if Rc::ptr_eq(l, r) {
                        return true;
                    }
                    if !self.values_structurally_equal(&l.head, &r.head) {
                        return false;
                    }
                    left = &l.tail;
                    right = &r.tail;
                }
                _ => return false,
            }
        }
    }

    /// Equality for one element of a container, which may itself be a list.
    fn values_structurally_equal(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Cons(_) | Value::EmptyList, Value::Cons(_) | Value::EmptyList) => {
                self.cons_lists_equal(left, right)
            }
            _ => self.adt_or_value_equal(left, right),
        }
    }

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
                (Value::Function(l), Value::Function(r)) => Arc::ptr_eq(l, r),
                (Value::Closure(l), Value::Closure(r)) => Rc::ptr_eq(l, r),
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
            // Cons lists compare structurally, like arrays and tuples. Walking
            // the spine iteratively keeps a long list from recursing once per
            // cell. Without this arm `[1, 2] == [1, 2]` reached the catch-all
            // below and trapped with `unsupported comparison: List and List`,
            // even though every other container compares here.
            (Value::Cons(_) | Value::EmptyList, Value::Cons(_) | Value::EmptyList) => {
                match opcode {
                    OpCode::OpEqual => Ok(self.cons_lists_equal(left, right)),
                    OpCode::OpNotEqual => Ok(!self.cons_lists_equal(left, right)),
                    _ => Err(format!("cannot compare List with {:?}", opcode)),
                }
            }
            // Structural, element-by-element, like tuples. Ordering is not
            // defined for arrays, so only equality is offered.
            (Value::Array(l), Value::Array(r)) => match opcode {
                OpCode::OpEqual => Ok(l == r),
                OpCode::OpNotEqual => Ok(l != r),
                _ => Err(format!("cannot compare Array with {:?}", opcode)),
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
            (Value::AdtUnit(_), Value::AdtUnit(_)) | (Value::Adt(_), Value::Adt(_)) => match opcode
            {
                OpCode::OpEqual => Ok(self.adt_or_value_equal(left, right)),
                OpCode::OpNotEqual => Ok(!self.adt_or_value_equal(left, right)),
                _ => Err(format!("cannot compare Adt with {:?}", opcode)),
            },
            (Value::AdtUnit(_), _)
            | (_, Value::AdtUnit(_))
            | (Value::Adt(_), _)
            | (_, Value::Adt(_)) => match opcode {
                OpCode::OpEqual => Ok(false),
                OpCode::OpNotEqual => Ok(true),
                _ => Err(format!("cannot compare Adt with {:?}", opcode)),
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
