use crate::{
    bytecode::{debug_info::FunctionDebugInfo, op_code::Instructions},
    runtime::value::Value,
};

#[derive(Debug, Clone)]
pub struct Bytecode {
    pub instructions: Instructions,
    pub constants: Vec<Value>,
    pub debug_info: Option<FunctionDebugInfo>,
}
