//! Internal inference-time type AST used by the HM type checker.

use std::{
    collections::{HashSet, VecDeque},
    fmt,
};

use serde::{Deserialize, Serialize};

use crate::{
    syntax::symbol::Symbol,
    types::{
        TypeVarId, infer_effect_row::InferEffectRow, type_constructor::TypeConstructor,
        type_subst::TypeSubst,
    },
};

/// The compile-time type representation used by the HM type checker.
///
/// This is distinct from `RuntimeType` (used for VM boundary checks) and
/// `TypeExpr` (the surface-syntax annotation AST).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InferType {
    /// Unification variable. Resolved via `Subst`.
    Var(TypeVarId),
    /// Concrete nullary type: `Int`, `String`, …
    Con(TypeConstructor),
    /// Type application: `List<T>`, `Map<K, V>`, `Option<T>`, `Adt<T>`.
    App(TypeConstructor, Vec<InferType>),
    /// Function type: `(T, U) -> V with E1, E2`.
    Fun(Vec<InferType>, Box<InferType>, InferEffectRow),
    /// Tuple type: `(Int, String)`.
    Tuple(Vec<InferType>),
    /// Higher-kinded type application: `f<a>` where `f` is a type variable.
    ///
    /// Distinguished from `App` because the head is an arbitrary `InferType`
    /// (typically `Var`) rather than a concrete `TypeConstructor`. During
    /// substitution, `HktApp(Con(tc), args)` collapses to `App(tc, args)`.
    HktApp(Box<InferType>, Vec<InferType>),
}

impl InferType {
    /// Collect all `Symbol`s (from `Adt` constructors and effect names).
    pub fn collect_symbols(&self, out: &mut HashSet<Symbol>) {
        match self {
            InferType::Var(_) => {}
            InferType::Con(tc) | InferType::App(tc, _) => {
                tc.collect_symbols(out);
                if let InferType::App(_, args) = self {
                    for arg in args {
                        arg.collect_symbols(out);
                    }
                }
            }
            InferType::Fun(params, ret, effects) => {
                for p in params {
                    p.collect_symbols(out);
                }
                ret.collect_symbols(out);
                effects.collect_symbols(out);
            }
            InferType::Tuple(elems) => {
                for e in elems {
                    e.collect_symbols(out);
                }
            }
            InferType::HktApp(head, args) => {
                head.collect_symbols(out);
                for arg in args {
                    arg.collect_symbols(out);
                }
            }
        }
    }

    /// Replace Symbol IDs according to `remap`. Returns a new type.
    pub fn remap_symbols(&self, remap: &std::collections::HashMap<Symbol, Symbol>) -> Self {
        match self {
            InferType::Var(v) => InferType::Var(*v),
            InferType::Con(tc) => InferType::Con(tc.remap_symbols(remap)),
            InferType::App(tc, args) => InferType::App(
                tc.remap_symbols(remap),
                args.iter().map(|a| a.remap_symbols(remap)).collect(),
            ),
            InferType::Fun(params, ret, effects) => InferType::Fun(
                params.iter().map(|p| p.remap_symbols(remap)).collect(),
                Box::new(ret.remap_symbols(remap)),
                effects.remap_symbols(remap),
            ),
            InferType::Tuple(elems) => {
                InferType::Tuple(elems.iter().map(|e| e.remap_symbols(remap)).collect())
            }
            InferType::HktApp(head, args) => InferType::HktApp(
                Box::new(head.remap_symbols(remap)),
                args.iter().map(|a| a.remap_symbols(remap)).collect(),
            ),
        }
    }

    /// Collect all free (unbound) type variables.
    pub fn free_vars(&self) -> HashSet<TypeVarId> {
        let mut set = HashSet::new();
        self.collect_type_free_vars(&mut set);
        self.collect_row_free_vars(&mut set);
        set
    }

    pub fn free_type_vars(&self) -> HashSet<TypeVarId> {
        let mut set = HashSet::new();
        self.collect_type_free_vars(&mut set);
        set
    }

