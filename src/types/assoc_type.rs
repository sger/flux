//! Recognition and reduction of associated type applications
//! (Proposal 0179 Stage 6).
//!
//! An associated type is a type-level function a class declares and each
//! instance defines:
//!
//! ```flux
//! class Collection<c> {
//!     type Element<c>
//! }
//!
//! instance Collection<List<a>> {
//!     type Element<List<a>> = a
//! }
//! ```
//!
//! Surface syntax gives `Element<c>` no special form, so type conversion
//! produces an ordinary constructor application. This module does two things
//! that conversion cannot, because both need the class environment:
//!
//! 1. **Recognition** — rewrite an application of a declared associated type
//!    into [`InferType::Assoc`].
//! 2. **Reduction** — replace an `Assoc` whose arguments select an instance
//!    equation with that equation's body.
//!
//! Reduction deliberately does not live in `apply_type_subst` or `unify_core`:
//! neither has a `ClassEnv`, and threading one through them would put a
//! type-level lookup on every substitution. Inference calls
//! [`normalize_associated_types`] at the points where it holds the
//! environment, and unification then sees either a reduced type or an
//! irreducible application it can compare structurally.

use crate::{
    syntax::interner::Interner,
    types::{
        class_env::{ClassEnv, instantiate_instance_type_expr},
        infer_type::InferType,
        type_constructor::TypeConstructor,
    },
};

/// How many reduction steps one type may take before normalization gives up.
///
/// E483 rejects an equation whose body names the type it defines, but that
/// check is per-equation: two instances could still reduce into each other.
/// The bound keeps normalization total rather than trusting that they cannot.
const REDUCTION_FUEL: usize = 64;

/// Rewrite applications of declared associated types into [`InferType::Assoc`]
/// and reduce the ones whose arguments select an instance equation.
///
/// Irreducible applications are preserved, not reported: `Element<c>` inside a
/// function generic in `c` is not an error, it is a type waiting for the call
/// site that fixes `c`.
pub fn normalize_associated_types(
    ty: &InferType,
    class_env: &ClassEnv,
    interner: &Interner,
) -> InferType {
    normalize(ty, class_env, interner, &mut REDUCTION_FUEL.clone())
}

fn normalize(
    ty: &InferType,
    class_env: &ClassEnv,
    interner: &Interner,
    fuel: &mut usize,
) -> InferType {
    // Normalize the arguments first: an application only becomes reducible once
    // its arguments are concrete enough to match an equation head.
    let ty = match ty {
        InferType::Var(_) | InferType::Con(_) => ty.clone(),
        InferType::App(con, args) => InferType::App(
            con.clone(),
            args.iter()
                .map(|arg| normalize(arg, class_env, interner, fuel))
                .collect(),
        ),
        InferType::Assoc(class_id, name, args) => InferType::Assoc(
            *class_id,
            *name,
            args.iter()
                .map(|arg| normalize(arg, class_env, interner, fuel))
                .collect(),
        ),
        InferType::Tuple(elements) => InferType::Tuple(
            elements
                .iter()
                .map(|element| normalize(element, class_env, interner, fuel))
                .collect(),
        ),
        InferType::Fun(params, ret, effects) => InferType::Fun(
            params
                .iter()
                .map(|param| normalize(param, class_env, interner, fuel))
                .collect(),
            Box::new(normalize(ret, class_env, interner, fuel)),
            effects.clone(),
        ),
        InferType::HktApp(head, args) => InferType::HktApp(
            Box::new(normalize(head, class_env, interner, fuel)),
            args.iter()
                .map(|arg| normalize(arg, class_env, interner, fuel))
                .collect(),
        ),
    };

    let Some(assoc) = recognize(&ty, class_env) else {
        return ty;
    };
    reduce(assoc, class_env, interner, fuel)
}

/// Rewrite an application of a declared associated type into `Assoc`.
///
/// Conversion has no way to tell `Element<c>` from an ordinary parameterized
/// type, so both arrive as an ADT application; only the class environment knows
/// which names were declared as associated types.
fn recognize(ty: &InferType, class_env: &ClassEnv) -> Option<InferType> {
    let (name, args) = match ty {
        InferType::Assoc(_, _, _) => return Some(ty.clone()),
        InferType::Con(TypeConstructor::Adt(name)) => (*name, Vec::new()),
        InferType::App(TypeConstructor::Adt(name), args) => (*name, args.clone()),
        _ => return None,
    };
    let class_id = class_env.associated_type_class(name)?;
    Some(InferType::Assoc(class_id, name, args))
}

/// Replace an `Assoc` with the body of the equation its arguments select.
///
/// Returns the application unchanged when no equation matches — the arguments
/// are not yet concrete enough to choose one, which is the stuck case.
fn reduce(ty: InferType, class_env: &ClassEnv, interner: &Interner, fuel: &mut usize) -> InferType {
    let InferType::Assoc(class_id, name, args) = &ty else {
        return ty;
    };
    if *fuel == 0 {
        return ty;
    }
    let Some((equation, subst)) =
        class_env.associated_type_equation(*class_id, *name, args, interner)
    else {
        return ty;
    };
    let Some(body) = instantiate_instance_type_expr(&equation.body, &subst, interner) else {
        return ty;
    };
    *fuel -= 1;
    // The body may itself mention associated types, so normalize the result.
    normalize(&body, class_env, interner, fuel)
}

/// Whether `ty` still contains an unreduced associated type application.
///
/// Used at the boundaries that require a fully known type, so "this did not
/// reduce" can be reported where it matters instead of leaking into a backend.
pub fn contains_unreduced(ty: &InferType) -> bool {
    match ty {
        InferType::Assoc(_, _, _) => true,
        InferType::Var(_) | InferType::Con(_) => false,
        InferType::App(_, args) | InferType::Tuple(args) => args.iter().any(contains_unreduced),
        InferType::Fun(params, ret, _) => {
            params.iter().any(contains_unreduced) || contains_unreduced(ret)
        }
        InferType::HktApp(head, args) => {
            contains_unreduced(head) || args.iter().any(contains_unreduced)
        }
    }
}
