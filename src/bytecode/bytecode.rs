use crate::{
    bytecode::{debug_info::FunctionDebugInfo, op_code::Instructions},
    runtime::object::Object,
};

#[derive(Debug, Clone)]
pub struct Bytecode {
    pub instructions: Instructions,
    pub constants: Vec<Object>,
    pub debug_info: Option<FunctionDebugInfo>,
}
