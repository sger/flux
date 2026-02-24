/// Unit tests for the HM type inference engine (src/types/).
// ============================================================================
// Helper constructors
// ============================================================================
use flux::types::infer_type::InferType;
use flux::types::scheme::{Scheme, generalize};
use flux::types::type_constructor::TypeConstructor;
use flux::types::type_env::TypeEnv;
use flux::types::type_subst::TypeSubst;
use flux::types::unify_error::{UnifyErrorKind, unify};

fn int() -> InferType {
    InferType::Con(TypeConstructor::Int)
}

fn float() -> InferType {
    InferType::Con(TypeConstructor::Float)
}

fn string() -> InferType {
    InferType::Con(TypeConstructor::String)
}

fn bool_() -> InferType {
    InferType::Con(TypeConstructor::Bool)
}

fn any() -> InferType {
    InferType::Con(TypeConstructor::Any)
}

fn var(v: u32) -> InferType {
    InferType::Var(v)
}

fn list(t: InferType) -> InferType {
    InferType::App(TypeConstructor::List, vec![t])
}

fn option(t: InferType) -> InferType {
    InferType::App(TypeConstructor::Option, vec![t])
}

fn fun(params: Vec<InferType>, ret: InferType) -> InferType {
    InferType::Fun(params, Box::new(ret))
}

fn tuple(elems: Vec<InferType>) -> InferType {
    InferType::Tuple(elems)
}

// ============================================================================
// Unification tests
// ============================================================================

#[test]
fn unify_same_con() {
    assert!(unify(&int(), &int()).is_ok());
    assert!(unify(&string(), &string()).is_ok());
    assert!(unify(&bool_(), &bool_()).is_ok());
}

#[test]
fn unify_con_mismatch() {
    let err = unify(&int(), &string()).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::Mismatch);
}

#[test]
fn unify_var_to_con() {
    let subst = unify(&var(0), &int()).unwrap();
    assert_eq!(subst.get(0), Some(&int()));
}

#[test]
fn unify_con_to_var() {
    let subst = unify(&string(), &var(1)).unwrap();
    assert_eq!(subst.get(1), Some(&string()));
}

#[test]
fn unify_var_to_var_same() {
    // Var(0) = Var(0) → empty substitution
    let subst = unify(&var(0), &var(0)).unwrap();
    assert!(subst.is_empty());
}

#[test]
fn unify_var_to_var_different() {
    // Var(0) = Var(1) → bind one to the other
    let subst = unify(&var(0), &var(1)).unwrap();
    assert!(subst.get(0).is_some() || subst.get(1).is_some());
}

#[test]
fn unify_list_int_list_int() {
    assert!(unify(&list(int()), &list(int())).is_ok());
}

#[test]
fn unify_list_var_list_int() {
    let subst = unify(&list(var(0)), &list(int())).unwrap();
    assert_eq!(subst.get(0), Some(&int()));
}

#[test]
fn unify_list_mismatch() {
    let err = unify(&list(int()), &list(string())).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::Mismatch);
}

#[test]
fn unify_option_var() {
    let subst = unify(&option(var(0)), &option(float())).unwrap();
    assert_eq!(subst.get(0), Some(&float()));
}

#[test]
fn unify_fun_types() {
    // (Int -> String) = (Int -> String)
    assert!(unify(&fun(vec![int()], string()), &fun(vec![int()], string())).is_ok());
}

#[test]
fn unify_fun_with_var_param() {
    // (Var(0) -> Int) = (String -> Int) → {0 → String}
    let subst = unify(&fun(vec![var(0)], int()), &fun(vec![string()], int())).unwrap();
    assert_eq!(subst.get(0), Some(&string()));
}

#[test]
fn unify_fun_with_var_return() {
    // (Int -> Var(0)) = (Int -> Bool) → {0 → Bool}
    let subst = unify(&fun(vec![int()], var(0)), &fun(vec![int()], bool_())).unwrap();
    assert_eq!(subst.get(0), Some(&bool_()));
}

#[test]
fn unify_fun_arity_mismatch() {
    let err = unify(
        &fun(vec![int()], string()),
        &fun(vec![int(), int()], string()),
    )
    .unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::Mismatch);
}

#[test]
fn unify_tuple_match() {
    let subst = unify(
        &tuple(vec![var(0), string()]),
        &tuple(vec![int(), string()]),
    )
    .unwrap();
    assert_eq!(subst.get(0), Some(&int()));
}

