use crate::{bytecode::op_code::Instructions, runtime::object::Object};

#[derive(Debug, Clone)]
pub struct Bytecode {
    pub instructions: Instructions,
    pub constants: Vec<Object>,
}
