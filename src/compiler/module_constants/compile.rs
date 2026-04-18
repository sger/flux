//! Compilation and evaluation of module constants.

use std::collections::HashMap;

use crate::{
    diagnostics::position::Position,
    runtime::value::Value,
    syntax::{block::Block, interner::Interner, symbol::Symbol},
};

use super::{analysis::analyze_module_constants, error::ConstEvalError, eval::eval_const_expr};

/// Error that can occur during module constants compilation.
#[derive(Debug)]
pub enum ConstCompileError {
    /// Circular dependency detected among constants.
    CircularDependency(Vec<String>),
    /// Error evaluating a constant expression.
    EvalError {
        const_name: String,
        position: Position,
        error: ConstEvalError,
    },
}

/// Compile module constants by analyzing dependencies and evaluating in order.
///
/// Returns a map of qualified constant symbols (e.g., `Module.CONST`) to their values.
///
/// # Errors
///
/// Returns `ConstCompileError::CircularDependency` if constants have circular dependencies.
/// Returns `ConstCompileError::EvalError` if a constant expression cannot be evaluated.
pub fn compile_module_constants(
    body: &Block,
    module_name: Symbol,
    interner: &mut Interner,
) -> Result<HashMap<Symbol, Value>, ConstCompileError> {
    // Step 1: Analyze constants and resolve dependencies
    let analysis = analyze_module_constants(body).map_err(|cycle| {
        ConstCompileError::CircularDependency(
            cycle
                .iter()
                .map(|s| interner.resolve(*s).to_string())
                .collect(),
        )
    })?;

    // Step 2: Evaluate constants in dependency order
    let mut local_constants: HashMap<Symbol, Value> = HashMap::new();
    let mut module_constants: HashMap<Symbol, Value> = HashMap::new();

    for const_name in &analysis.eval_order {
        let (expr, pos) = analysis.expressions.get(const_name).unwrap();

        let const_value = match eval_const_expr(expr, &local_constants, interner) {
            Ok(value) => value,
            Err(error) if should_defer_to_runtime(&error) => continue,
            Err(error) => {
                return Err(ConstCompileError::EvalError {
                    const_name: interner.resolve(*const_name).to_string(),
                    position: *pos,
                    error,
                });
            }
        };

        local_constants.insert(*const_name, const_value.clone());
        let qualified_name = interner.intern_join(module_name, *const_name);
        module_constants.insert(qualified_name, const_value);
    }

    Ok(module_constants)
}

fn should_defer_to_runtime(error: &ConstEvalError) -> bool {
    matches!(error.code, "E040" | "E041" | "E042" | "E048")
}