#[test]
fn unify_tuple_length_mismatch() {
    let err = unify(&tuple(vec![int(), string()]), &tuple(vec![int()])).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::Mismatch);
}

#[test]
fn unify_any_with_int() {
    // Any is compatible with everything (gradual typing)
    assert!(unify(&any(), &int()).is_ok());
    assert!(unify(&int(), &any()).is_ok());
    assert!(unify(&any(), &list(string())).is_ok());
    assert!(unify(&any(), &var(42)).is_ok());
}

#[test]
fn unify_occurs_check() {
    // Var(0) = List<Var(0)> → infinite type
    let err = unify(&var(0), &list(var(0))).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::OccursCheck(0));
}

#[test]
fn unify_occurs_check_nested() {
    // Var(0) = Option<Var(0)> → infinite type
    let err = unify(&var(0), &option(var(0))).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::OccursCheck(0));
}

// ============================================================================
// Substitution tests
// ============================================================================

#[test]
fn subst_apply_concrete() {
    let subst = TypeSubst::empty();
    assert_eq!(int().apply_type_subst(&subst), int());
}

#[test]
fn subst_apply_var() {
    let mut subst = TypeSubst::empty();
    subst.insert(0, int());
    assert_eq!(var(0).apply_type_subst(&subst), int());
    assert_eq!(var(1).apply_type_subst(&subst), var(1)); // unbound
}

#[test]
fn subst_apply_nested() {
    let mut subst = TypeSubst::empty();
    subst.insert(0, string());
    assert_eq!(list(var(0)).apply_type_subst(&subst), list(string()));
    assert_eq!(
        fun(vec![var(0)], var(0)).apply_type_subst(&subst),
        fun(vec![string()], string())
    );
}

#[test]
fn subst_compose_sequential() {
    // s1 = {0 → Int}, s2 = {1 → Var(0)}
    // s1 ∘ s2 applied to Var(1) should give Int
    let mut s1 = TypeSubst::empty();
    s1.insert(0, int());
    let mut s2 = TypeSubst::empty();
    s2.insert(1, var(0));
    let composed = s1.compose(&s2);
    let result = var(1).apply_type_subst(&composed);
    assert_eq!(result, int());
}

// ============================================================================
// Scheme tests
// ============================================================================

#[test]
fn scheme_mono_no_forall() {
    let s = Scheme::mono(int());
    assert!(s.forall.is_empty());
    assert_eq!(s.infer_type, int());
}

#[test]
fn scheme_instantiate_mono() {
    let s = Scheme::mono(int());
    let mut counter = 0u32;
    let (ty, mapping) = s.instantiate(&mut counter);
    assert_eq!(ty, int());
    assert!(mapping.is_empty());
    assert_eq!(counter, 0); // no fresh vars allocated
}

#[test]
fn scheme_instantiate_poly() {
    // ∀0. 0 → 0  (identity function scheme)
    let s = Scheme {
        forall: vec![0],
        infer_type: fun(vec![var(0)], var(0)),
    };
    let mut counter = 10u32;
    let (ty, mapping) = s.instantiate(&mut counter);
    // Fresh var 10 should replace var 0
    assert_eq!(counter, 11);
    assert_eq!(*mapping.get(&0).unwrap(), 10u32);
    assert_eq!(ty, fun(vec![var(10)], var(10)));
}

#[test]
fn scheme_instantiate_two_vars() {
    // ∀0 1. (0, 1) → 0  (const scheme)
    let s = Scheme {
        forall: vec![0, 1],
        infer_type: fun(vec![var(0), var(1)], var(0)),
    };
    let mut counter = 5u32;
    let (ty, mapping) = s.instantiate(&mut counter);
    assert_eq!(counter, 7); // allocated vars 5 and 6
    let v0 = *mapping.get(&0).unwrap();
    let v1 = *mapping.get(&1).unwrap();
    assert_eq!(ty, fun(vec![var(v0), var(v1)], var(v0)));
}

// ============================================================================
// Generalize tests
// ============================================================================

#[test]
fn generalize_no_free_vars() {
    use std::collections::HashSet;
    // int() has no free vars → scheme has no forall
    let scheme = generalize(&int(), &HashSet::new());
    assert!(scheme.forall.is_empty());
}

