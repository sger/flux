//! Dependency analysis and topological sorting for module constants.

use std::collections::{HashMap, HashSet};

use crate::syntax::{expression::Expression, symbol::Symbol};

/// Find all constant references in an expression.
///
/// Returns a list of constant names that the expression depends on.
pub fn find_constant_refs(
    expression: &Expression,
    known_constants: &HashSet<Symbol>,
) -> Vec<Symbol> {
    let mut refs = HashSet::new();
    collect_constant_refs(expression, known_constants, &mut refs);
    refs.into_iter().collect()
}

fn collect_constant_refs(
    expr: &Expression,
    known_constants: &HashSet<Symbol>,
    refs: &mut HashSet<Symbol>,
) {
    match expr {
        Expression::Identifier { name, .. } => {
            if known_constants.contains(name) {
                refs.insert(*name);
            }
        }
        Expression::Infix { left, right, .. } => {
            collect_constant_refs(left, known_constants, refs);
            collect_constant_refs(right, known_constants, refs);
        }
        Expression::Prefix { right, .. } => {
            collect_constant_refs(right, known_constants, refs);
        }
        Expression::Array { elements, .. } => {
            for elem in elements {
                collect_constant_refs(elem, known_constants, refs);
            }
        }
        Expression::Hash { pairs, .. } => {
            for (key, value) in pairs {
                collect_constant_refs(key, known_constants, refs);
                collect_constant_refs(value, known_constants, refs);
            }
        }
        Expression::Some { value, .. } => {
            collect_constant_refs(value, known_constants, refs);
        }
        _ => {}
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
