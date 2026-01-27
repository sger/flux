use std::rc::Rc;

use crate::runtime::{compiled_function::CompiledFunction, object::Object};

#[derive(Debug, Clone, PartialEq)]
pub struct Closure {
    pub function: Rc<CompiledFunction>,
    pub free: Vec<Object>,
}

impl Closure {
    pub fn new(function: Rc<CompiledFunction>, free: Vec<Object>) -> Self {
        Self { function, free }
    }
}
