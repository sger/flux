use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    syntax::{Identifier, effect_expr::EffectExpr},
    types::{TypeVarId, type_subst::TypeSubst},
};

/// Error returned when `InferEffectRow::from_effect_exprs` encounters more than
/// one distinct row variable in a single effect expression list.
///
/// The parser prevents this for well-formed surface syntax, so `Err` is
/// unreachable in practice — the guard makes the invariant explicit and testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipleRowVarError {
    pub first: Identifier,
    pub second: Identifier,
}

/// Effect row used during type inference.
///
/// An effect row is represented as:
/// - a concrete set of effect names, and
/// - an optional tail row variable for open rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferEffectRow {
    concrete: HashSet<Identifier>,
    tail: Option<TypeVarId>,
}

impl InferEffectRow {
    /// Create a closed row with no concrete effects.
    pub fn closed_empty() -> Self {
        Self {
            concrete: HashSet::new(),
            tail: None,
        }
    }

    /// Create a closed row from concrete effect names.
    pub fn closed_from_symbols<I>(symbols: I) -> Self
    where
        I: IntoIterator<Item = Identifier>,
    {
        Self {
            concrete: symbols.into_iter().collect(),
            tail: None,
        }
    }

    /// Create an open row from concrete effects plus a tail variable.
    pub fn open_from_symbols<I>(symbols: I, tail: TypeVarId) -> Self
    where
        I: IntoIterator<Item = Identifier>,
    {
        Self {
            concrete: symbols.into_iter().collect(),
            tail: Some(tail),
        }
    }

    /// Return the concrete effect names in this row.
    pub fn concrete(&self) -> &HashSet<Identifier> {
        &self.concrete
    }

    /// Return mutable access to the concrete effect names.
    pub fn concrete_mut(&mut self) -> &mut HashSet<Identifier> {
        &mut self.concrete
    }

    /// Return the optional tail row variable.
    pub fn tail(&self) -> Option<TypeVarId> {
        self.tail
    }

    /// Return row variables that occur free in this row.
    ///
    /// With the current representation this is either empty (closed row)
    /// or a singleton containing the tail variable (open row).
    pub fn free_row_vars(&self) -> HashSet<TypeVarId> {
        let mut vars = HashSet::new();
        if let Some(tail) = self.tail {
            vars.insert(tail);
        }
        vars
    }

    /// Collect all `Symbol`s (effect names) in this row.
    pub fn collect_symbols(&self, out: &mut HashSet<Identifier>) {
        out.extend(self.concrete.iter().copied());
    }

    /// Replace Symbol IDs according to `remap`. Returns a new row.
    pub fn remap_symbols(&self, remap: &std::collections::HashMap<Identifier, Identifier>) -> Self {
        let concrete = self
            .concrete
            .iter()
            .map(|sym| *remap.get(sym).unwrap_or(sym))
            .collect();
        Self {
            concrete,
            tail: self.tail,
        }
    }

    /// Apply row-variable substitution to this row.
    ///
    /// Follows row-tail bindings transitively, unions concrete effects along the
    /// chain, and stops on:
    /// - an unbound tail variable,
    /// - a closed bound row, or
    /// - a cycle in row-variable bindings.
    pub fn apply_row_subst(&self, type_subst: &TypeSubst) -> Self {
        let mut concrete = self.concrete.clone();
        let mut tail = self.tail;
        let mut seen = HashSet::new();

        loop {
            // No tail means the row is closed; substitution is complete.
            let Some(current_tail) = tail else {
                break;
            };
            // Break on cycles in row-variable bindings. This guards against
            // pathological substitution inputs and ensures termination.
            if !seen.insert(current_tail) {
                break;
            }
            // If the current tail is not bound, we keep it as the remaining open tail.
            let Some(bound_row) = type_subst.get_row(current_tail) else {
                break;
            };

            // Merge concrete effects from the bound row and continue walking
            // through its tail. Using a set keeps duplicates naturally deduped.
            concrete.extend(bound_row.concrete.iter().copied());
            tail = bound_row.tail;
        }

        Self { concrete, tail }
    }

