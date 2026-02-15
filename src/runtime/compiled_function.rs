use crate::bytecode::{debug_info::FunctionDebugInfo, op_code::Instructions};
use crate::runtime::leak_detector;

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledFunction {
    pub instructions: Instructions,
    pub num_locals: usize,
    pub num_parameters: usize,
    pub max_stack: usize,
    pub debug_info: Option<FunctionDebugInfo>,
}

impl CompiledFunction {
    pub fn new(
        instructions: Instructions,
        num_locals: usize,
        num_parameters: usize,
        debug_info: Option<FunctionDebugInfo>,
    ) -> Self {
        leak_detector::record_compiled_function();
        Self {
            instructions,
            num_locals,
            num_parameters,
            // Default stack frame requirement: locals. Compiler can raise this later.
            max_stack: num_locals,
            debug_info,
        }
    }
}
