//! Dependency analysis and topological sorting for module constants.

use std::collections::{HashMap, HashSet};

use crate::{
    ast::{Visitor, visit},
    syntax::{expression::Expression, symbol::Symbol},
};

/// Find all constant references in an expression.
///
/// Returns a list of constant names that the expression depends on.
pub fn find_constant_refs(
    expression: &Expression,
    known_constants: &HashSet<Symbol>,
) -> Vec<Symbol> {
    let mut collector = ConstRefCollector {
        known_constants,
        refs: HashSet::new(),
    };
    collector.visit_expr(expression);
    collector.refs.into_iter().collect()
}

struct ConstRefCollector<'a> {
    known_constants: &'a HashSet<Symbol>,
    refs: HashSet<Symbol>,
}

impl<'ast> Visitor<'ast> for ConstRefCollector<'_> {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        // walk_expr routes bare Identifier fields (function parameters,
        // MemberAccess.member) through visit_identifier (no-op), not
        // visit_expr, so only expression-position identifiers match here.
        match expr {
            Expression::Identifier { name, .. } if self.known_constants.contains(name) => {
                self.refs.insert(*name);
            }
            _ => {}
        }
        visit::walk_expr(self, expr);
    }
}

/// Topologically sort constants based on their dependencies.
///
/// Returns constants in evaluation order (dependencies first),
/// or an error with the cycle path if circular dependencies are detected.
pub fn topological_sort_constants(
    dependencies: &HashMap<Symbol, Vec<Symbol>>,
) -> Result<Vec<Symbol>, Vec<Symbol>> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    let mut in_progress = HashSet::new();

    let mut names: Vec<&Symbol> = dependencies.keys().collect();
    names.sort_by_key(|s| s.as_u32());
    for name in names {
        visit_constant(
            *name,
            dependencies,
            &mut visited,
            &mut in_progress,
            &mut result,
        )?;
    }

    Ok(result)
}

fn visit_constant(
    name: Symbol,
    deps: &HashMap<Symbol, Vec<Symbol>>,
    visited: &mut HashSet<Symbol>,
    in_progress: &mut HashSet<Symbol>,
    result: &mut Vec<Symbol>,
) -> Result<(), Vec<Symbol>> {
    if visited.contains(&name) {
        return Ok(());
    }

    if in_progress.contains(&name) {
        return Err(vec![name]);
    }

    in_progress.insert(name);

    if let Some(dependencies) = deps.get(&name) {
        for dep in dependencies {
            if let Err(mut cycle) = visit_constant(*dep, deps, visited, in_progress, result) {
                cycle.push(name);
                return Err(cycle);
            }
        }
    }

    in_progress.remove(&name);
    visited.insert(name);
    result.push(name);

    Ok(())
}
