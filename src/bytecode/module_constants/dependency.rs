//! Dependency analysis and topological sorting for module constants.

use std::collections::{HashMap, HashSet};

use crate::frontend::expression::Expression;

/// Find all constant references in an expression.
///
/// Returns a list of constant names that the expression depends on.
pub fn find_constant_refs(
    expression: &Expression,
    known_constants: &HashSet<String>,
) -> Vec<String> {
    let mut refs = HashSet::new();
    collect_constant_refs(expression, known_constants, &mut refs);
    refs.into_iter().collect()
}

fn collect_constant_refs(
    expr: &Expression,
    known_constants: &HashSet<String>,
    refs: &mut HashSet<String>,
) {
    match expr {
        Expression::Identifier { name, .. } => {
            if known_constants.contains(name) {
                refs.insert(name.clone());
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
    dependencies: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, Vec<String>> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    let mut in_progress = HashSet::new();

    let mut names: Vec<&String> = dependencies.keys().collect();
    names.sort();
    for name in names {
        visit_constant(
            name,
            dependencies,
            &mut visited,
            &mut in_progress,
            &mut result,
        )?;
    }

    Ok(result)
}

fn visit_constant(
    name: &str,
    deps: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    in_progress: &mut HashSet<String>,
    result: &mut Vec<String>,
) -> Result<(), Vec<String>> {
    if visited.contains(name) {
        return Ok(());
    }

    if in_progress.contains(name) {
        return Err(vec![name.to_string()]);
    }

    in_progress.insert(name.to_string());

    if let Some(dependencies) = deps.get(name) {
        for dep in dependencies {
            if let Err(mut cycle) = visit_constant(dep, deps, visited, in_progress, result) {
                cycle.push(name.to_string());
                return Err(cycle);
            }
        }
    }

    in_progress.remove(name);
    visited.insert(name.to_string());
    result.push(name.to_string());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topological_sort_simple() {
        let mut deps = HashMap::new();
        deps.insert("A".to_string(), vec![]);
        deps.insert("B".to_string(), vec!["A".to_string()]);
        deps.insert("C".to_string(), vec!["B".to_string()]);

        let result = topological_sort_constants(&deps).unwrap();
        assert_eq!(result, vec!["A", "B", "C"]);
    }

    #[test]
    fn test_topological_sort_independent() {
        let mut deps = HashMap::new();
        deps.insert("A".to_string(), vec![]);
        deps.insert("B".to_string(), vec![]);

        let result = topological_sort_constants(&deps).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"A".to_string()));
        assert!(result.contains(&"B".to_string()));
    }

    #[test]
    fn test_topological_sort_cycle() {
        let mut deps = HashMap::new();
        deps.insert("A".to_string(), vec!["B".to_string()]);
        deps.insert("B".to_string(), vec!["A".to_string()]);

        let result = topological_sort_constants(&deps);
        assert!(result.is_err());
        let cycle = result.unwrap_err();
        assert!(cycle.contains(&"A".to_string()));
        assert!(cycle.contains(&"B".to_string()));
    }
}