#[test]
fn generalize_free_var_not_in_env() {
    use std::collections::HashSet;
    // Var(0) not in env → gets generalized
    let scheme = generalize(&var(0), &HashSet::new());
    assert!(scheme.forall.contains(&0));
    assert_eq!(scheme.infer_type, var(0));
}

#[test]
fn generalize_free_var_in_env() {
    use std::collections::HashSet;
    // Var(0) IS in the env's free vars → not generalized (would be escaping)
    let env_free = HashSet::from([0u32]);
    let scheme = generalize(&var(0), &env_free);
    assert!(scheme.forall.is_empty()); // NOT quantified
}

#[test]
fn generalize_fun_partial() {
    use std::collections::HashSet;
    // (Var(0) -> Var(1)) where Var(0) is in env (fixed) but Var(1) is free
    let env_free = HashSet::from([0u32]);
    let scheme = generalize(&fun(vec![var(0)], var(1)), &env_free);
    assert!(!scheme.forall.contains(&0)); // env var, not quantified
    assert!(scheme.forall.contains(&1)); // free var, quantified
}

// ============================================================================
// TypeEnv bridge tests
// ============================================================================

#[test]
fn type_env_fresh() {
    let mut env = TypeEnv::new();
    let v0 = env.fresh();
    let v1 = env.fresh();
    assert_ne!(v0, v1);
    assert_eq!(v0 + 1, v1);
}

#[test]
fn type_env_bind_lookup() {
    use flux::syntax::interner::Interner;
    let mut env = TypeEnv::new();
    // We need a Symbol. Since Symbol::new is crate-private, use the interner.
    let mut interner = Interner::new();
    let x = interner.intern("x");
    env.bind(x, Scheme::mono(int()));
    assert!(env.lookup(x).is_some());
    assert_eq!(env.lookup(x).unwrap().infer_type, int());
}

#[test]
fn type_env_scope() {
    use flux::syntax::interner::Interner;
    let mut env = TypeEnv::new();
    let mut interner = Interner::new();
    let x = interner.intern("x");
    env.bind(x, Scheme::mono(int()));
    env.enter_scope();
    env.bind(x, Scheme::mono(string())); // shadow in inner scope
    assert_eq!(env.lookup(x).unwrap().infer_type, string());
    env.leave_scope();
    assert_eq!(env.lookup(x).unwrap().infer_type, int()); // outer restored
}

#[test]
fn type_env_free_vars() {
    use flux::syntax::interner::Interner;
    let mut env = TypeEnv::new();
    let mut interner = Interner::new();
    let x = interner.intern("x");
    // Monomorphic type with a free var
    env.bind(x, Scheme::mono(var(42)));
    let fvs = env.free_vars();
    assert!(fvs.contains(&42));
}

#[test]
fn type_env_to_runtime_primitives() {
    use flux::runtime::runtime_type::RuntimeType;
    assert_eq!(
        TypeEnv::to_runtime(&int(), &TypeSubst::empty()),
        RuntimeType::Int
    );
    assert_eq!(
        TypeEnv::to_runtime(&float(), &TypeSubst::empty()),
        RuntimeType::Float
    );
    assert_eq!(
        TypeEnv::to_runtime(&string(), &TypeSubst::empty()),
        RuntimeType::String
    );
    assert_eq!(
        TypeEnv::to_runtime(&bool_(), &TypeSubst::empty()),
        RuntimeType::Bool
    );
    assert_eq!(
        TypeEnv::to_runtime(&any(), &TypeSubst::empty()),
        RuntimeType::Any
    );
    // Unresolved var → Any
    assert_eq!(
        TypeEnv::to_runtime(&var(0), &TypeSubst::empty()),
        RuntimeType::Any
    );
}

#[test]
fn type_env_to_runtime_option() {
    use flux::runtime::runtime_type::RuntimeType;
    let rt = TypeEnv::to_runtime(&option(int()), &TypeSubst::empty());
    assert_eq!(rt, RuntimeType::Option(Box::new(RuntimeType::Int)));
}

#[test]
fn type_env_to_runtime_resolves_var() {
    use flux::runtime::runtime_type::RuntimeType;
    let mut subst = TypeSubst::empty();
    subst.insert(0, int());
    // var(0) with substitution {0 → Int} → Int
    let rt = TypeEnv::to_runtime(&var(0), &subst);
    assert_eq!(rt, RuntimeType::Int);
}
