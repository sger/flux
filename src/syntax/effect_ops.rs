use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, type_expr::TypeExpr},
};

/// A single operation declared inside an effect block.
///
/// ```flux
/// effect Console {
///     print: String -> ()
///     read: () -> String
/// }
/// ```
///
/// The type_expr field holds the full operation type as a TypeExpr, typically
/// a function type (String -> ()) or a non function type for constant ops.
#[derive(Debug, Clone)]
pub struct EffectOp {
    pub name: Identifier,
    pub type_expr: TypeExpr,
    pub span: Span,
}
