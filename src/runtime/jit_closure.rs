use crate::runtime::value::Value;

/// Runtime representation of a JIT-compiled callable and its captured values.
#[derive(Debug, Clone, PartialEq)]
pub struct JitClosure {
    pub function_index: usize,
    pub captures: Vec<Value>,
}

impl JitClosure {
    pub fn new(function_index: usize, captures: Vec<Value>) -> Self {
        Self {
            function_index,
            captures,
        }
    }
}
