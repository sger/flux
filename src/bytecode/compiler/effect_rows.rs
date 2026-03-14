use std::collections::{HashMap, HashSet, VecDeque};

use crate::syntax::{effect_expr::EffectExpr, symbol::Symbol};

#[derive(Debug, Clone, Default)]
pub(crate) struct EffectRow {
    pub(crate) atoms: HashSet<Symbol>,
    pub(crate) vars: HashSet<Symbol>,
}

impl EffectRow {
    /// Builds a row from an effect list, partitioning concrete atoms and row variables.
    pub(crate) fn from_effect_exprs(effects: &[EffectExpr]) -> Self {
        let mut row = Self::default();
        for effect in effects {
            let piece = Self::from_effect_expr(effect);
            row.atoms.extend(piece.atoms);
            row.vars.extend(piece.vars);
        }
        row
    }

    pub(crate) fn from_effect_expr(effect: &EffectExpr) -> Self {
        match effect {
            EffectExpr::Named { name, .. } => {
                let mut row = Self::default();
                row.atoms.insert(*name);
                row
            }
            EffectExpr::RowVar { name, .. } => {
                // Row variables are always treated as open row bindings.
                let mut row = Self::default();
                row.vars.insert(*name);
                row
            }
            EffectExpr::Add { left, right, .. } => {
                let mut row = Self::from_effect_expr(left);
                let right_row = Self::from_effect_expr(right);
                row.atoms.extend(right_row.atoms);
                row.vars.extend(right_row.vars);
                row
            }
            EffectExpr::Subtract { left, right, .. } => {
                let mut row = Self::from_effect_expr(left);
                let right_row = Self::from_effect_expr(right);
                for atom in right_row.atoms {
                    row.atoms.remove(&atom);
                }
                for var in right_row.vars {
                    row.vars.remove(&var);
                }
                row
            }
        }
    }

    pub(crate) fn concrete_effects(&self, solution: &RowSolution) -> HashSet<Symbol> {
        let mut out = self.atoms.clone();
        for var in &self.vars {
            if let Some(bound) = solution.bindings.get(var) {
                out.extend(bound.iter().copied());
            }
        }
        out
    }

    pub(crate) fn unresolved_vars(&self, solution: &RowSolution) -> HashSet<Symbol> {
        self.vars
            .iter()
            .filter(|var| !solution.constrained_vars.contains(var))
            .copied()
            .collect()
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) enum RowConstraint {
    Eq(EffectRow, EffectRow),
    Contains(EffectRow, Symbol),
    Absent(EffectRow, Symbol),
    // Reserved: not currently emitted by compiler callers; kept for solver completeness.
    Extend {
        out: EffectRow,
        input: EffectRow,
        atom: Symbol,
    },
    // Reserved: not currently emitted by compiler callers; kept for solver completeness.
    Subtract {
        out: EffectRow,
        input: EffectRow,
        atom: Symbol,
    },
    Subset(EffectRow, EffectRow),
}

#[derive(Debug, Clone)]
pub(crate) enum RowConstraintViolation {
    InvalidSubtract { atom: Symbol },
    UnresolvedVars { vars: Vec<Symbol> },
    UnsatisfiedSubset { missing: Vec<Symbol> },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RowSolution {
    pub(crate) bindings: HashMap<Symbol, HashSet<Symbol>>,
    pub(crate) constrained_vars: HashSet<Symbol>,
    pub(crate) violations: Vec<RowConstraintViolation>,
}

#[derive(Debug, Default)]
struct RowSolveState {
    bindings: HashMap<Symbol, HashSet<Symbol>>,
    links: HashMap<Symbol, HashSet<Symbol>>,
    constrained_vars: HashSet<Symbol>,
    violations: Vec<RowConstraintViolation>,
}

impl RowSolveState {
    fn mark_vars_constrained(&mut self, row: &EffectRow) {
        self.constrained_vars.extend(row.vars.iter().copied());
    }

    fn link_vars(&mut self, left: &HashSet<Symbol>, right: &HashSet<Symbol>) {
        for l in left {
            for r in right {
                if l == r {
                    continue;
                }
                self.links.entry(*l).or_default().insert(*r);
                self.links.entry(*r).or_default().insert(*l);
            }
        }
    }

    fn bind_row_atoms(&mut self, row: &EffectRow, atoms: &HashSet<Symbol>) {
        for var in &row.vars {
            self.bindings
                .entry(*var)
                .or_default()
                .extend(atoms.iter().copied());
        }
    }

