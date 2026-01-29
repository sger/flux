use crate::bytecode::{
    debug_info::InstructionLocation, emitted_instruction::EmittedInstruction,
    op_code::Instructions,
};

#[derive(Debug, Clone)]
pub struct CompilationScope {
    pub instructions: Instructions,
    pub files: Vec<String>,
    pub locations: Vec<InstructionLocation>,
    pub last_instruction: EmittedInstruction,
    pub previous_instruction: EmittedInstruction,
}

impl CompilationScope {
    pub fn new() -> Self {
        Self {
            instructions: Instructions::new(),
            files: Vec::new(),
            locations: Vec::new(),
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
