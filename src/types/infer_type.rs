//! Internal inference-time type AST used by the HM type checker.

use std::{collections::HashSet, fmt};

use crate::{
    syntax::Identifier,
    types::{TypeVarId, type_constructor::TypeConstructor, type_subst::TypeSubst},
};

/// The compile-time type representation used by the HM type checker.
///
/// This is distinct from `RuntimeType` (used for VM boundary checks) and
/// `TypeExpr` (the surface-syntax annotation AST).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferType {
    /// Unification variable. Resolved via `Subst`.
    Var(TypeVarId),
    /// Concrete nullary type: `Int`, `String`, `Any`, …
    Con(TypeConstructor),
    /// Type application: `List<T>`, `Map<K, V>`, `Option<T>`, `Adt<T>`.
    App(TypeConstructor, Vec<InferType>),
    /// Function type: `(T, U) -> V with E1, E2`.
    Fun(Vec<InferType>, Box<InferType>, Vec<Identifier>),
    /// Tuple type: `(Int, String)`.
    Tuple(Vec<InferType>),
}

impl InferType {
    /// Collect all free (unbound) type variables.
    pub fn free_vars(&self) -> HashSet<TypeVarId> {
        let mut set = HashSet::new();
        self.collect_free_vars(&mut set);
        set
    }

    fn collect_free_vars(&self, acc: &mut HashSet<TypeVarId>) {
        match self {
            InferType::Var(v) => {
                acc.insert(*v);
            }
            InferType::Con(_) => {}
            InferType::App(_, args) => {
                for arg in args {
                    arg.collect_free_vars(acc);
                }
            }
            InferType::Fun(params, ret, _) => {
                for param in params {
                    param.collect_free_vars(acc);
                }
                ret.collect_free_vars(acc);
            }
            InferType::Tuple(elements) => {
                for element in elements {
                    element.collect_free_vars(acc);
                }
            }
        }
    }

    /// Apply a substitution, replacing any `Var(v)` that appears in `TypeSubst`.
    pub fn apply_type_subst(&self, type_subst: &TypeSubst) -> InferType {
        match self {
            InferType::Var(v) => {
                if let Some(infer_type) = type_subst.get(*v) {
                    // Apply recursively in case the substitution chains.
                    infer_type.apply_type_subst(type_subst)
                } else {
                    InferType::Var(*v)
                }
            }
            InferType::Con(_) => self.clone(),
            InferType::App(con, args) => InferType::App(
                con.clone(),
                args.iter()
                    .map(|a| a.apply_type_subst(type_subst))
                    .collect(),
            ),
            InferType::Fun(params, ret, effects) => InferType::Fun(
                params
                    .iter()
                    .map(|p| p.apply_type_subst(type_subst))
                    .collect(),
                Box::new(ret.apply_type_subst(type_subst)),
                effects.clone(),
            ),
            InferType::Tuple(elements) => InferType::Tuple(
                elements
                    .iter()
                    .map(|e| e.apply_type_subst(type_subst))
                    .collect(),
            ),
        }
    }

    /// Returns `true` if this type is fully concrete (no `Var` nodes).
    pub fn is_concrete(&self) -> bool {
        self.free_vars().is_empty()
    }

    /// `Any` is the gradual escape hatch.
    pub fn is_any(&self) -> bool {
        matches!(self, InferType::Con(TypeConstructor::Any))
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
                if effects.is_empty() {
                    write!(f, ") -> {ret}")
                } else {
                    write!(f, ") -> {ret} with ")?;
                    for (i, effect) in effects.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{effect}")?;
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
    use crate::types::{type_constructor::TypeConstructor, type_subst::TypeSubst};

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
            vec![],
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
            vec![],
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
            vec![],
        );
        assert_eq!(applied, expected);
    }

    #[test]
    fn concrete_and_any_checks() {
        let concrete = InferType::Tuple(vec![int(), InferType::Con(TypeConstructor::Bool)]);
        assert!(concrete.is_concrete());
        assert!(!concrete.is_any());

        let not_concrete = InferType::Fun(vec![infer_var(0)], Box::new(int()), vec![]);
        assert!(!not_concrete.is_concrete());

        let any = InferType::Con(TypeConstructor::Any);
        assert!(any.is_any());
        assert!(any.is_concrete());
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
            vec![],
        );

        assert_eq!(
            infer_type.to_string(),
            "(List<Int>, (Bool, ?3)) -> Map<String, Int>"
        );
    }
}
