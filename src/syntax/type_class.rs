use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, block::Block, effect_expr::EffectExpr, type_expr::TypeExpr},
};
use serde::{Deserialize, Serialize};

/// A type class constraint like `Eq<a>` or `Ord<a>`.
///
/// Used in superclass declarations (`class Eq<a> => Ord<a>`) and instance
/// contexts (`instance Eq<a> => Eq<List<a>>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// An associated type declared by a class (Proposal 0179 Stage 6).
///
/// ```flux
/// class Collection<c> {
///     type Element<c>
///     fn first(xs: c) -> Element<c>
/// }
/// ```
///
/// `params` repeats the class parameters the type is indexed by, so the
/// declaration and each instance's [`AssociatedTypeEquation`] have visibly the
/// same shape. A use of `Element<c>` in a method signature is an application of
/// this declaration, not a reference to an ordinary type constructor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssociatedTypeDecl {
    pub name: Identifier,
    pub params: Vec<TypeExpr>,
    pub span: Span,
}

/// One instance's definition of an associated type (Proposal 0179 Stage 6).
///
/// ```flux
/// instance Collection<List<a>> {
///     type Element<List<a>> = a
///     fn first(xs) { head(xs) }
/// }
/// ```
///
/// `head` repeats the instance head the equation applies to and `body` is what
/// the application reduces to. Every variable in `body` must be bound by
/// `head`, otherwise the reduction would produce a type out of nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssociatedTypeEquation {
    pub name: Identifier,
    pub head: Vec<TypeExpr>,
    pub body: TypeExpr,
    pub span: Span,
}
