use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use crate::frontend::{diagnostics::Diagnostic, position::Position, program::Program};

mod module_binding;
mod module_order;
mod module_resolution;

pub use module_binding::{
    import_binding_name, is_valid_module_alias, is_valid_module_name, module_binding_name,
};

use module_order::topo_order;
use module_resolution::{normalize_roots, parse_program, resolve_imports, validate_file_kind};

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

impl ModuleGraph {
    pub fn build_with_entry_and_roots(
        entry_path: &Path,
        entry_program: &Program,
        roots: &[PathBuf],
    ) -> Result<Self, Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();
        let (entry_id, entry_path) = ModuleId::from_path(entry_path);
        let roots = normalize_roots(roots);

        let mut nodes: HashMap<ModuleId, ModuleNode> = HashMap::new();
        let mut pending: Vec<PathBuf> = vec![entry_path.clone()];

        while let Some(path) = pending.pop() {
            let (id, canonical_path) = ModuleId::from_path(&path);
            if nodes.contains_key(&id) {
                continue;
            }

            let program = if id == entry_id {
                entry_program.clone()
            } else {
                match parse_program(&canonical_path) {
                    Ok(program) => program,
                    Err(mut diags) => {
                        diagnostics.append(&mut diags);
                        continue;
                    }
                }
            };

            if let Err(mut diags) =
                validate_file_kind(&canonical_path, &program, id == entry_id, &roots)
            {
                diagnostics.append(&mut diags);
                continue;
            }

            let imports = match resolve_imports(&canonical_path, &program, &roots) {
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

        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }

        let order = topo_order(&nodes, &entry_id).map_err(|diag| vec![*diag])?;

        Ok(Self {
            entry: entry_id,
            nodes,
            order,
        })
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
}