    fn collect_type_free_vars(&self, acc: &mut HashSet<TypeVarId>) {
        match self {
            InferType::Var(v) => {
                acc.insert(*v);
            }
            InferType::Con(_) => {}
            InferType::App(_, args) => {
                for arg in args {
                    arg.collect_type_free_vars(acc);
                }
            }
            InferType::Fun(params, ret, _) => {
                for param in params {
                    param.collect_type_free_vars(acc);
                }
                ret.collect_type_free_vars(acc);
            }
            InferType::Tuple(elements) => {
                for element in elements {
                    element.collect_type_free_vars(acc);
                }
            }
            InferType::HktApp(head, args) => {
                head.collect_type_free_vars(acc);
                for arg in args {
                    arg.collect_type_free_vars(acc);
                }
            }
        }
    }

    fn collect_row_free_vars(&self, acc: &mut HashSet<TypeVarId>) {
        match self {
            InferType::Fun(params, ret, effects) => {
                for param in params {
                    param.collect_row_free_vars(acc);
                }
                ret.collect_row_free_vars(acc);
                acc.extend(effects.free_row_vars());
            }
            InferType::App(_, args) | InferType::Tuple(args) => {
                for arg in args {
                    arg.collect_row_free_vars(acc);
                }
            }
            InferType::HktApp(head, args) => {
                head.collect_row_free_vars(acc);
                for arg in args {
                    arg.collect_row_free_vars(acc);
                }
            }
            InferType::Var(_) | InferType::Con(_) => {}
        }
    }

    /// Apply a substitution, replacing any `Var(v)` that appears in `TypeSubst`.
    pub fn apply_type_subst(&self, type_subst: &TypeSubst) -> InferType {
        self.apply_type_subst_with_seen(type_subst, &mut VecDeque::new())
    }

    fn apply_type_subst_with_seen(
        &self,
        type_subst: &TypeSubst,
        seen_vars: &mut VecDeque<TypeVarId>,
    ) -> InferType {
        match self {
            InferType::Var(v) => {
                if seen_vars.contains(v) {
                    // Break substitution cycles (e.g. ?1 -> ?1 or ?1 -> ?2 -> ?1).
                    return InferType::Var(*v);
                }
                if let Some(infer_type) = type_subst.get(*v) {
                    seen_vars.push_back(*v);
                    let applied = infer_type.apply_type_subst_with_seen(type_subst, seen_vars);
                    seen_vars.pop_back();
                    applied
                } else {
                    InferType::Var(*v)
                }
            }
            InferType::Con(_) => self.clone(),
            InferType::App(con, args) => InferType::App(
                con.clone(),
                args.iter()
                    .map(|a| a.apply_type_subst_with_seen(type_subst, seen_vars))
                    .collect(),
            ),
            InferType::Fun(params, ret, effects) => InferType::Fun(
                params
                    .iter()
                    .map(|p| p.apply_type_subst_with_seen(type_subst, seen_vars))
                    .collect(),
                Box::new(ret.apply_type_subst_with_seen(type_subst, seen_vars)),
                effects.apply_row_subst(type_subst),
            ),
            InferType::Tuple(elements) => InferType::Tuple(
                elements
                    .iter()
                    .map(|e| e.apply_type_subst_with_seen(type_subst, seen_vars))
                    .collect(),
            ),
            InferType::HktApp(head, args) => {
                let resolved_head = head.apply_type_subst_with_seen(type_subst, seen_vars);
                let resolved_args: Vec<InferType> = args
                    .iter()
                    .map(|a| a.apply_type_subst_with_seen(type_subst, seen_vars))
                    .collect();
                // Beta-reduction: if head resolved to a concrete constructor,
                // collapse HktApp(Con(tc), args) → App(tc, args).
                match resolved_head {
                    InferType::Con(tc) => InferType::App(tc, resolved_args),
                    other => InferType::HktApp(Box::new(other), resolved_args),
                }
            }
        }
    }

    /// Returns `true` if this type is fully concrete (no `Var` nodes).
    ///
    /// Uses a short-circuit walk that exits on the first `Var` found,
    /// avoiding the full `HashSet` allocation of `free_vars().is_empty()`.
    pub fn is_concrete(&self) -> bool {
        !self.contains_var()
    }

