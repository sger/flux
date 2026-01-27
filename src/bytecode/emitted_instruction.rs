use crate::bytecode::op_code::OpCode;

#[derive(Debug, Clone, Default)]
pub struct EmittedInstruction {
    pub opcode: Option<OpCode>,
    pub position: usize,
}
