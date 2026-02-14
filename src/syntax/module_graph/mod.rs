use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    diagnostics::{Diagnostic, position::Position},
    syntax::{
        interner::Interner, module_graph::module_resolution::parse_program_with_interner,
        program::Program,
    },
};

mod module_binding;
mod module_order;
mod module_resolution;

pub use module_binding::{
    import_binding_name, is_valid_module_alias, is_valid_module_name, module_binding_name,
};

use module_order::topo_order;
use module_resolution::{normalize_roots, resolve_imports, validate_file_kind};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModuleId(String);

impl ModuleId {
    fn from_path(path: &Path) -> (Self, PathBuf) {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let id = ModuleId(canonical.to_string_lossy().to_string());
        (id, canonical)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct ImportEdge {
    pub name: String,
    pub position: Position,
    pub target: ModuleId,
    pub target_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ModuleNode {
    pub id: ModuleId,
    pub path: PathBuf,
    pub program: Program,
    pub imports: Vec<ImportEdge>,
}

#[derive(Debug, Clone)]
pub struct ModuleGraph {
    entry: ModuleId,
    nodes: HashMap<ModuleId, ModuleNode>,
    order: Vec<ModuleId>,
}

/// Result of building a module graph. Always returns whatever graph could be
/// constructed (possibly empty) alongside any diagnostics collected during
/// module discovery, parsing, validation, and topological sorting.
pub struct GraphBuildResult {
    pub graph: ModuleGraph,
    pub interner: Interner,
    pub diagnostics: Vec<Diagnostic>,
    /// Canonical paths of modules that failed to parse or validate.
    pub failed_modules: HashSet<PathBuf>,
}

impl ModuleGraph {
    pub fn build_with_entry_and_roots(
        entry_path: &Path,
        entry_program: &Program,
        interner: Interner,
        roots: &[PathBuf],
    ) -> GraphBuildResult {
        let mut diagnostics = Vec::new();
        let mut failed_modules: HashSet<PathBuf> = HashSet::new();
        let (entry_id, entry_path) = ModuleId::from_path(entry_path);
        let roots = normalize_roots(roots);

        let mut nodes: HashMap<ModuleId, ModuleNode> = HashMap::new();
        let mut pending: Vec<PathBuf> = vec![entry_path.clone()];
        let mut interner = interner;

        while let Some(path) = pending.pop() {
            let (id, canonical_path) = ModuleId::from_path(&path);
            if nodes.contains_key(&id) || failed_modules.contains(&canonical_path) {
                continue;
            }

            let program = if id == entry_id {
                entry_program.clone()
            } else {
                let (result, returned_interner) =
                    parse_program_with_interner(&canonical_path, interner);

                interner = returned_interner;

                match result {
                    Ok(program) => program,
                    Err(mut diags) => {
                        diagnostics.append(&mut diags);
                        failed_modules.insert(canonical_path);
                        continue;
                    }
                }
            };

            if let Err(mut diags) =
                validate_file_kind(&canonical_path, &program, id == entry_id, &roots, &interner)
            {
                diagnostics.append(&mut diags);
                failed_modules.insert(canonical_path);
                continue;
            }

            let imports = match resolve_imports(&canonical_path, &program, &roots, &interner) {
                Ok(mut imports) => {
                    // Sort edges by stable ID to make traversal deterministic.
                    imports.sort_by(|a, b| a.target.as_str().cmp(b.target.as_str()));
                    imports
                }
                Err(mut diags) => {
                    diagnostics.append(&mut diags);
                    Vec::new()
                }
            };

            for edge in &imports {
                pending.push(edge.target_path.clone());
            }

            nodes.insert(
                id.clone(),
                ModuleNode {
                    id,
                    path: canonical_path,
                    program,
                    imports,
                },
            );
        }

        // Topological sort on successfully parsed modules only.
        // On cycle, push the diagnostic and use an empty order (nothing compiles).
        let order = match topo_order(&nodes, &entry_id) {
            Ok(order) => order,
            Err(diag) => {
                diagnostics.push(*diag);
                Vec::new()
            }
        };

        GraphBuildResult {
            graph: Self {
                entry: entry_id,
                nodes,
                order,
            },
            interner,
            diagnostics,
            failed_modules,
        }
    }

    pub fn topo_order(&self) -> Vec<&ModuleNode> {
        self.order
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .collect()
    }

    pub fn imported_files(&self) -> Vec<String> {
        let mut files: Vec<String> = self
            .nodes
            .keys()
            .filter(|id| **id != self.entry)
            .map(|id| id.as_str().to_string())
            .collect();
        files.sort();
        files
    }

    /// Returns the number of modules in the graph (including the entry).
    pub fn module_count(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod module_graph_test;
