//! Adapter bridging AST patterns + `InferType` scrutinees into the
//! type-agnostic [`pattern_coverage`](super::pattern_coverage) checker.
//!
//! Scope: translates Bool, Option, Either, List, Tuple, and user
//! ADTs including nested ADT field types. Integers, floats, and
//! strings translate to `TyShape::Opaque` (infinite domains; only a
//! wildcard can exhaust them).

use super::pattern_coverage::{Ctor, LitKey, Pat, TyShape};
use crate::syntax::{
    Identifier,
    expression::{Expression, Pattern},
    interner::Interner,
    type_expr::TypeExpr,
};
use crate::types::{infer_type::InferType, type_constructor::TypeConstructor};

/// Callback interface for ADT lookups during `TyShape` translation.
///
/// The adapter stays decoupled from [`super::InferCtx`] state so its
/// logic remains easy to unit-test.
pub(super) trait AdtResolver {
    /// Resolve an ADT by its type-constructor symbol to the full
    /// list of `(constructor_name, field_type_exprs)`. Returns `None`
    /// when the ADT is unknown (e.g. not yet registered).
    fn lookup_adt(&self, adt: Identifier) -> Option<Vec<(String, Vec<TypeExpr>)>>;
}

/// Translate an `InferType` into a [`TyShape`] descriptor for the
/// coverage checker. Returns `TyShape::Opaque` for types the
/// adapter does not model.
pub(super) fn ty_shape_of(
    ty: &InferType,
    adts: &dyn AdtResolver,
    interner: &Interner,
) -> TyShape {
    ty_shape_of_rec(ty, adts, interner, &mut Vec::new())
}

/// Recursive worker backing [`ty_shape_of`]. `visiting` tracks the
/// ADT symbols currently on the translation stack so recursive
/// types like `type Tree = Node(Tree, Tree) | Leaf` collapse to
/// `Opaque` on re-entry instead of infinitely unfolding.
fn ty_shape_of_rec(
    ty: &InferType,
    adts: &dyn AdtResolver,
    interner: &Interner,
    visiting: &mut Vec<Identifier>,
) -> TyShape {
    match ty {
        InferType::Con(TypeConstructor::Bool) => TyShape::Bool,
        InferType::App(TypeConstructor::Option, args) => {
            let inner = args
                .first()
                .map(|t| ty_shape_of_rec(t, adts, interner, visiting))
                .unwrap_or(TyShape::Opaque);
            TyShape::Option(Box::new(inner))
        }
        InferType::App(TypeConstructor::Either, args) => {
            let l = args
                .first()
                .map(|t| ty_shape_of_rec(t, adts, interner, visiting))
                .unwrap_or(TyShape::Opaque);
            let r = args
                .get(1)
                .map(|t| ty_shape_of_rec(t, adts, interner, visiting))
                .unwrap_or(TyShape::Opaque);
            TyShape::Either(Box::new(l), Box::new(r))
        }
        InferType::App(TypeConstructor::List, args) => {
            let inner = args
                .first()
                .map(|t| ty_shape_of_rec(t, adts, interner, visiting))
                .unwrap_or(TyShape::Opaque);
            TyShape::List(Box::new(inner))
        }
        InferType::Tuple(fields) => TyShape::Tuple(
            fields
                .iter()
                .map(|t| ty_shape_of_rec(t, adts, interner, visiting))
                .collect(),
        ),
        InferType::Con(TypeConstructor::Adt(name))
        | InferType::App(TypeConstructor::Adt(name), _) => {
            adt_shape(*name, adts, interner, visiting)
        }
        _ => TyShape::Opaque,
    }
}

/// Translate a user ADT into a [`TyShape::Adt`] with per-field
/// shapes resolved through [`type_expr_shape`]. The `visiting` stack
/// breaks cycles for recursive ADTs like `type Tree = Node(Tree, Tree)
/// | Leaf` — re-entering the same ADT returns `TyShape::Opaque`. The
/// matrix checker's own wildcard-row fast path then terminates
/// coverage recursion safely.
fn adt_shape(
    name: Identifier,
    adts: &dyn AdtResolver,
    interner: &Interner,
    visiting: &mut Vec<Identifier>,
) -> TyShape {
    if visiting.contains(&name) {
        return TyShape::Opaque;
    }
    let Some(ctors) = adts.lookup_adt(name) else {
        return TyShape::Opaque;
    };
    visiting.push(name);
    let shape = TyShape::Adt {
        name: interner.resolve(name).to_string(),
        ctors: ctors
            .into_iter()
            .map(|(n, fields)| {
                (
                    n,
                    fields
                        .iter()
                        .map(|f| type_expr_shape(f, adts, interner, visiting))
                        .collect(),
                )
            })
            .collect(),
    };
    visiting.pop();
    shape
}

