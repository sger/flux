use std::rc::Rc;

use crate::runtime::{compiled_function::CompiledFunction, leak_detector, value::Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Closure {
    pub function: Rc<CompiledFunction>,
    pub free: Vec<Value>,
}

impl Closure {
    pub fn new(function: Rc<CompiledFunction>, free: Vec<Value>) -> Self {
        leak_detector::record_closure();
        Self { function, free }
    }
}
