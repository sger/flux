//! Deriving a class predicate from a class-method call (Proposal 0179, Stage 4).
//!
//! A call to a class method must produce the predicate the *class declaration*
//! describes, not whatever the call's leading argument happens to be. Given
//!
//! ```flux
//! class Tagged<a> { fn tag(n: Int, x: a) -> Int }
//! ```
//!
//! the call `tag(1, true)` must yield `Tagged<Bool>`: the class parameter `a`
//! occurs in the *second* value parameter. Before Stage 4 the emitter took the
//! first argument's type and produced `Tagged<Int>`, silently dispatching to
//! the wrong instance.
//!
//! The rule is uniform, and covers result-directed dispatch without a special
//! case. Each class parameter is located by matching the method's declared
//! signature against the call's actual types — value parameters first, then the
//! return type. A parameter mentioned only in the return type (`Parse<a>` with
//! `fn parse(s: String) -> a`) is therefore determined by the expected result.
//!
//! Arguments are matched *before* the return type deliberately. A parameter
//! occurring in both is pinned by the argument, and the return match then only
//! has to agree with it rather than overwrite it.
//!
//! # Why three states and not `Option`
//!
//! [`ClassParamBinding`] distinguishes `Pending` from `Unmentioned` because the
//! two demand opposite treatment. `Pending` means the parameter's position is
//! known but the type there is still an unsolved variable — the wanted
//! constraint is re-substituted after unification (see
//! `finalize_binding_class_constraints`), so it will very likely be refined,
//! and diagnosing it at emission would reject correct programs. `Unmentioned`
//! means the parameter occurs nowhere in the signature, so *no* call can ever
//! determine it; that is a property of the class declaration and is refutable
//! immediately. Collapsing both into `None` is what produced the wrong-guess
//! behaviour this module replaces.

use std::collections::{HashMap, HashSet};

use crate::syntax::Identifier;
use crate::syntax::interner::Interner;
use crate::syntax::type_expr::TypeExpr;
use crate::types::class_env::{ClassDef, ClassEnv, MethodSig};
use crate::types::infer_type::InferType;

/// How a single class type parameter was determined at a call site.
#[derive(Debug, Clone, PartialEq)]
pub enum ClassParamBinding {
    /// Structurally fixed by an argument or the call's result.
    Determined(InferType),
    /// The parameter occurs in the signature, but the actual type at that
    /// position is still an unsolved variable. Carried so later substitution
    /// can refine it in place; never diagnosed at emission.
    Pending(InferType),
    /// The parameter occurs nowhere in this method's signature, so no call to
    /// it can determine the parameter.
    Unmentioned,
}

impl ClassParamBinding {
    /// The type to place in the emitted predicate.
    ///
    /// `Determined` and `Pending` both contribute their type — a `Pending`
    /// variable is exactly what lets unification refine the predicate later.
    /// `Unmentioned` has no type to contribute.
    pub fn type_arg(&self) -> Option<&InferType> {
        match self {
            Self::Determined(ty) | Self::Pending(ty) => Some(ty),
            Self::Unmentioned => None,
        }
    }
}

/// Locate every class type parameter of `class_def` in a call to `method`.
///
/// Returns one binding per entry of `class_def.type_params`, in declaration
/// order, so the result can be used directly as a predicate's type arguments.
///
/// `actual_arg_tys` may be shorter than the method's declared parameter list
/// (an under-applied call still yields whatever the present arguments fix);
/// surplus arguments are ignored.
pub fn class_param_bindings(
    class_def: &ClassDef,
    method: &MethodSig,
    actual_arg_tys: &[InferType],
    actual_result_ty: &InferType,
    interner: &Interner,
    mut fresh_var: impl FnMut() -> InferType,
) -> Vec<ClassParamBinding> {
    // Both the class's parameters and the method's own generics are variables
    // for matching purposes: in `fn fmap<a, b>(xs: f<a>, g: (a) -> b) -> f<b>`
    // the pattern `f<a>` only matches `List<Int>` if `a` is treated as a
    // variable. Only the class parameters are projected out afterwards, so the
    // method's generics cannot leak into the predicate.
    let mut vars: HashSet<Identifier> = class_def.type_params.iter().copied().collect();
    vars.extend(method.type_params.iter().copied());

    let mut subst: HashMap<Identifier, InferType> = HashMap::new();

    // Value parameters first, so a class parameter appearing in both an
    // argument and the return type is pinned by the argument.
    for (declared, actual) in method.param_types.iter().zip(actual_arg_tys) {
        // A failed match is a type error the unifier reports against a better
        // span; it must not stop the remaining positions from contributing.
        // Bindings recorded before a failure stay sound because `match_type`
        // only inserts on a successful leaf match.
        match_type(declared, actual, &vars, &mut subst, interner);
    }
    match_type(
        &method.return_type,
        actual_result_ty,
        &vars,
        &mut subst,
        interner,
    );

    class_def
        .type_params
        .iter()
        .map(|param| match subst.get(param) {
            Some(ty) if is_resolved(ty) => ClassParamBinding::Determined(ty.clone()),
            Some(ty) => ClassParamBinding::Pending(ty.clone()),
            // The parameter occurs in the signature but nothing bound it —
            // typically the actual type is still a bare variable that cannot
            // match a constructor pattern. A fresh variable keeps the predicate
            // well-formed and lets unification refine it, which is exactly what
            // makes a not-yet-known result type work.
            None if mentions_param(method, *param) => ClassParamBinding::Pending(fresh_var()),
            // Genuinely absent from the signature: no call can ever fix it.
            None => ClassParamBinding::Unmentioned,
        })
        .collect()
}

