use std::rc::Rc;

use crate::{bytecode::op_code::Instructions, runtime::closure::Closure};

#[derive(Clone)]
pub struct Frame {
    pub closure: Rc<Closure>,
    pub ip: usize,
    pub base_pointer: usize,
}

impl Frame {
    pub fn new(closure: Rc<Closure>, base_pointer: usize) -> Self {
        Self {
            closure,
            ip: 0,
            base_pointer,
        }
    }

    pub fn instructions(&self) -> &Instructions {
        &self.closure.function.instructions
    }
}
