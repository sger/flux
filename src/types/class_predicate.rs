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
//! `decide_quantification`), so it will very likely be refined,
//! and diagnosing it at emission would reject correct programs. `Unmentioned`
//! means the parameter occurs nowhere in the signature, so *no* call can ever
//! determine it; that is a property of the class declaration and is refutable
//! immediately. Collapsing both into `None` is what produced the wrong-guess
//! behaviour this module replaces.

use std::collections::HashMap;

use crate::syntax::interner::Interner;
use crate::types::TypeVarId;
use crate::types::class_env::{ClassDef, MethodSig};
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
    let unmentioned = || {
        class_def
            .type_params
            .iter()
            .map(|_| ClassParamBinding::Unmentioned)
            .collect::<Vec<_>>()
    };

    let Some(declared) = method_fun_type(class_def, method, interner) else {
        return unmentioned();
    };
    let InferType::Fun(declared_params, declared_ret, _) = &declared else {
        return unmentioned();
    };

    // Both the class's parameters and the method's own generics are variables
    // for matching purposes: in `fn fmap<a, b>(xs: f<a>, g: (a) -> b) -> f<b>`
    // the pattern `f<a>` only matches `List<Int>` if `a` is treated as a
    // variable. The numbering documented on `MethodSig::infer_type` puts both
    // below `var_count`, and only the class parameters — which come first —
    // are projected out afterwards, so a method generic cannot leak into the
    // predicate.
    let class_count = class_def.type_params.len();
    let var_count = (class_count + method.type_params.len()) as TypeVarId;

    let mut subst: HashMap<TypeVarId, InferType> = HashMap::new();

    // Value parameters first, so a class parameter appearing in both an
    // argument and the return type is pinned by the argument.
    //
    // `actual_arg_tys` may be shorter than the declared list (an under-applied
    // call still yields whatever the present arguments fix); `zip` ignores the
    // surplus on either side. A failed match is a type error the unifier
    // reports against a better span, so it must not stop the remaining
    // positions from contributing — and bindings recorded before a failure
    // stay sound because `match_infer` only inserts on a successful leaf match.
    for (declared, actual) in declared_params.iter().zip(actual_arg_tys) {
        match_infer(declared, actual, var_count, &mut subst);
    }
    match_infer(declared_ret, actual_result_ty, var_count, &mut subst);

    (0..class_count)
        .map(|index| {
            let var = index as TypeVarId;
            match subst.get(&var) {
                Some(ty) if is_resolved(ty) => ClassParamBinding::Determined(ty.clone()),
                Some(ty) => ClassParamBinding::Pending(ty.clone()),
                // The parameter occurs in the signature but nothing bound it —
                // typically the actual type is still a bare variable that cannot
                // match a constructor pattern. A fresh variable keeps the predicate
                // well-formed and lets unification refine it, which is exactly what
                // makes a not-yet-known result type work.
                None if mentions_var(&declared, var) => ClassParamBinding::Pending(fresh_var()),
                // Genuinely absent from the signature: no call can ever fix it.
                None => ClassParamBinding::Unmentioned,
            }
        })
        .collect()
}

/// The method's declared type in the solver's representation.
///
/// Normally this is the conversion recorded when the class was collected. The
/// fallback re-converts, for a `MethodSig` built without one — the synthetic
/// placeholders in dictionary elaboration, and tests.
fn method_fun_type(
    class_def: &ClassDef,
    method: &MethodSig,
    interner: &Interner,
) -> Option<InferType> {
    method.infer_type.clone().or_else(|| {
        crate::types::class_env::method_infer_type(
            &class_def.type_params,
            &method.type_params,
            &method.param_types,
            &method.return_type,
            &method.effects,
            interner,
        )
    })
}

/// True when `ty` carries enough structure to select an instance.
///
/// A bare unification variable does not; anything else does, including a
/// constructor applied to still-unresolved arguments, which narrows the
/// candidate set even before its arguments are known.
fn is_resolved(ty: &InferType) -> bool {
    !matches!(ty, InferType::Var(_))
}

/// True when `var` occurs anywhere in `ty`.
fn mentions_var(ty: &InferType, var: TypeVarId) -> bool {
    match ty {
        InferType::Var(v) => *v == var,
        InferType::Con(_) => false,
        InferType::App(_, args) | InferType::Assoc(_, _, args) | InferType::Tuple(args) => {
            args.iter().any(|a| mentions_var(a, var))
        }
        InferType::HktApp(head, args) => {
            mentions_var(head, var) || args.iter().any(|a| mentions_var(a, var))
        }
        InferType::Fun(params, ret, _) => {
            params.iter().any(|p| mentions_var(p, var)) || mentions_var(ret, var)
        }
    }
}