/// Translate a surface [`TypeExpr`] into a [`TyShape`] for nested
/// constructor exhaustiveness. Falls back to `Opaque` for function
/// types, unresolved names, or anything else the checker does not
/// model. This is intentionally best-effort — the checker remains
/// sound under `Opaque` (conservative, never false-positive).
fn type_expr_shape(
    expr: &TypeExpr,
    adts: &dyn AdtResolver,
    interner: &Interner,
    visiting: &mut Vec<Identifier>,
) -> TyShape {
    match expr {
        TypeExpr::Tuple { elements, .. } => TyShape::Tuple(
            elements
                .iter()
                .map(|e| type_expr_shape(e, adts, interner, visiting))
                .collect(),
        ),
        TypeExpr::Function { .. } => TyShape::Opaque,
        TypeExpr::Named { name, args, .. } => {
            let label = interner.resolve(*name);
            match label {
                "Bool" => TyShape::Bool,
                "Int" | "Float" | "String" | "Unit" | "Never" | "Char" => TyShape::Opaque,
                "Option" => {
                    let inner = args
                        .first()
                        .map(|a| type_expr_shape(a, adts, interner, visiting))
                        .unwrap_or(TyShape::Opaque);
                    TyShape::Option(Box::new(inner))
                }
                "Either" => {
                    let l = args
                        .first()
                        .map(|a| type_expr_shape(a, adts, interner, visiting))
                        .unwrap_or(TyShape::Opaque);
                    let r = args
                        .get(1)
                        .map(|a| type_expr_shape(a, adts, interner, visiting))
                        .unwrap_or(TyShape::Opaque);
                    TyShape::Either(Box::new(l), Box::new(r))
                }
                "List" => {
                    let inner = args
                        .first()
                        .map(|a| type_expr_shape(a, adts, interner, visiting))
                        .unwrap_or(TyShape::Opaque);
                    TyShape::List(Box::new(inner))
                }
                // User ADT: look up by symbol.
                _ => adt_shape(*name, adts, interner, visiting),
            }
        }
    }
}

/// Translate an AST [`Pattern`] into a [`Pat`]. The interner is
/// used to resolve identifier symbols to their source names so the
/// checker can match them against `TyShape::Adt` entries.
///
/// Named-field patterns (proposal 0152) are normalized to the same
/// positional constructor space — field names are discarded because
/// coverage works over constructors, not labels.
pub(super) fn pat_of(p: &Pattern, interner: &Interner) -> Pat {
    match p {
        Pattern::Wildcard { .. } | Pattern::Identifier { .. } => Pat::Wild,
        Pattern::Literal { expression, .. } => lit_pat(expression),
        Pattern::None { .. } => Pat::nullary(Ctor::None),
        Pattern::Some { pattern, .. } => {
            Pat::Ctor(Ctor::Some, vec![pat_of(pattern, interner)])
        }
        Pattern::Left { pattern, .. } => {
            Pat::Ctor(Ctor::Left, vec![pat_of(pattern, interner)])
        }
        Pattern::Right { pattern, .. } => {
            Pat::Ctor(Ctor::Right, vec![pat_of(pattern, interner)])
        }
        Pattern::EmptyList { .. } => Pat::nullary(Ctor::Nil),
        Pattern::Cons { head, tail, .. } => Pat::Ctor(
            Ctor::Cons,
            vec![pat_of(head, interner), pat_of(tail, interner)],
        ),
        Pattern::Tuple { elements, .. } => Pat::Ctor(
            Ctor::Tuple(elements.len()),
            elements.iter().map(|e| pat_of(e, interner)).collect(),
        ),
        Pattern::Constructor { name, fields, .. } => {
            let ctor_name = interner.resolve(*name).to_string();
            let arity = fields.len();
            Pat::Ctor(
                Ctor::Adt(ctor_name, arity),
                fields.iter().map(|f| pat_of(f, interner)).collect(),
            )
        }
        Pattern::NamedConstructor {
            name,
            fields,
            rest,
            ..
        } => named_ctor_pat(*name, fields, *rest, interner),
    }
}

/// Translate a named-field constructor pattern to positional form.
/// `..` rest-patterns fall back to `Pat::Wild` because the positional
/// arity is partial; this remains sound (over-approximates coverage).
fn named_ctor_pat(
    name: crate::syntax::Identifier,
    fields: &[crate::syntax::expression::NamedFieldPattern],
    rest: bool,
    interner: &Interner,
) -> Pat {
    if rest {
        return Pat::Wild;
    }
    let ctor_name = interner.resolve(name).to_string();
    let arity = fields.len();
    let sub: Vec<Pat> = fields
        .iter()
        .map(|f| {
            f.pattern
                .as_ref()
                .map(|p| pat_of(p, interner))
                .unwrap_or(Pat::Wild)
        })
        .collect();
    Pat::Ctor(Ctor::Adt(ctor_name, arity), sub)
}

/// Lower a literal expression to a `Pat::Ctor(Ctor::Lit(...))` or to
/// a `Bool` constructor for `true`/`false`. Unsupported literal
/// shapes fall back to `Pat::Wild`.
fn lit_pat(expr: &Expression) -> Pat {
    match expr {
        Expression::Boolean { value, .. } => {
            Pat::nullary(Ctor::Bool(*value))
        }
        Expression::Integer { value, .. } => {
            Pat::nullary(Ctor::Lit(LitKey(value.to_string())))
        }
        Expression::String { value, .. } => {
            Pat::nullary(Ctor::Lit(LitKey(format!("\"{value}\""))))
        }
        _ => Pat::Wild,
    }
}
