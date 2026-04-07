use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, block::Block, type_expr::TypeExpr},
};

/// A type class constraint like `Eq<a>` or `Ord<a>`.
///
/// Used in superclass declarations (`class Eq<a> => Ord<a>`) and instance
/// contexts (`instance Eq<a> => Eq<List<a>>`).
#[derive(Debug, Clone)]
pub struct ClassConstraint {
    pub class_name: Identifier,
    pub type_args: Vec<TypeExpr>,
    pub span: Span,
}

/// A method signature in a `class` declaration.
///
/// Methods can be required (no body) or have a default implementation (with body).
///
/// ```flux
/// class Eq<a> {
///     fn eq(x: a, y: a) -> Bool               // required — no default
///     fn neq(x: a, y: a) -> Bool { !eq(x, y) } // default implementation
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ClassMethod {
    pub name: Identifier,
    pub params: Vec<Identifier>,
    pub param_types: Vec<TypeExpr>,
    pub return_type: TypeExpr,
    pub default_body: Option<Block>,
    pub span: Span,
}

/// A method implementation in an `instance` declaration.
///
/// ```flux
/// instance Eq<Int> {
///     fn eq(x, y) { prim_int_eq(x, y) }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct InstanceMethod {
    pub name: Identifier,
    pub params: Vec<Identifier>,
    pub body: Block,
    pub span: Span,
}
