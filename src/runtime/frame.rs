use std::rc::Rc;

use crate::{bytecode::op_code::Instructions, runtime::closure::Closure};

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub closure: Rc<Closure>,
    pub ip: usize,
    pub base_pointer: usize,
    pub return_slot: usize,
}

impl Frame {
    pub fn new(closure: Rc<Closure>, base_pointer: usize) -> Self {
        Self::new_with_return_slot(closure, base_pointer, base_pointer.saturating_sub(1))
    }

    pub fn new_with_return_slot(
        closure: Rc<Closure>,
        base_pointer: usize,
        return_slot: usize,
    ) -> Self {
        Self {
            closure,
            ip: 0,
            base_pointer,
            return_slot,
        }
    }

    pub fn instructions(&self) -> &Instructions {
        &self.closure.function.instructions
    }
}
