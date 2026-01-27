use crate::bytecode::{emitted_instruction::EmittedInstruction, op_code::Instructions};

#[derive(Debug, Clone)]
pub struct CompilationScope {
    pub instructions: Instructions,
    pub last_instruction: EmittedInstruction,
    pub previous_instruction: EmittedInstruction,
}

impl CompilationScope {
    pub fn new() -> Self {
        Self {
            instructions: Instructions::new(),
            last_instruction: EmittedInstruction::default(),
            previous_instruction: EmittedInstruction::default(),
        }
    }
}

impl Default for CompilationScope {
    fn default() -> Self {
        Self::new()
    }
}