/// One-sided match of a declared class-method type against an inferred one.
///
/// Only variables below `var_count` — the class's parameters followed by the
/// method's own generics, per the numbering on `MethodSig::infer_type` — are
/// treated as pattern variables. Anything else, including a variable the
/// inferencer created, must match structurally.
///
/// Returns whether the match succeeded. Bindings are inserted only on a
/// successful leaf match, so a partial failure leaves the substitution sound.
fn match_infer(
    pattern: &InferType,
    actual: &InferType,
    var_count: TypeVarId,
    subst: &mut HashMap<TypeVarId, InferType>,
) -> bool {
    /// Records `binding` for `var`, or checks it against what is already there.
    ///
    /// Occurs-consistency: a parameter bound twice must agree. This is what
    /// makes the return-type match verify the argument match rather than
    /// silently overwrite it.
    fn bind(var: TypeVarId, binding: InferType, subst: &mut HashMap<TypeVarId, InferType>) -> bool {
        match subst.get(&var) {
            Some(bound) => *bound == binding,
            None => {
                subst.insert(var, binding);
                true
            }
        }
    }

    fn all_match(
        patterns: &[InferType],
        actuals: &[InferType],
        var_count: TypeVarId,
        subst: &mut HashMap<TypeVarId, InferType>,
    ) -> bool {
        patterns.len() == actuals.len()
            && patterns
                .iter()
                .zip(actuals)
                .all(|(p, a)| match_infer(p, a, var_count, subst))
    }

    match pattern {
        InferType::Var(v) if *v < var_count => bind(*v, actual.clone(), subst),

        // A pattern variable applied to arguments — `f<a>` in a higher-kinded
        // signature. Bind the head to the actual constructor and recurse.
        InferType::HktApp(head, args) => {
            let InferType::Var(v) = head.as_ref() else {
                // A non-variable head is structural; match it as such.
                return match actual {
                    InferType::HktApp(actual_head, actual_args) => {
                        match_infer(head, actual_head, var_count, subst)
                            && all_match(args, actual_args, var_count, subst)
                    }
                    _ => false,
                };
            };
            if *v >= var_count {
                return false;
            }
            match actual {
                InferType::App(tc, actual_args) if args.len() == actual_args.len() => {
                    bind(*v, InferType::Con(tc.clone()), subst)
                        && all_match(args, actual_args, var_count, subst)
                }
                InferType::HktApp(actual_head, actual_args) if args.len() == actual_args.len() => {
                    bind(*v, actual_head.as_ref().clone(), subst)
                        && all_match(args, actual_args, var_count, subst)
                }
                // The pattern applies fewer arguments than the actual type has:
                // `f<a>` against `Either<String, Int>`. The head is then partially
                // applied — `f` is `Either<String>` and `a` is the trailing
                // argument. Without this the arity guards above reject the match,
                // so a class whose parameter is higher-kinded could not be used
                // over a two-parameter constructor at all.
                InferType::App(tc, actual_args) if actual_args.len() > args.len() => {
                    let applied = actual_args.len() - args.len();
                    let head = InferType::HktApp(
                        Box::new(InferType::Con(tc.clone())),
                        actual_args[..applied].to_vec(),
                    );
                    bind(*v, head, subst)
                        && all_match(args, &actual_args[applied..], var_count, subst)
                }
                _ => false,
            }
        }

        InferType::Var(_) => matches!(actual, InferType::Var(v) if pattern == &InferType::Var(*v)),

        InferType::Con(tc) => match actual {
            InferType::Con(actual_tc) => tc == actual_tc,
            // A nullary pattern against an applied type matches on the head
            // alone, as the surface form did: `List` matches `List<Int>`.
            InferType::App(actual_tc, _) => tc == actual_tc,
            InferType::HktApp(head, _) => matches!(head.as_ref(), InferType::Con(h) if h == tc),
            _ => false,
        },

        InferType::App(tc, args) => match actual {
            InferType::App(actual_tc, actual_args) => {
                tc == actual_tc && all_match(args, actual_args, var_count, subst)
            }
            InferType::HktApp(head, actual_args) => {
                matches!(head.as_ref(), InferType::Con(h) if h == tc)
                    && all_match(args, actual_args, var_count, subst)
            }
            _ => false,
        },

        InferType::Tuple(elements) => match actual {
            InferType::Tuple(actual_elements) => {
                all_match(elements, actual_elements, var_count, subst)
            }
            _ => false,
        },

        InferType::Fun(params, ret, _) => match actual {
            InferType::Fun(actual_params, actual_ret, _) => {
                all_match(params, actual_params, var_count, subst)
                    && match_infer(ret, actual_ret, var_count, subst)
            }
            _ => false,
        },

        // An unreduced associated type is not something a call site can pin a
        // class parameter through; `normalize_associated_types` runs first.
        InferType::Assoc(..) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::position::Span;
    use crate::syntax::type_expr::TypeExpr;
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
            superclass_class_ids: Vec::new(),
            associated_types: Vec::new(),
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
            infer_type: None,
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
