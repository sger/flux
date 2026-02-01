use std::collections::{HashMap, HashSet};

use crate::{
    frontend::expression::Expression,
    runtime::{hash_key::HashKey, object::Object},
};

/// Find all constant references in an expression
///
/// # Arguments
/// * `expr` - The expression to analyze
/// * `known_constants` - Set of names that are module constants
///
/// # Returns
/// A vector of constant names that this expression depends on
pub fn find_constant_refs(
    expression: &Expression,
    known_constants: &HashSet<String>,
) -> Vec<String> {
    let mut refs = Vec::new();
    collect_constant_refs(expression, known_constants, &mut refs);
    refs
}

fn collect_constant_refs(
    expr: &Expression,
    known_constants: &HashSet<String>,
    refs: &mut Vec<String>,
) {
    match expr {
        Expression::Identifier { name, .. } => {
            if known_constants.contains(name) && !refs.contains(name) {
                refs.push(name.clone());
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
        // Note: No Expression::Grouped - parser unwraps parentheses transparently
        // Literals and other expressions don't reference constants
        _ => {}
    }
}

/// Topologically sort constants based on their dependencies
///
/// # Arguments
/// * `dependencies` - Map from constant name to list of constants it depends on
///
/// # Returns
/// * `Ok(Vec<String>)` - Constants in evaluation order (dependencies first)
/// * `Err(Vec<String>)` - The cycle path if a circular dependency is detected
pub fn topological_sort_constants(
    dependencies: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, Vec<String>> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    let mut in_progress = HashSet::new();

    for name in dependencies.keys() {
        if let Err(cycle) = visit_constant(
            name,
            dependencies,
            &mut visited,
            &mut in_progress,
            &mut result,
        ) {
            return Err(cycle);
        }
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
        // Found a cycle return the starting point
        return Err(vec![name.to_string()]);
    }

    in_progress.insert(name.to_string());

    if let Some(dependencies) = deps.get(name) {
        for dependency in dependencies {
            if let Err(mut cycle) = visit_constant(dependency, deps, visited, in_progress, result) {
                // Build the cycle path
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

/// Error type for constant evaluation failures
#[derive(Debug, Clone)]
pub struct ConstEvalError {
    pub code: &'static str,
    pub message: String,
    pub hint: Option<String>,
}

impl ConstEvalError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Evaluate a constant expression
///
/// # Arguments
/// * `expr` - The expression to evaluate
/// * `defined` - Map of already-evaluated constants (for references)
///
/// # Returns
/// The evaluated Object value, or an error
pub fn eval_const_expr(
    expr: &Expression,
    defined: &HashMap<String, Object>,
) -> Result<Object, ConstEvalError> {
    match expr {
        // Literals
        Expression::Integer { value, .. } => Ok(Object::Integer(*value)),
        Expression::Float { value, .. } => Ok(Object::Float(*value)),
        Expression::String { value, .. } => Ok(Object::String(value.clone())),
        Expression::Boolean { value, .. } => Ok(Object::Boolean(*value)),
        Expression::None { .. } => Ok(Object::None),
        Expression::Some { value, .. } => {
            let inner_expression = eval_const_expr(value, defined)?;
            Ok(Object::Some(Box::new(inner_expression)))
        }

        // Arrays of constants
        Expression::Array { elements, .. } => {
            let mut values = Vec::new();

            for element in elements {
                values.push(eval_const_expr(element, defined)?);
            }

            Ok(Object::Array(values))
        }

        // Hashes of constants
        Expression::Hash { pairs, .. } => {
            let mut map = HashMap::new();

            for (key, value) in pairs {
                let k = eval_const_expr(key, defined)?;
                let v = eval_const_expr(value, defined)?;

                let hash_key = match &k {
                    Object::Integer(i) => HashKey::Integer(*i),
                    Object::Boolean(b) => HashKey::Boolean(*b),
                    Object::String(s) => HashKey::String(s.clone()),
                    _ => {
                        return Err(ConstEvalError::new(
                            "E040",
                            "Hash keys must be integers, booleans, or strings.",
                        ));
                    }
                };
                map.insert(hash_key, v);
            }
            Ok(Object::Hash(map))
        }

        // Reference to another constant
        Expression::Identifier { name, .. } => defined.get(name).cloned().ok_or_else(|| {
            ConstEvalError::new("E041", format!("'{}' is not a module constant.", name))
        }),

        // Unary operations
        Expression::Prefix {
            operator, right, ..
        } => {
            let r = eval_const_expr(right, defined)?;
            eval_const_unary_op(operator, &r)
        }

        // Binary operations
        Expression::Infix {
            left,
            operator,
            right,
            ..
        } => {
            let l = eval_const_expr(left, defined)?;
            let r = eval_const_expr(right, defined)?;
            eval_const_binary_op(&l, operator, &r)
        }

        // Anything else is not a constant expression
        _ => Err(ConstEvalError::new(
            "E042",
            "Only literals, basic operations, and references to module constants are allowed.",
        )
        .with_hint("Module constants must be evaluable at compile time.")),
    }
}

fn eval_const_unary_op(op: &str, right: &Object) -> Result<Object, ConstEvalError> {
    match (op, right) {
        ("-", Object::Integer(i)) => Ok(Object::Integer(-i)),
        ("-", Object::Float(f)) => Ok(Object::Float(-f)),
        ("!", Object::Boolean(b)) => Ok(Object::Boolean(!b)),
        _ => Err(ConstEvalError::new(
            "E043",
            format!("Cannot apply '{}' to {:?} at compile time.", op, right),
        )),
    }
}

fn eval_const_binary_op(left: &Object, op: &str, right: &Object) -> Result<Object, ConstEvalError> {
    match (left, op, right) {
        // Integer operations
        (Object::Integer(a), "+", Object::Integer(b)) => Ok(Object::Integer(a + b)),
        (Object::Integer(a), "-", Object::Integer(b)) => Ok(Object::Integer(a - b)),
        (Object::Integer(a), "*", Object::Integer(b)) => Ok(Object::Integer(a * b)),
        (Object::Integer(a), "/", Object::Integer(b)) => Ok(Object::Integer(a / b)),
        (Object::Integer(a), "%", Object::Integer(b)) => Ok(Object::Integer(a % b)),

        // Float operations
        (Object::Float(a), "+", Object::Float(b)) => Ok(Object::Float(a + b)),
        (Object::Float(a), "-", Object::Float(b)) => Ok(Object::Float(a - b)),
        (Object::Float(a), "*", Object::Float(b)) => Ok(Object::Float(a * b)),
        (Object::Float(a), "/", Object::Float(b)) => Ok(Object::Float(a / b)),

        // Mixed numeric promote to float
        (Object::Integer(i), op, Object::Float(_)) => {
            eval_const_binary_op(&Object::Float(*i as f64), op, right)
        }
        (Object::Float(_), op, Object::Integer(f)) => {
            eval_const_binary_op(left, op, &Object::Float(*f as f64))
        }

        // String concatenation
        (Object::String(a), "+", Object::String(b)) => Ok(Object::String(format!("{}{}", a, b))),

        // Boolean operations
        (Object::Boolean(a), "&&", Object::Boolean(b)) => Ok(Object::Boolean(*a && *b)),
        (Object::Boolean(a), "||", Object::Boolean(b)) => Ok(Object::Boolean(*a || *b)),

        // Integer comparisons
        (Object::Integer(a), "==", Object::Integer(b)) => Ok(Object::Boolean(a == b)),
        (Object::Integer(a), "!=", Object::Integer(b)) => Ok(Object::Boolean(a != b)),
        (Object::Integer(a), "<", Object::Integer(b)) => Ok(Object::Boolean(a < b)),
        (Object::Integer(a), ">", Object::Integer(b)) => Ok(Object::Boolean(a > b)),
        (Object::Integer(a), "<=", Object::Integer(b)) => Ok(Object::Boolean(a <= b)),
        (Object::Integer(a), ">=", Object::Integer(b)) => Ok(Object::Boolean(a >= b)),

        // Float comparisons
        (Object::Float(a), "==", Object::Float(b)) => Ok(Object::Boolean(a == b)),
        (Object::Float(a), "!=", Object::Float(b)) => Ok(Object::Boolean(a != b)),
        (Object::Float(a), "<", Object::Float(b)) => Ok(Object::Boolean(a < b)),
        (Object::Float(a), ">", Object::Float(b)) => Ok(Object::Boolean(a > b)),
        (Object::Float(a), "<=", Object::Float(b)) => Ok(Object::Boolean(a <= b)),
        (Object::Float(a), ">=", Object::Float(b)) => Ok(Object::Boolean(a >= b)),

        // String comparisons
        (Object::String(a), "==", Object::String(b)) => Ok(Object::Boolean(a == b)),
        (Object::String(a), "!=", Object::String(b)) => Ok(Object::Boolean(a != b)),

        // Boolean comparisons
        (Object::Boolean(a), "==", Object::Boolean(b)) => Ok(Object::Boolean(a == b)),
        (Object::Boolean(a), "!=", Object::Boolean(b)) => Ok(Object::Boolean(a != b)),

        _ => Err(ConstEvalError::new(
            "E044",
            format!(
                "Cannot apply '{}' to {:?} and {:?} at compile time.",
                op, left, right
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use crate::bytecode::const_eval::topological_sort_constants;
    use std::collections::HashMap;

    #[test]
    fn test_topological_sort_simple() {
        let mut deps = HashMap::new();
        deps.insert("a".to_string(), vec![]);
        deps.insert("b".to_string(), vec!["a".to_string()]);
        deps.insert("c".to_string(), vec!["b".to_string()]);

        let result = topological_sort_constants(&deps).unwrap();
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_topological_sort_cycle() {
        let mut deps = HashMap::new();
        deps.insert("a".to_string(), vec!["b".to_string()]);
        deps.insert("b".to_string(), vec!["a".to_string()]);

        let result = topological_sort_constants(&deps);
        assert!(result.is_err());
    }
}