    fn resolve_links(&mut self) {
        let mut worklist: VecDeque<Symbol> = self.bindings.keys().copied().collect();
        while let Some(var) = worklist.pop_front() {
            let current = self.bindings.get(&var).cloned().unwrap_or_default();
            let linked = self.links.get(&var).cloned().unwrap_or_default();
            for other in linked {
                let entry = self.bindings.entry(other).or_default();
                let before = entry.len();
                entry.extend(current.iter().copied());
                if entry.len() != before {
                    worklist.push_back(other);
                }
            }
        }
    }
}

pub(crate) fn solve_row_constraints(constraints: &[RowConstraint]) -> RowSolution {
    let mut state = RowSolveState::default();
    let mut queue: VecDeque<RowConstraint> = constraints.iter().cloned().collect();
    let mut deferred_absent: Vec<(EffectRow, Symbol)> = Vec::new();

    while let Some(constraint) = queue.pop_front() {
        match constraint {
            RowConstraint::Eq(left, right) => {
                state.mark_vars_constrained(&left);
                state.mark_vars_constrained(&right);
                state.link_vars(&left.vars, &right.vars);

                let mut atoms = left.atoms;
                atoms.extend(right.atoms);
                state.bind_row_atoms(
                    &EffectRow {
                        atoms: HashSet::new(),
                        vars: left.vars,
                    },
                    &atoms,
                );
                state.bind_row_atoms(
                    &EffectRow {
                        atoms: HashSet::new(),
                        vars: right.vars,
                    },
                    &atoms,
                );
            }
            RowConstraint::Contains(row, atom) => {
                state.mark_vars_constrained(&row);
                if row.atoms.contains(&atom) {
                    continue;
                }
                if row.vars.is_empty() {
                    state
                        .violations
                        .push(RowConstraintViolation::UnsatisfiedSubset {
                            missing: vec![atom],
                        });
                    continue;
                }
                for var in row.vars {
                    state.bindings.entry(var).or_default().insert(atom);
                }
            }
            RowConstraint::Absent(row, atom) => {
                // Evaluate `Absent` after row bindings stabilize; queue-order checks can miss
                // conflicts when later argument constraints bind shared effect variables.
                deferred_absent.push((row, atom));
            }
            RowConstraint::Extend { out, input, atom } => {
                let mut extended = input.clone();
                extended.atoms.insert(atom);
                queue.push_back(RowConstraint::Eq(out, extended));
            }
            RowConstraint::Subtract { out, input, atom } => {
                if !input.atoms.contains(&atom) && input.vars.is_empty() {
                    state
                        .violations
                        .push(RowConstraintViolation::InvalidSubtract { atom });
                    continue;
                }
                let mut reduced = input.clone();
                reduced.atoms.remove(&atom);
                queue.push_back(RowConstraint::Eq(out, reduced));
            }
            RowConstraint::Subset(left, right) => {
                state.mark_vars_constrained(&left);
                state.mark_vars_constrained(&right);
                let missing: Vec<Symbol> = left
                    .atoms
                    .iter()
                    .copied()
                    .filter(|atom| !right.atoms.contains(atom))
                    .collect();
                if missing.is_empty() {
                    continue;
                }
                if right.vars.is_empty() {
                    state
                        .violations
                        .push(RowConstraintViolation::UnsatisfiedSubset { missing });
                } else {
                    for var in right.vars {
                        state
                            .bindings
                            .entry(var)
                            .or_default()
                            .extend(missing.iter().copied());
                    }
                }
            }
        }
    }

    state.resolve_links();
    apply_absent_constraints(&mut state, &deferred_absent);

    // Deduplicate violations: sort by (discriminant, first payload key) then
    // collapse adjacent entries with the same discriminant so callers always
    // receive a clean list without repeated identical violation kinds.
    state.violations.sort_by_key(|v| match v {
        RowConstraintViolation::InvalidSubtract { atom } => (0u8, atom.as_u32()),
        RowConstraintViolation::UnresolvedVars { vars } => {
            (1u8, vars.first().map_or(0, |s| s.as_u32()))
        }
        RowConstraintViolation::UnsatisfiedSubset { missing } => {
            (2u8, missing.first().map_or(0, |s| s.as_u32()))
        }
    });
    state
        .violations
        .dedup_by(|a, b| std::mem::discriminant(a) == std::mem::discriminant(b));

    RowSolution {
        bindings: state.bindings,
        constrained_vars: state.constrained_vars,
        violations: state.violations,
    }
}

fn apply_absent_constraints(state: &mut RowSolveState, absent: &[(EffectRow, Symbol)]) {
    let mut unresolved_vars: HashSet<Symbol> = HashSet::new();

    for (row, atom) in absent {
        if row.atoms.contains(atom) {
            state
                .violations
                .push(RowConstraintViolation::InvalidSubtract { atom: *atom });
            continue;
        }

        let found_bound = row.vars.iter().any(|var| {
            state
                .bindings
                .get(var)
                .is_some_and(|bound| bound.contains(atom))
        });

        if found_bound {
            state
                .violations
                .push(RowConstraintViolation::InvalidSubtract { atom: *atom });
            continue;
        }

        // Absence is proven if at least one var is bound (and its bindings don't contain
        // the atom — already checked above). Only flag unresolved when *all* vars lack
        // bindings, meaning we cannot confirm or deny the atom's presence.
        let all_unbound = row.vars.iter().all(|var| !state.bindings.contains_key(var));
        if all_unbound {
            for var in &row.vars {
                unresolved_vars.insert(*var);
            }
        }
    }

    if !unresolved_vars.is_empty() {
        let mut vars: Vec<Symbol> = unresolved_vars.into_iter().collect();
        vars.sort_by_key(|symbol| symbol.as_u32());
        state
            .violations
            .push(RowConstraintViolation::UnresolvedVars { vars });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(n: u32) -> Symbol {
        Symbol::new(n)
    }

    fn row(atoms: &[u32], vars: &[u32]) -> EffectRow {
        EffectRow {
            atoms: atoms.iter().copied().map(sym).collect(),
            vars: vars.iter().copied().map(sym).collect(),
        }
    }

    #[test]
    fn eq_binds_atoms_to_both_var_sets() {
        let constraints = vec![RowConstraint::Eq(row(&[10], &[1]), row(&[20], &[2]))];
        let sol = solve_row_constraints(&constraints);
        assert!(sol.bindings[&sym(1)].contains(&sym(10)));
        assert!(sol.bindings[&sym(1)].contains(&sym(20)));
        assert!(sol.bindings[&sym(2)].contains(&sym(10)));
        assert!(sol.bindings[&sym(2)].contains(&sym(20)));
    }

    #[test]
    fn contains_emits_violation_for_closed_row_missing_atom() {
        let constraints = vec![RowConstraint::Contains(row(&[10], &[]), sym(20))];
        let sol = solve_row_constraints(&constraints);
        assert_eq!(sol.violations.len(), 1);
        assert!(matches!(
            sol.violations[0],
            RowConstraintViolation::UnsatisfiedSubset { .. }
        ));
    }

    #[test]
    fn absent_deferred_fires_after_resolve_links() {
        // var 1 is bound to atom 10 by Eq; Absent(row with var 1, 10) should fail.
        let constraints = vec![
            RowConstraint::Eq(row(&[10], &[1]), row(&[], &[])),
            RowConstraint::Absent(row(&[], &[1]), sym(10)),
        ];
        let sol = solve_row_constraints(&constraints);
        assert!(!sol.violations.is_empty());
    }

    #[test]
    fn subset_emits_violation_for_missing_atoms() {
        let constraints = vec![RowConstraint::Subset(row(&[10, 20], &[]), row(&[10], &[]))];
        let sol = solve_row_constraints(&constraints);
        assert_eq!(sol.violations.len(), 1);
        assert!(matches!(
            sol.violations[0],
            RowConstraintViolation::UnsatisfiedSubset { ref missing } if missing.contains(&sym(20))
        ));
    }

    #[test]
    fn resolve_links_propagates_transitively() {
        // var 1 linked to var 2 via Eq; var 2 bound to atom 30 via Contains.
        let constraints = vec![
            RowConstraint::Eq(row(&[], &[1]), row(&[], &[2])),
            RowConstraint::Contains(row(&[], &[2]), sym(30)),
        ];
        let sol = solve_row_constraints(&constraints);
        assert!(
            sol.bindings
                .get(&sym(1))
                .is_some_and(|b| b.contains(&sym(30)))
        );
    }

    #[test]
    fn violations_are_deduplicated() {
        // Three identical Subset failures for the same missing atom.
        let constraints = vec![
            RowConstraint::Subset(row(&[10], &[]), row(&[], &[])),
            RowConstraint::Subset(row(&[10], &[]), row(&[], &[])),
            RowConstraint::Subset(row(&[10], &[]), row(&[], &[])),
        ];
        let sol = solve_row_constraints(&constraints);
        assert_eq!(sol.violations.len(), 1);
    }
}
