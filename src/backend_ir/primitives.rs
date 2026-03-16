use crate::{
    diagnostics::position::Span, shared_ir::AdtId, syntax::expression::ExprId,
    types::infer_type::InferType,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IrType {
    Any,
    Int,
    Float,
    Bool,
    String,
    List,
    Array,
    Tuple(usize),
    Hash,
    Adt(AdtId),
    Function(usize),
    Unit,
    Never,
}

#[derive(Debug, Clone)]
pub struct IrMetadata {
    pub span: Option<Span>,
    pub inferred_type: Option<InferType>,
    pub expr_id: Option<ExprId>,
}

impl IrMetadata {
    pub fn empty() -> Self {
        Self {
            span: None,
            inferred_type: None,
            expr_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrConst {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IrBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    IAdd,
    ISub,
    IMul,
    IDiv,
    FAdd,
    FSub,
    FMul,
    FDiv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IrTagTest {
    None,
    Some,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IrListTest {
    Empty,
    Cons,
}
