use std::rc::Rc;

use crate::runtime::{compiled_function::CompiledFunction, leak_detector, object::Object};

#[derive(Debug, Clone, PartialEq)]
pub struct Closure {
    pub function: Rc<CompiledFunction>,
    pub free: Vec<Object>,
}

impl Closure {
    pub fn new(function: Rc<CompiledFunction>, free: Vec<Object>) -> Self {
        leak_detector::record_closure();
        Self { function, free }
    }
}