    fn contains_var(&self) -> bool {
        match self {
            InferType::Var(_) => true,
            InferType::Con(_) => false,
            InferType::App(_, args) | InferType::Tuple(args) => {
                args.iter().any(InferType::contains_var)
            }
            InferType::Fun(params, ret, effects) => {
                params.iter().any(InferType::contains_var)
                    || ret.contains_var()
                    || effects.tail().is_some()
            }
            InferType::HktApp(head, args) => {
                head.contains_var() || args.iter().any(InferType::contains_var)
            }
        }
    }

    /// Extract parameter types from a function type.
    /// Returns an empty slice for non-function types.
    pub fn param_types(&self) -> &[InferType] {
        match self {
            InferType::Fun(params, _, _) => params,
            _ => &[],
        }
    }

    /// Returns `true` if this type contains any `Var` node whose ID is NOT
    /// in the provided `bound_vars` set.
    ///
    /// This distinguishes legitimately polymorphic variables (quantified in a
    /// scheme's `forall`) from unresolved inference fallback variables.
    pub fn contains_unresolved_var(&self, bound_vars: &HashSet<TypeVarId>) -> bool {
        match self {
            InferType::Var(v) => !bound_vars.contains(v),
            InferType::Con(_) => false,
            InferType::App(_, args) | InferType::Tuple(args) => {
                args.iter().any(|a| a.contains_unresolved_var(bound_vars))
            }
            InferType::Fun(params, ret, effects) => {
                params.iter().any(|p| p.contains_unresolved_var(bound_vars))
                    || ret.contains_unresolved_var(bound_vars)
                    || effects.tail().is_some_and(|t| !bound_vars.contains(&t))
            }
            InferType::HktApp(head, args) => {
                head.contains_unresolved_var(bound_vars)
                    || args.iter().any(|a| a.contains_unresolved_var(bound_vars))
            }
        }
    }
}

