//! Module constant analysis and dependency resolution.

use std::collections::{HashMap, HashSet};

use crate::syntax::{
    block::Block, expression::Expression, position::Position, statement::Statement, symbol::Symbol,
};

use super::dependency::{find_constant_refs, topological_sort_constants};

/// Result of analyzing module constants.
///
/// Contains constants in evaluation order and their expressions.
#[derive(Debug)]
pub struct ModuleConstantAnalysis<'a> {
    /// Constants in evaluation order (dependencies come first)
    pub eval_order: Vec<Symbol>,
    /// Map of constant name to (expression, source position)
    pub expressions: HashMap<Symbol, (&'a Expression, Position)>,
}

/// Analyze module constants: collect, build dependencies, and sort topologically.
///
/// Returns constants in evaluation order, or an error with the cycle path
/// if circular dependencies are detected.
pub fn analyze_module_constants(body: &Block) -> Result<ModuleConstantAnalysis<'_>, Vec<Symbol>> {
    // Step 1: Collect constant definitions
    let mut expressions: HashMap<Symbol, (&Expression, Position)> = HashMap::new();
    let mut names: HashSet<Symbol> = HashSet::new();

    for statement in &body.statements {
        if let Statement::Let {
            name, value, span, ..
        } = statement
        {
            names.insert(*name);
            expressions.insert(*name, (value, span.start));
        }
    }

    // Step 2: Build dependency graph
    let mut dependencies: HashMap<Symbol, Vec<Symbol>> = HashMap::new();

    for (name, (expr, _)) in &expressions {
        let refs = find_constant_refs(expr, &names);
        dependencies.insert(*name, refs);
    }

    // Step 3: Topological sort with cycle detection
    let eval_order = topological_sort_constants(&dependencies)?;

    Ok(ModuleConstantAnalysis {
        eval_order,
        expressions,
    })
}
