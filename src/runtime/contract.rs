#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeType {
    Any,
    Int,
    Float,
    Bool,
    String,
    Unit,
    Option(Box<RuntimeType>),
    Array(Box<RuntimeType>),
    Map(Box<RuntimeType>, Box<RuntimeType>),
    Tuple(Vec<RuntimeType>),
}