impl fmt::Display for InferType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InferType::Var(v) => write!(f, "?{v}"),
            InferType::Con(c) => write!(f, "{c}"),
            InferType::App(con, args) => {
                write!(f, "{con}<")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                write!(f, ">")
            }
            InferType::Fun(params, ret, effects) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                if effects.concrete().is_empty() && effects.tail().is_none() {
                    write!(f, ") -> {ret}")
                } else {
                    write!(f, ") -> {ret} with ")?;
                    let mut concrete: Vec<_> = effects.concrete().iter().copied().collect();
                    concrete.sort_by_key(|symbol| symbol.as_u32());
                    for (i, effect) in concrete.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{effect}")?;
                    }
                    if let Some(tail) = effects.tail() {
                        if !effects.concrete().is_empty() {
                            write!(f, " ")?;
                        }
                        write!(f, "|?{tail}")?;
                    }
                    Ok(())
                }
            }
            InferType::Tuple(elements) => {
                write!(f, "(")?;
                for (i, e) in elements.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{e}")?;
                }
                write!(f, ")")
            }
            InferType::HktApp(head, args) => {
                write!(f, "{head}<")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                write!(f, ">")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::InferType;
    use crate::types::{
        infer_effect_row::InferEffectRow, type_constructor::TypeConstructor, type_subst::TypeSubst,
    };

    fn infer_var(id: u32) -> InferType {
        InferType::Var(id)
    }

    fn int() -> InferType {
        InferType::Con(TypeConstructor::Int)
    }

    #[test]
    fn free_vars_collects_and_deduplicates() {
        let infer_type = InferType::Fun(
            vec![
                infer_var(0),
                InferType::Tuple(vec![infer_var(1), infer_var(0)]),
                InferType::App(TypeConstructor::List, vec![infer_var(2)]),
            ],
            Box::new(infer_var(1)),
            InferEffectRow::closed_empty(),
        );

        let got = infer_type.free_vars();
        let expected = HashSet::from([0, 1, 2]);
        assert_eq!(got, expected);
    }

    #[test]
    fn apply_type_subst_rewrites_nested_and_chained_vars() {
        let infer_type = InferType::Fun(
            vec![infer_var(0), InferType::Tuple(vec![infer_var(2)])],
            Box::new(InferType::App(TypeConstructor::Option, vec![infer_var(1)])),
            InferEffectRow::closed_empty(),
        );

        let mut type_subst = TypeSubst::empty();
        type_subst.insert(0, infer_var(1));
        type_subst.insert(1, int());
        type_subst.insert(2, InferType::Con(TypeConstructor::Bool));

        let applied = infer_type.apply_type_subst(&type_subst);
        let expected = InferType::Fun(
            vec![
                int(),
                InferType::Tuple(vec![InferType::Con(TypeConstructor::Bool)]),
            ],
            Box::new(InferType::App(TypeConstructor::Option, vec![int()])),
            InferEffectRow::closed_empty(),
        );
        assert_eq!(applied, expected);
    }

    #[test]
    fn concrete_checks() {
        let concrete = InferType::Tuple(vec![int(), InferType::Con(TypeConstructor::Bool)]);
        assert!(concrete.is_concrete());

        let not_concrete = InferType::Fun(
            vec![infer_var(0)],
            Box::new(int()),
            InferEffectRow::closed_empty(),
        );
        assert!(!not_concrete.is_concrete());
    }

    #[test]
    fn contains_unresolved_var_bound_var_returns_false() {
        let ty = infer_var(0);
        let bound = HashSet::from([0]);
        assert!(!ty.contains_unresolved_var(&bound));
    }

    #[test]
    fn contains_unresolved_var_unbound_var_returns_true() {
        let ty = infer_var(0);
        let bound = HashSet::new();
        assert!(ty.contains_unresolved_var(&bound));
    }

    #[test]
    fn contains_unresolved_var_wrong_bound_returns_true() {
        let ty = infer_var(0);
        let bound = HashSet::from([1]);
        assert!(ty.contains_unresolved_var(&bound));
    }

    #[test]
    fn contains_unresolved_var_concrete_always_false() {
        let ty = int();
        assert!(!ty.contains_unresolved_var(&HashSet::new()));

        let list_ty = InferType::App(TypeConstructor::List, vec![int()]);
        assert!(!list_ty.contains_unresolved_var(&HashSet::new()));

        let tuple_ty = InferType::Tuple(vec![int(), InferType::Con(TypeConstructor::String)]);
        assert!(!tuple_ty.contains_unresolved_var(&HashSet::new()));
    }

    #[test]
    fn contains_unresolved_var_mixed_bound_unbound() {
        // Fun([Var(0)], Var(1)) with bound {0} → true (var 1 unresolved)
        let ty = InferType::Fun(
            vec![infer_var(0)],
            Box::new(infer_var(1)),
            InferEffectRow::closed_empty(),
        );
        let bound = HashSet::from([0]);
        assert!(ty.contains_unresolved_var(&bound));
    }

    #[test]
    fn contains_unresolved_var_all_bound_returns_false() {
        let ty = InferType::Fun(
            vec![infer_var(0)],
            Box::new(infer_var(1)),
            InferEffectRow::closed_empty(),
        );
        let bound = HashSet::from([0, 1]);
        assert!(!ty.contains_unresolved_var(&bound));
    }

    #[test]
    fn contains_unresolved_var_nested_types() {
        let ty = InferType::App(TypeConstructor::List, vec![infer_var(2)]);
        assert!(ty.contains_unresolved_var(&HashSet::new()));
        assert!(!ty.contains_unresolved_var(&HashSet::from([2])));
    }

    #[test]
    fn contains_unresolved_var_effect_tail() {
        let ty = InferType::Fun(
            vec![],
            Box::new(int()),
            InferEffectRow::open_from_symbols(std::iter::empty(), 3),
        );
        // Tail var 3 is unbound
        assert!(ty.contains_unresolved_var(&HashSet::new()));
        // Tail var 3 is bound
        assert!(!ty.contains_unresolved_var(&HashSet::from([3])));
    }

    #[test]
    fn contains_unresolved_var_hkt_app() {
        let ty = InferType::HktApp(Box::new(infer_var(0)), vec![infer_var(1)]);
        assert!(ty.contains_unresolved_var(&HashSet::new()));
        assert!(ty.contains_unresolved_var(&HashSet::from([0])));
        assert!(!ty.contains_unresolved_var(&HashSet::from([0, 1])));
    }

    #[test]
    fn display_formats_composite_types() {
        let infer_type = InferType::Fun(
            vec![
                InferType::App(TypeConstructor::List, vec![int()]),
                InferType::Tuple(vec![InferType::Con(TypeConstructor::Bool), infer_var(3)]),
            ],
            Box::new(InferType::App(
                TypeConstructor::Map,
                vec![InferType::Con(TypeConstructor::String), int()],
            )),
            InferEffectRow::closed_empty(),
        );

        assert_eq!(
            infer_type.to_string(),
            "(List<Int>, (Bool, ?3)) -> Map<String, Int>"
        );
    }

    #[test]
    fn apply_type_subst_breaks_cycles() {
        let infer_type = infer_var(0);
        let mut type_subst = TypeSubst::empty();
        type_subst.insert(0, infer_var(1));
        type_subst.insert(1, infer_var(0));

        let applied = infer_type.apply_type_subst(&type_subst);
        assert!(matches!(applied, InferType::Var(0) | InferType::Var(1)));
    }

    #[test]
    fn collect_symbols_finds_adt_and_effects() {
        use crate::syntax::symbol::Symbol;

        let adt_sym = Symbol::new(7);
        let effect_sym = Symbol::new(42);
        let infer_type = InferType::Fun(
            vec![InferType::App(TypeConstructor::Adt(adt_sym), vec![int()])],
            Box::new(int()),
            InferEffectRow::closed_from_symbols([effect_sym]),
        );

        let mut out = HashSet::new();
        infer_type.collect_symbols(&mut out);
        assert!(out.contains(&adt_sym));
        assert!(out.contains(&effect_sym));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn collect_symbols_empty_for_primitive_types() {
        use crate::syntax::symbol::Symbol;

        let infer_type = InferType::Fun(
            vec![int(), InferType::Con(TypeConstructor::Bool)],
            Box::new(InferType::Tuple(vec![int(), infer_var(0)])),
            InferEffectRow::closed_empty(),
        );

        let mut out = HashSet::<Symbol>::new();
        infer_type.collect_symbols(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn remap_symbols_rewrites_adt_and_effects() {
        use crate::syntax::symbol::Symbol;
        use std::collections::HashMap;

        let old_adt = Symbol::new(5);
        let new_adt = Symbol::new(50);
        let old_effect = Symbol::new(10);
        let new_effect = Symbol::new(100);

        let infer_type = InferType::Fun(
            vec![InferType::App(TypeConstructor::Adt(old_adt), vec![int()])],
            Box::new(InferType::Con(TypeConstructor::Adt(old_adt))),
            InferEffectRow::closed_from_symbols([old_effect]),
        );

        let remap = HashMap::from([(old_adt, new_adt), (old_effect, new_effect)]);
        let remapped = infer_type.remap_symbols(&remap);

        let expected = InferType::Fun(
            vec![InferType::App(TypeConstructor::Adt(new_adt), vec![int()])],
            Box::new(InferType::Con(TypeConstructor::Adt(new_adt))),
            InferEffectRow::closed_from_symbols([new_effect]),
        );
        assert_eq!(remapped, expected);
    }

    #[test]
    fn remap_symbols_preserves_var_and_builtin_types() {
        use crate::syntax::symbol::Symbol;
        use std::collections::HashMap;

        let infer_type = InferType::Fun(
            vec![infer_var(0), int()],
            Box::new(InferType::Tuple(vec![
                InferType::Con(TypeConstructor::Bool),
                InferType::App(TypeConstructor::List, vec![infer_var(1)]),
            ])),
            InferEffectRow::closed_empty(),
        );

        let remap = HashMap::from([(Symbol::new(99), Symbol::new(100))]);
        let remapped = infer_type.remap_symbols(&remap);
        assert_eq!(remapped, infer_type);
    }
}
