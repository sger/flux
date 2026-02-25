use crate::syntax::Identifier;

use crate::runtime::runtime_type::RuntimeType;

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionContract {
    pub params: Vec<Option<RuntimeType>>,
    pub ret: Option<RuntimeType>,
    pub effects: Vec<Identifier>,
}
