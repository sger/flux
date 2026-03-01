use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, type_expr::TypeExpr},
};

#[derive(Debug, Clone)]
pub struct DataVariant {
    pub name: Identifier,
    pub fields: Vec<TypeExpr>,
    pub span: Span,
}
