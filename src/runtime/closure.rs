use std::sync::Arc;

use crate::runtime::{compiled_function::CompiledFunction, leak_detector, value::Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Closure {
    pub function: Arc<CompiledFunction>,
    pub free: Vec<Value>,
}

impl Closure {
    pub fn new(function: Arc<CompiledFunction>, free: Vec<Value>) -> Self {
        leak_detector::record_closure();
        Self { function, free }
    }
}