/// True when `ty` carries enough structure to select an instance.
///
/// A bare unification variable does not; anything else does, including a
/// constructor applied to still-unresolved arguments, which narrows the
/// candidate set even before its arguments are known.
fn is_resolved(ty: &InferType) -> bool {
    !matches!(ty, InferType::Var(_))
}

/// True when `param` occurs anywhere in the method's declared signature.
fn mentions_param(method: &MethodSig, param: Identifier) -> bool {
    method
        .param_types
        .iter()
        .chain(std::iter::once(&method.return_type))
        .any(|ty| type_expr_mentions(ty, param))
}

fn type_expr_mentions(expr: &TypeExpr, param: Identifier) -> bool {
    match expr {
        TypeExpr::Named { name, args, .. } => {
            *name == param || args.iter().any(|a| type_expr_mentions(a, param))
        }
        TypeExpr::Tuple { elements, .. } => elements.iter().any(|e| type_expr_mentions(e, param)),
        TypeExpr::Function { params, ret, .. } => {
            params.iter().any(|p| type_expr_mentions(p, param)) || type_expr_mentions(ret, param)
        }
    }
}

/// Match a declared type against an actual one, binding names in `vars`.
///
/// This mirrors `ClassEnv::match_instance_type_expr`, but takes its notion of
/// "is a variable" from an explicit set rather than from the lowercase-initial
/// heuristic that instance-head matching uses. A method-level generic or a
/// lowercase ADT name must not be mistaken for a class parameter.
fn match_type(
    pattern: &TypeExpr,
    actual: &InferType,
    vars: &HashSet<Identifier>,
    subst: &mut HashMap<Identifier, InferType>,
    interner: &Interner,
) -> bool {
    match pattern {
        TypeExpr::Named { name, args, .. } if args.is_empty() && vars.contains(name) => {
            match subst.get(name) {
                // Occurs-consistency: a parameter bound twice must agree. This
                // is what makes the return-type match verify the argument match
                // rather than silently overwrite it.
                Some(bound) => bound == actual,
                None => {
                    subst.insert(*name, actual.clone());
                    true
                }
            }
        }
        // A variable applied to arguments — `f<a>` in a higher-kinded
        // signature. Bind the head to the actual constructor and recurse.
        TypeExpr::Named { name, args, .. } if vars.contains(name) => match actual {
            InferType::App(tc, actual_args) if args.len() == actual_args.len() => {
                let head = InferType::Con(tc.clone());
                let head_ok = match subst.get(name) {
                    Some(bound) => *bound == head,
                    None => {
                        subst.insert(*name, head);
                        true
                    }
                };
                head_ok
                    && args
                        .iter()
                        .zip(actual_args)
                        .all(|(p, a)| match_type(p, a, vars, subst, interner))
            }
            InferType::HktApp(head, actual_args) if args.len() == actual_args.len() => {
                let head_ok = match subst.get(name) {
                    Some(bound) => bound == head.as_ref(),
                    None => {
                        subst.insert(*name, head.as_ref().clone());
                        true
                    }
                };
                head_ok
                    && args
                        .iter()
                        .zip(actual_args)
                        .all(|(p, a)| match_type(p, a, vars, subst, interner))
            }
            _ => false,
        },
        TypeExpr::Named { name, args, .. } => match actual {
            InferType::Con(tc) => {
                args.is_empty() && ClassEnv::type_constructor_matches(*name, tc, interner)
            }
            InferType::App(tc, actual_args) => {
                ClassEnv::type_constructor_matches(*name, tc, interner)
                    && (args.is_empty()
                        || (args.len() == actual_args.len()
                            && args
                                .iter()
                                .zip(actual_args)
                                .all(|(p, a)| match_type(p, a, vars, subst, interner))))
            }
            InferType::HktApp(head, actual_args) => match head.as_ref() {
                InferType::Con(tc) => {
                    ClassEnv::type_constructor_matches(*name, tc, interner)
                        && (args.is_empty()
                            || (args.len() == actual_args.len()
                                && args
                                    .iter()
                                    .zip(actual_args)
                                    .all(|(p, a)| match_type(p, a, vars, subst, interner))))
                }
                _ => false,
            },
            _ => false,
        },
        TypeExpr::Tuple { elements, .. } => match actual {
            InferType::Tuple(actual_elems) => {
                elements.len() == actual_elems.len()
                    && elements
                        .iter()
                        .zip(actual_elems)
                        .all(|(p, a)| match_type(p, a, vars, subst, interner))
            }
            _ => false,
        },
        TypeExpr::Function { params, ret, .. } => match actual {
            InferType::Fun(actual_params, actual_ret, _) => {
                params.len() == actual_params.len()
                    && params
                        .iter()
                        .zip(actual_params)
                        .all(|(p, a)| match_type(p, a, vars, subst, interner))
                    && match_type(ret, actual_ret, vars, subst, interner)
            }
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::position::Span;
    use crate::types::class_id::ModulePath;
    use crate::types::type_constructor::TypeConstructor;

    fn named(interner: &mut Interner, name: &str, args: Vec<TypeExpr>) -> TypeExpr {
        TypeExpr::Named {
            name: interner.intern(name),
            args,
            span: Span::default(),
        }
    }

    fn class(interner: &mut Interner, name: &str, params: &[&str], method: MethodSig) -> ClassDef {
        ClassDef {
            name: interner.intern(name),
            module: ModulePath::EMPTY,
            is_public: false,
            is_builtin: false,
            type_params: params.iter().map(|p| interner.intern(p)).collect(),
            superclasses: Vec::new(),
            methods: vec![method],
            default_methods: Vec::new(),
            span: Span::default(),
        }
    }

    fn method(
        interner: &mut Interner,
        name: &str,
        param_types: Vec<TypeExpr>,
        return_type: TypeExpr,
    ) -> MethodSig {
        let arity = param_types.len();
        MethodSig {
            name: interner.intern(name),
            type_params: Vec::new(),
            param_names: Vec::new(),
            param_types,
            return_type,
            arity,
            effects: Vec::new(),
            default_body: None,
        }
    }

    /// A fresh-variable source that would be a bug if it were ever reached in
    /// tests that expect every slot to be determined.
    fn no_fresh() -> InferType {
        panic!("did not expect a fresh variable to be required");
    }

    /// `class Tagged<a> { fn tag(n: Int, x: a) -> Int }` called as `tag(1, true)`.
    ///
    /// The class parameter sits in the *second* value parameter. Taking the
    /// first argument's type — what the pre-Stage-4 emitter did — yields
    /// `Tagged<Int>` and dispatches to the wrong instance.
    #[test]
    fn binds_a_class_parameter_in_a_non_first_argument() {
        let mut interner = Interner::new();
        let int = named(&mut interner, "Int", vec![]);
        let a = named(&mut interner, "a", vec![]);
        let sig = method(&mut interner, "tag", vec![int.clone(), a], int);
        let def = class(&mut interner, "Tagged", &["a"], sig.clone());

        let bindings = class_param_bindings(
            &def,
            &sig,
            &[
                InferType::Con(TypeConstructor::Int),
                InferType::Con(TypeConstructor::Bool),
            ],
            &InferType::Con(TypeConstructor::Int),
            &interner,
            no_fresh,
        );

        assert_eq!(
            bindings,
            vec![ClassParamBinding::Determined(InferType::Con(
                TypeConstructor::Bool
            ))]
        );
    }

    /// `class Parse<a> { fn parse(s: String) -> a }`.
    ///
    /// Nothing about the argument selects the instance; the parameter is
    /// determined entirely by the expected result.
    #[test]
    fn binds_a_class_parameter_that_occurs_only_in_the_return_type() {
        let mut interner = Interner::new();
        let string = named(&mut interner, "String", vec![]);
        let a = named(&mut interner, "a", vec![]);
        let sig = method(&mut interner, "parse", vec![string], a);
        let def = class(&mut interner, "Parse", &["a"], sig.clone());

        let bindings = class_param_bindings(
            &def,
            &sig,
            &[InferType::Con(TypeConstructor::String)],
            &InferType::Con(TypeConstructor::Int),
            &interner,
            no_fresh,
        );

        assert_eq!(
            bindings,
            vec![ClassParamBinding::Determined(InferType::Con(
                TypeConstructor::Int
            ))]
        );
    }

    /// `class Convert<a, b> { fn convert(x: a) -> b }` yields a predicate with
    /// *both* arguments. The pre-Stage-4 emitter produced an arity-1 predicate,
    /// which could never match a two-parameter instance head.
    #[test]
    fn binds_every_parameter_of_a_multi_parameter_class() {
        let mut interner = Interner::new();
        let a = named(&mut interner, "a", vec![]);
        let b = named(&mut interner, "b", vec![]);
        let sig = method(&mut interner, "convert", vec![a], b);
        let def = class(&mut interner, "Convert", &["a", "b"], sig.clone());

        let bindings = class_param_bindings(
            &def,
            &sig,
            &[InferType::Con(TypeConstructor::Int)],
            &InferType::Con(TypeConstructor::String),
            &interner,
            no_fresh,
        );

        assert_eq!(
            bindings,
            vec![
                ClassParamBinding::Determined(InferType::Con(TypeConstructor::Int)),
                ClassParamBinding::Determined(InferType::Con(TypeConstructor::String)),
            ]
        );
    }

    /// A result type that is not yet known stays `Pending`, never a guess.
    /// The wanted constraint is re-substituted after unification, so a pending
    /// variable is refined rather than diagnosed.
    #[test]
    fn leaves_an_unresolved_result_pending() {
        let mut interner = Interner::new();
        let a = named(&mut interner, "a", vec![]);
        let b = named(&mut interner, "b", vec![]);
        let sig = method(&mut interner, "convert", vec![a], b);
        let def = class(&mut interner, "Convert", &["a", "b"], sig.clone());

        let bindings = class_param_bindings(
            &def,
            &sig,
            &[InferType::Con(TypeConstructor::Int)],
            &InferType::Var(7),
            &interner,
            no_fresh,
        );

        assert_eq!(
            bindings,
            vec![
                ClassParamBinding::Determined(InferType::Con(TypeConstructor::Int)),
                ClassParamBinding::Pending(InferType::Var(7)),
            ]
        );
    }

    /// A class parameter absent from the method signature can never be fixed by
    /// any call. This is a property of the declaration, distinct from `Pending`.
    #[test]
    fn reports_a_parameter_absent_from_the_signature_as_unmentioned() {
        let mut interner = Interner::new();
        let a = named(&mut interner, "a", vec![]);
        let sig = method(&mut interner, "convert", vec![a.clone()], a);
        let def = class(&mut interner, "Convert", &["a", "b"], sig.clone());

        let bindings = class_param_bindings(
            &def,
            &sig,
            &[InferType::Con(TypeConstructor::Int)],
            &InferType::Con(TypeConstructor::Int),
            &interner,
            no_fresh,
        );

        assert_eq!(
            bindings,
            vec![
                ClassParamBinding::Determined(InferType::Con(TypeConstructor::Int)),
                ClassParamBinding::Unmentioned,
            ]
        );
    }

    /// A class parameter nested inside a constructor is reached by recursion:
    /// `fn size(xs: List<a>) -> Int` at `List<Int>` binds `a` to `Int`.
    #[test]
    fn binds_a_class_parameter_nested_in_a_constructor() {
        let mut interner = Interner::new();
        let a = named(&mut interner, "a", vec![]);
        let list_a = named(&mut interner, "List", vec![a]);
        let int = named(&mut interner, "Int", vec![]);
        let sig = method(&mut interner, "size", vec![list_a], int);
        let def = class(&mut interner, "Sizeable", &["a"], sig.clone());

        let bindings = class_param_bindings(
            &def,
            &sig,
            &[InferType::App(
                TypeConstructor::List,
                vec![InferType::Con(TypeConstructor::Int)],
            )],
            &InferType::Con(TypeConstructor::Int),
            &interner,
            no_fresh,
        );

        assert_eq!(
            bindings,
            vec![ClassParamBinding::Determined(InferType::Con(
                TypeConstructor::Int
            ))]
        );
    }

    /// A method-level generic must not be mistaken for a class parameter, and
    /// must not leak into the predicate. `fn convert<t>(x: a, extra: t) -> b`
    /// still yields exactly the class's own two parameters.
    #[test]
    fn does_not_leak_method_generics_into_the_predicate() {
        let mut interner = Interner::new();
        let a = named(&mut interner, "a", vec![]);
        let b = named(&mut interner, "b", vec![]);
        let t = named(&mut interner, "t", vec![]);
        let mut sig = method(&mut interner, "convert", vec![a, t], b);
        sig.type_params = vec![interner.intern("t")];
        let def = class(&mut interner, "Convert", &["a", "b"], sig.clone());

        let bindings = class_param_bindings(
            &def,
            &sig,
            &[
                InferType::Con(TypeConstructor::Int),
                InferType::Con(TypeConstructor::Bool),
            ],
            &InferType::Con(TypeConstructor::String),
            &interner,
            no_fresh,
        );

        assert_eq!(
            bindings,
            vec![
                ClassParamBinding::Determined(InferType::Con(TypeConstructor::Int)),
                ClassParamBinding::Determined(InferType::Con(TypeConstructor::String)),
            ]
        );
    }
}
