//! Module interface files (.flxi) for separate compilation.
//!
//! A `.flxi` file stores the exported type signatures of a compiled module.
//! When a dependent module imports it, the compiler loads the `.flxi` to get
//! type information without recompiling the dependency.
//!
//! Format: JSON for human readability and debuggability.
//! Cache invalidation: source hash comparison.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bytecode::bytecode_cache::hash_bytes;
use crate::bytecode::compiler::contracts::{ContractKey, FnContract};
use crate::syntax::interner::Interner;

/// On-disk representation of a module's exported interface.
#[derive(Debug, Serialize, Deserialize)]
pub struct ModuleInterface {
    /// Module name (e.g., "Day07Solver").
    pub module_name: String,
    /// SHA-256 hash of the source file contents.
    pub source_hash: String,
    /// Compiler version that generated this interface.
    pub compiler_version: String,
    /// Exported function signatures.
    pub exports: Vec<ExportedFunction>,
}

/// A single exported function's signature.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExportedFunction {
    /// Function name (unqualified, e.g., "map").
    pub name: String,
    /// Arity (parameter count).
    pub arity: usize,
    /// Type parameters (e.g., ["a", "b"]).
    pub type_params: Vec<String>,
    /// Parameter type signatures as display strings (e.g., ["Array<a>", "(a) -> b"]).
    pub params: Vec<String>,
    /// Return type signature as display string (e.g., "Array<b>").
    pub return_type: String,
    /// Effect annotations as display strings (e.g., ["IO"]).
    pub effects: Vec<String>,
}

/// Build a `ModuleInterface` from the compiler's contract table.
///
/// Extracts public functions belonging to `module_name` and serializes
/// their type signatures using the interner for display.
pub fn build_interface(
    module_name: &str,
    module_sym: crate::syntax::Identifier,
    source_hash: &[u8; 32],
    contracts: &HashMap<ContractKey, FnContract>,
    visibility: &HashMap<(crate::syntax::Identifier, crate::syntax::Identifier), bool>,
    interner: &Interner,
) -> ModuleInterface {
    let mut exports = Vec::new();

    for (key, contract) in contracts {
        // Only include functions from this module.
        if key.module_name != Some(module_sym) {
            continue;
        }
        // Only include public functions.
        if visibility.get(&(module_sym, key.function_name)) != Some(&true) {
            continue;
        }
        // Only include fully-typed functions.
        if contract.params.iter().any(|p| p.is_none()) || contract.ret.is_none() {
            continue;
        }

        let fn_name = interner.resolve(key.function_name).to_string();
        let type_params: Vec<String> = contract
            .type_params
            .iter()
            .map(|tp| interner.resolve(*tp).to_string())
            .collect();
        let params: Vec<String> = contract
            .params
            .iter()
            .filter_map(|p| p.as_ref().map(|ty| ty.display_with(interner)))
            .collect();
        let return_type = contract
            .ret
            .as_ref()
            .map(|ty| ty.display_with(interner))
            .unwrap_or_default();
        let effects: Vec<String> = contract
            .effects
            .iter()
            .map(|e| e.display_with(interner))
            .collect();

        exports.push(ExportedFunction {
            name: fn_name,
            arity: key.arity,
            type_params,
            params,
            return_type,
            effects,
        });
    }

    // Sort exports by name for deterministic output.
    exports.sort_by(|a, b| a.name.cmp(&b.name));

    ModuleInterface {
        module_name: module_name.to_string(),
        source_hash: hex::encode(source_hash),
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
        exports,
    }
}

/// Compute the `.flxi` cache path for a module source file.
///
/// Stores alongside the source: `MyModule.flx` → `MyModule.flxi`.
pub fn interface_path(source_path: &Path) -> PathBuf {
    source_path.with_extension("flxi")
}

/// Save a module interface to disk as JSON.
pub fn save_interface(path: &Path, interface: &ModuleInterface) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(interface)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, json)
}

/// Load a module interface from disk.
///
/// Returns `None` if the file doesn't exist or can't be parsed.
pub fn load_interface(path: &Path) -> Option<ModuleInterface> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Check if a cached interface is still valid for the given source.
///
/// Returns the cached interface if the source hash matches and the compiler
/// version is compatible. Returns `None` if the cache is stale.
pub fn load_valid_interface(source_path: &Path, source: &str) -> Option<ModuleInterface> {
    let cache_path = interface_path(source_path);
    let interface = load_interface(&cache_path)?;

    // Check compiler version.
    if interface.compiler_version != env!("CARGO_PKG_VERSION") {
        return None;
    }

    // Check source hash.
    let current_hash = hex::encode(&hash_bytes(source.as_bytes()));
    if interface.source_hash != current_hash {
        return None;
    }

    Some(interface)
}

/// Hex encoding (avoids adding `hex` crate dependency).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_roundtrip() {
        let interface = ModuleInterface {
            module_name: "TestMod".to_string(),
            source_hash: "abc123".to_string(),
            compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            exports: vec![
                ExportedFunction {
                    name: "double".to_string(),
                    arity: 1,
                    type_params: vec![],
                    params: vec!["Int".to_string()],
                    return_type: "Int".to_string(),
                    effects: vec![],
                },
                ExportedFunction {
                    name: "map".to_string(),
                    arity: 2,
                    type_params: vec!["a".to_string(), "b".to_string()],
                    params: vec!["Array<a>".to_string(), "(a) -> b".to_string()],
                    return_type: "Array<b>".to_string(),
                    effects: vec![],
                },
            ],
        };

        let json = serde_json::to_string_pretty(&interface).unwrap();
        let loaded: ModuleInterface = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.module_name, "TestMod");
        assert_eq!(loaded.exports.len(), 2);
        assert_eq!(loaded.exports[0].name, "double");
        assert_eq!(loaded.exports[0].params, vec!["Int"]);
        assert_eq!(loaded.exports[1].name, "map");
        assert_eq!(loaded.exports[1].type_params, vec!["a", "b"]);
    }

    #[test]
    fn interface_path_from_source() {
        let path = interface_path(Path::new("examples/aoc/2024/Day07Solver.flx"));
        assert_eq!(path, PathBuf::from("examples/aoc/2024/Day07Solver.flxi"));
    }
}
