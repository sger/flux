use std::collections::{HashMap, HashSet};

use crate::{
    syntax::{Identifier, effect_expr::EffectExpr},
    types::{TypeVarId, type_subst::TypeSubst},
};

/// Effect row used during type inference.
///
/// An effect row is represented as:
/// - a concrete set of effect names, and
/// - an optional tail row variable for open rows.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// the last row-variable tail encountered.
    pub fn from_effect_exprs(
        effects: &[EffectExpr],
        row_var_env: &mut HashMap<Identifier, TypeVarId>,
        fresh: &mut u32,
    ) -> Self {
        let mut concrete = HashSet::new();
        let mut tail = None;
        for effect in effects {
            // Concrete names are normalized at parse/syntax layer before insertion.
            concrete.extend(effect.normalized_concrete_names());
            if let Some(row_var) = effect.row_var() {
                // Row variables are interned through `row_var_env` to keep a stable
                // TypeVarId per symbolic row variable in this inference context.
                let mapped = *row_var_env.entry(row_var).or_insert_with(|| {
                    let next = *fresh;
                    *fresh += 1;
                    next
                });
                // Current behavior keeps the last seen row-var as the tail.
                // Multiple row-vars in one list are not merged here; they are
                // represented by this single open tail plus concrete effects.
                tail = Some(mapped);
            }
        }

        match tail {
            Some(row_var) => Self::open_from_symbols(concrete, row_var),
            None => Self::closed_from_symbols(concrete),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InferEffectRow;
    use crate::{syntax::symbol::Symbol, types::type_subst::TypeSubst};

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
}
