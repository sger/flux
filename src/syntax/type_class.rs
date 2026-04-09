use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, block::Block, effect_expr::EffectExpr, type_expr::TypeExpr},
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
    /// Per-method type parameters (e.g., `<a, b>` in `fn fmap<a, b>(...)`).
    /// These are in addition to the class-level type parameters.
    pub type_params: Vec<Identifier>,
    pub params: Vec<Identifier>,
    pub param_types: Vec<TypeExpr>,
    pub return_type: TypeExpr,
    /// Declared effect row for the method (Proposal 0151, Phase 4a).
    /// Empty when no `with` clause is present. Acts as a *floor* on
    /// what implementing instances may declare.
    pub effects: Vec<EffectExpr>,
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
    /// Declared effect row for the instance method (Proposal 0151,
    /// Phase 4a). Must be a *superset* of the class method's declared
    /// row (floor semantics) — checked by the E452 walker.
    pub effects: Vec<EffectExpr>,
    pub body: Block,
    pub span: Span,
}
