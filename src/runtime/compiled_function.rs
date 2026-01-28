use crate::bytecode::op_code::Instructions;
use crate::runtime::leak_detector;

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledFunction {
    pub instructions: Instructions,
    pub num_locals: usize,
    pub num_parameters: usize,
}

impl CompiledFunction {
    pub fn new(instructions: Instructions, num_locals: usize, num_parameters: usize) -> Self {
        leak_detector::record_compiled_function();
        Self {
            instructions,
            num_locals,
            num_parameters,
        }
    }
}