    /// Build an inferred row from parsed effect expressions.
    ///
    /// Concrete effect names are normalized and accumulated from all entries.
    /// If any row-variable expression appears, the resulting row is open with
    /// that row-variable as the tail. Returns `Err` when more than one *distinct*
    /// row variable is encountered — the parser prevents this for well-formed
    /// surface syntax, so `Err` is unreachable in practice.
    pub fn from_effect_exprs(
        effects: &[EffectExpr],
        row_var_env: &mut HashMap<Identifier, TypeVarId>,
        row_var_counter: &mut u32,
    ) -> Result<Self, MultipleRowVarError> {
        let mut concrete = HashSet::new();
        let mut tail: Option<TypeVarId> = None;
        let mut tail_name: Option<Identifier> = None;

        for effect in effects {
            // Concrete names are normalized at parse/syntax layer before insertion.
            concrete.extend(effect.normalized_concrete_names());
            if let Some(row_var) = effect.row_var() {
                if let Some(existing_name) = tail_name
                    && existing_name != row_var
                {
                    return Err(MultipleRowVarError {
                        first: existing_name,
                        second: row_var,
                    });
                }
                tail_name = Some(row_var);
                // Row variables are interned through `row_var_env` to keep a stable
                // TypeVarId per symbolic row variable in this inference context.
                let mapped = *row_var_env.entry(row_var).or_insert_with(|| {
                    let next = *row_var_counter;
                    *row_var_counter += 1;
                    next
                });
                tail = Some(mapped);
            }
        }

        Ok(match tail {
            Some(row_var) => Self::open_from_symbols(concrete, row_var),
            None => Self::closed_from_symbols(concrete),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{InferEffectRow, MultipleRowVarError};
    use crate::{
        syntax::{effect_expr::EffectExpr, symbol::Symbol},
        types::type_subst::TypeSubst,
    };

    fn sym(i: u32) -> Symbol {
        Symbol::new(i)
    }

    #[test]
    fn apply_row_subst_follows_tail_chain_and_unions_concrete_effects() {
        let mut subst = TypeSubst::empty();
        subst.insert_row(1, InferEffectRow::open_from_symbols([sym(20)], 2));
        subst.insert_row(2, InferEffectRow::closed_from_symbols([sym(30)]));

        let row = InferEffectRow::open_from_symbols([sym(10)], 1);
        let applied = row.apply_row_subst(&subst);

        assert!(applied.concrete().contains(&sym(10)));
        assert!(applied.concrete().contains(&sym(20)));
        assert!(applied.concrete().contains(&sym(30)));
        assert_eq!(applied.tail(), None);
    }

    fn named(sym: Symbol) -> EffectExpr {
        EffectExpr::Named {
            name: sym,
            span: Default::default(),
        }
    }

    fn row_var(sym: Symbol) -> EffectExpr {
        EffectExpr::RowVar {
            name: sym,
            span: Default::default(),
        }
    }

    #[test]
    fn from_effect_exprs_closed_row_collects_concrete_names() {
        let effects = vec![named(sym(10)), named(sym(20))];
        let mut env = HashMap::new();
        let mut counter = 0u32;
        let row = InferEffectRow::from_effect_exprs(&effects, &mut env, &mut counter).unwrap();
        assert!(row.concrete().contains(&sym(10)));
        assert!(row.concrete().contains(&sym(20)));
        assert_eq!(row.tail(), None);
    }

    #[test]
    fn from_effect_exprs_open_row_sets_tail() {
        let effects = vec![named(sym(10)), row_var(sym(99))];
        let mut env = HashMap::new();
        let mut counter = 5u32;
        let row = InferEffectRow::from_effect_exprs(&effects, &mut env, &mut counter).unwrap();
        assert!(row.concrete().contains(&sym(10)));
        assert_eq!(row.tail(), Some(5));
        assert_eq!(counter, 6);
    }

    #[test]
    fn from_effect_exprs_same_var_name_reuses_id() {
        let effects = vec![row_var(sym(99)), row_var(sym(99))];
        let mut env = HashMap::new();
        let mut counter = 0u32;
        let row = InferEffectRow::from_effect_exprs(&effects, &mut env, &mut counter).unwrap();
        assert_eq!(row.tail(), Some(0));
        assert_eq!(counter, 1); // only one fresh id allocated
    }

    #[test]
    fn from_effect_exprs_two_distinct_row_vars_returns_err() {
        let effects = vec![row_var(sym(1)), row_var(sym(2))];
        let mut env = HashMap::new();
        let mut counter = 0u32;
        let result = InferEffectRow::from_effect_exprs(&effects, &mut env, &mut counter);
        assert_eq!(
            result,
            Err(MultipleRowVarError {
                first: sym(1),
                second: sym(2),
            })
        );
    }

    #[test]
    fn apply_row_subst_stops_on_cycle() {
        let mut subst = TypeSubst::empty();
        subst.insert_row(1, InferEffectRow::open_from_symbols([sym(20)], 2));
        subst.insert_row(2, InferEffectRow::open_from_symbols([sym(30)], 1));

        let row = InferEffectRow::open_from_symbols([sym(10)], 1);
        let applied = row.apply_row_subst(&subst);

        assert!(applied.concrete().contains(&sym(10)));
        assert!(applied.concrete().contains(&sym(20)));
        assert!(applied.concrete().contains(&sym(30)));
        assert_eq!(applied.tail(), Some(1));
    }

    #[test]
    fn collect_symbols_gathers_concrete_effects() {
        let row = InferEffectRow::closed_from_symbols([sym(10), sym(20), sym(30)]);
        let mut out = std::collections::HashSet::new();
        row.collect_symbols(&mut out);
        assert_eq!(out.len(), 3);
        assert!(out.contains(&sym(10)));
        assert!(out.contains(&sym(20)));
        assert!(out.contains(&sym(30)));
    }

    #[test]
    fn collect_symbols_empty_for_empty_row() {
        let row = InferEffectRow::closed_empty();
        let mut out = std::collections::HashSet::new();
        row.collect_symbols(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn remap_symbols_rewrites_concrete_effects() {
        let row = InferEffectRow::closed_from_symbols([sym(5), sym(10)]);
        let remap = HashMap::from([(sym(5), sym(50)), (sym(10), sym(100))]);
        let remapped = row.remap_symbols(&remap);
        assert!(remapped.concrete().contains(&sym(50)));
        assert!(remapped.concrete().contains(&sym(100)));
        assert!(!remapped.concrete().contains(&sym(5)));
        assert!(!remapped.concrete().contains(&sym(10)));
    }

    #[test]
    fn remap_symbols_preserves_tail_variable() {
        let row = InferEffectRow::open_from_symbols([sym(5)], 42);
        let remap = HashMap::from([(sym(5), sym(50))]);
        let remapped = row.remap_symbols(&remap);
        assert!(remapped.concrete().contains(&sym(50)));
        assert_eq!(remapped.tail(), Some(42));
    }

    #[test]
    fn remap_symbols_preserves_unmapped_effects() {
        let row = InferEffectRow::closed_from_symbols([sym(5), sym(10)]);
        let remap = HashMap::from([(sym(5), sym(50))]);
        let remapped = row.remap_symbols(&remap);
        assert!(remapped.concrete().contains(&sym(50)));
        assert!(remapped.concrete().contains(&sym(10)));
    }
}
