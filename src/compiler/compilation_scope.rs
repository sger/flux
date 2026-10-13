use crate::bytecode::{
    debug_info::{BreakSite, EffectSummary, InstructionLocation, LocalVar},
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
    /// Debugger stop sites (`-g` only).
    pub sites: Vec<BreakSite>,
    /// Local variables (`-g` only); an open one has `end_ip == usize::MAX`.
    pub locals: Vec<LocalVar>,
    /// For each open block scope, where its locals start in `locals`.
    pub block_marks: Vec<usize>,
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
            sites: Vec::new(),
            locals: Vec::new(),
            block_marks: Vec::new(),
        }
    }
}

impl Default for CompilationScope {
    fn default() -> Self {
        Self::new()
    }
}
