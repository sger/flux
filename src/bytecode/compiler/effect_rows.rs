use std::collections::{HashMap, HashSet, VecDeque};

use crate::syntax::{effect_expr::EffectExpr, symbol::Symbol};

#[derive(Debug, Clone, Default)]
pub(crate) struct EffectRow {
    pub(crate) atoms: HashSet<Symbol>,
    pub(crate) vars: HashSet<Symbol>,
}

impl EffectRow {
    pub(crate) fn from_effect_exprs<F>(effects: &[EffectExpr], is_var: F) -> Self
    where
        F: Fn(Symbol) -> bool,
    {
        let mut row = Self::default();
        for effect in effects {
            let piece = Self::from_effect_expr(effect, &is_var);
            row.atoms.extend(piece.atoms);
            row.vars.extend(piece.vars);
        }
        row
    }

    pub(crate) fn from_effect_expr<F>(effect: &EffectExpr, is_var: &F) -> Self
    where
        F: Fn(Symbol) -> bool,
    {
        match effect {
            EffectExpr::Named { name, .. } => {
                let mut row = Self::default();
                if is_var(*name) {
                    row.vars.insert(*name);
                } else {
                    row.atoms.insert(*name);
                }
                row
            }
            EffectExpr::Add { left, right, .. } => {
                let mut row = Self::from_effect_expr(left, is_var);
                let right_row = Self::from_effect_expr(right, is_var);
                row.atoms.extend(right_row.atoms);
                row.vars.extend(right_row.vars);
                row
            }
            EffectExpr::Subtract { left, right, .. } => {
                let mut row = Self::from_effect_expr(left, is_var);
                let right_row = Self::from_effect_expr(right, is_var);
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
    Extend {
        out: EffectRow,
        input: EffectRow,
        atom: Symbol,
    },
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
                if row.atoms.contains(&atom) || row.vars.is_empty() {
                    continue;
                }
                for var in row.vars {
                    state.bindings.entry(var).or_default().insert(atom);
                }
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

    RowSolution {
        bindings: state.bindings,
        constrained_vars: state.constrained_vars,
        violations: state.violations,
    }
}
