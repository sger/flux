//! Aether: Cross-function borrow inference (Perceus Section 5 / Lean).
//!
//! For each function definition, analyze how each parameter is used in the body:
//! - If all uses are in borrowed positions → `Borrowed`
//! - If any use is in an owned position → `Owned`
//!
//! The result is a `BorrowRegistry` mapping function names to per-parameter
//! borrow modes. This registry is used by `insert.rs` to skip Rc::clone
//! for arguments passed to borrowed parameters.

use std::collections::HashMap;

use crate::core::{CoreBinder, CoreDef, CoreExpr, CoreProgram};
use crate::syntax::Identifier;

use super::analysis::owned_use_count;

/// How a function parameter is used — does it need ownership or just a reference?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowMode {
    /// Parameter is consumed (stored in ADT, returned, captured by closure).
    /// Caller must Rc::clone the argument.
    Owned,
    /// Parameter is only read (PrimOp operand, Case scrutinee, passed to
    /// another borrowed param). Caller can skip Rc::clone.
    Borrowed,
}

/// Registry of per-parameter borrow modes for all functions in a program.
#[derive(Debug, Clone, Default)]
pub struct BorrowRegistry {
    /// Function name → per-param borrow modes (in parameter order).
    pub modes: HashMap<Identifier, Vec<BorrowMode>>,
}

impl BorrowRegistry {
    /// Look up borrow modes for a function. Returns `None` if the function
    /// is not in the registry (e.g., base functions, unknown callees).
    pub fn get(&self, name: Identifier) -> Option<&[BorrowMode]> {
        self.modes.get(&name).map(|v| v.as_slice())
    }

    /// Check if a specific parameter of a function is borrowed.
    /// Returns `false` if the function or parameter is not in the registry.
    pub fn is_borrowed(&self, name: Identifier, param_index: usize) -> bool {
        self.modes
            .get(&name)
            .and_then(|modes| modes.get(param_index))
            .copied()
            == Some(BorrowMode::Borrowed)
    }
}

/// Infer borrow modes for all function definitions in a Core program.
///
/// For each `CoreDef` whose body is a `Lam`, analyze each parameter:
/// if `owned_use_count(param, body) == 0`, the parameter is `Borrowed`.
pub fn infer_borrow_modes(program: &CoreProgram) -> BorrowRegistry {
    let mut registry = BorrowRegistry::default();

    for def in &program.defs {
        if let Some(modes) = infer_def_modes(def) {
            registry.modes.insert(def.name, modes);
        }
    }

    registry
}

/// Infer borrow modes for a single function definition.
/// Returns `None` if the definition is not a function (no Lam body).
fn infer_def_modes(def: &CoreDef) -> Option<Vec<BorrowMode>> {
    let (params, body) = extract_lam(&def.expr)?;

    let modes = params
        .iter()
        .map(|param| {
            let owned = owned_use_count(param.id, body);
            if owned == 0 {
                BorrowMode::Borrowed
            } else {
                BorrowMode::Owned
            }
        })
        .collect();

    Some(modes)
}

/// Extract parameters and body from a Lam expression.
fn extract_lam(expr: &CoreExpr) -> Option<(&[CoreBinder], &CoreExpr)> {
    match expr {
        CoreExpr::Lam { params, body, .. } => Some((params, body)),
        _ => None,
    }
}
