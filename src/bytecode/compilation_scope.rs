use crate::bytecode::{
    debug_info::{EffectSummary, InstructionLocation},
    emitted_instruction::EmittedInstruction,
    op_code::Instructions,
};

#[derive(Debug, Clone)]
pub struct CompilationScope {
    pub instructions: Instructions,
    pub files: Vec<String>,
    pub locations: Vec<InstructionLocation>,
    pub effect_summary: EffectSummary,
    pub last_instruction: EmittedInstruction,
    pub previous_instruction: EmittedInstruction,
}

impl CompilationScope {
    pub fn new() -> Self {
        Self {
            instructions: Instructions::new(),
            files: Vec::new(),
            locations: Vec::new(),
            effect_summary: EffectSummary::Pure,
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
