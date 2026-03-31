//! Internal inference-time type AST used by the HM type checker.

use std::{
    collections::{HashSet, VecDeque},
    fmt,
};

use serde::{Deserialize, Serialize};

use crate::types::{
    TypeVarId, infer_effect_row::InferEffectRow, type_constructor::TypeConstructor,
    type_subst::TypeSubst,
};

/// The compile-time type representation used by the HM type checker.
///
/// This is distinct from `RuntimeType` (used for VM boundary checks) and
/// `TypeExpr` (the surface-syntax annotation AST).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InferType {
    /// Unification variable. Resolved via `Subst`.
    Var(TypeVarId),
    /// Concrete nullary type: `Int`, `String`, `Any`, …
    Con(TypeConstructor),
    /// Type application: `List<T>`, `Map<K, V>`, `Option<T>`, `Adt<T>`.
    App(TypeConstructor, Vec<InferType>),
    /// Function type: `(T, U) -> V with E1, E2`.
    Fun(Vec<InferType>, Box<InferType>, InferEffectRow),
    /// Tuple type: `(Int, String)`.
    Tuple(Vec<InferType>),
}

impl InferType {
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
        }
    }

    /// `Any` is the gradual escape hatch.
    pub fn is_any(&self) -> bool {
        matches!(self, InferType::Con(TypeConstructor::Any))
    }

    /// Returns `true` if this type contains `Any` at any nesting depth.
    pub fn contains_any(&self) -> bool {
        match self {
            InferType::Con(TypeConstructor::Any) => true,
            InferType::App(_, args) | InferType::Tuple(args) => {
                args.iter().any(InferType::contains_any)
            }
            InferType::Fun(params, ret, _) => {
                params.iter().any(InferType::contains_any) || ret.contains_any()
            }
            InferType::Var(_) | InferType::Con(_) => false,
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
    fn concrete_and_any_checks() {
        let concrete = InferType::Tuple(vec![int(), InferType::Con(TypeConstructor::Bool)]);
        assert!(concrete.is_concrete());
        assert!(!concrete.is_any());
        assert!(!concrete.contains_any());

        let not_concrete = InferType::Fun(
            vec![infer_var(0)],
            Box::new(int()),
            InferEffectRow::closed_empty(),
        );
        assert!(!not_concrete.is_concrete());
        assert!(!not_concrete.contains_any());

        let any = InferType::Con(TypeConstructor::Any);
        assert!(any.is_any());
        assert!(any.is_concrete());
        assert!(any.contains_any());
    }

    #[test]
    fn contains_any_checks_nested_types() {
        let nested_any_app = InferType::App(
            TypeConstructor::List,
            vec![InferType::Con(TypeConstructor::Any)],
        );
        assert!(nested_any_app.contains_any());

        let nested_any_tuple = InferType::Tuple(vec![int(), InferType::Con(TypeConstructor::Any)]);
        assert!(nested_any_tuple.contains_any());

        let nested_any_fun = InferType::Fun(
            vec![InferType::Con(TypeConstructor::Any)],
            Box::new(int()),
            InferEffectRow::closed_empty(),
        );
        assert!(nested_any_fun.contains_any());

        let nested_any_fun_ret = InferType::Fun(
            vec![int()],
            Box::new(InferType::App(
                TypeConstructor::Option,
                vec![InferType::Con(TypeConstructor::Any)],
            )),
            InferEffectRow::closed_empty(),
        );
        assert!(nested_any_fun_ret.contains_any());
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
}
