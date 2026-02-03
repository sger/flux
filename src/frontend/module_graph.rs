use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use crate::frontend::{
    diagnostics::{
        Diagnostic, DUPLICATE_MODULE, IMPORT_CYCLE,
        IMPORT_NOT_FOUND, IMPORT_READ_FAILED, INVALID_MODULE_ALIAS,
        INVALID_MODULE_FILE, INVALID_MODULE_NAME, MODULE_PATH_MISMATCH,
        MULTIPLE_MODULES, SCRIPT_NOT_IMPORTABLE,
    },
    lexer::Lexer,
    parser::Parser,
    position::{Position, Span},
    program::Program,
    statement::Statement,
};

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

pub fn module_binding_name(name: &str) -> &str {
    name
}

pub fn import_binding_name<'a>(name: &'a str, alias: Option<&'a str>) -> &'a str {
    alias.unwrap_or(name)
}

pub fn is_valid_module_name(name: &str) -> bool {
    let segments: Vec<&str> = name.split('.').collect();
    if segments.is_empty() {
        return false;
    }
    segments
        .iter()
        .all(|segment| is_valid_module_segment(segment))
}

pub fn is_valid_module_alias(name: &str) -> bool {
    is_valid_module_segment(name)
}

fn is_valid_module_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric())
}

fn parse_program(path: &Path) -> Result<Program, Vec<Diagnostic>> {
    let source = fs::read_to_string(path).map_err(|err| {
        let error_spec = &IMPORT_READ_FAILED;
        let diag = Diagnostic::make_error(
            error_spec,
            &[&path.display().to_string(), &err.to_string()],
            path.display().to_string(),
            Span::new(Position::default(), Position::default()),
        );
        vec![diag]
    })?;

    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    if !parser.errors.is_empty() {
        let mut diags = parser.errors;
        for diag in &mut diags {
            diag.set_file(path.display().to_string());
        }
        return Err(diags);
    }

    Ok(program)
}

fn resolve_imports(
    path: &Path,
    program: &Program,
    roots: &[PathBuf],
) -> Result<Vec<ImportEdge>, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut edges = Vec::new();

    for statement in &program.statements {
        let (name, alias, position) = match statement {
            Statement::Import { name, alias, span } => (name.clone(), alias.clone(), span.start),
            _ => continue,
        };

        if !is_valid_module_name(&name) {
            let error_spec = &INVALID_MODULE_NAME;
            let diag = Diagnostic::make_error(
                error_spec,
                &[&name],
                path.display().to_string(),
                Span::new(position, position),
            );
            diagnostics.push(diag);
            continue;
        }

        if let Some(alias) = &alias
            && !is_valid_module_alias(alias)
        {
            let error_spec = &INVALID_MODULE_ALIAS;
            let diag = Diagnostic::make_error(
                error_spec,
                &[alias],
                path.display().to_string(),
                Span::new(position, position),
            );
            diagnostics.push(diag);
            continue;
        }

        match resolve_import_path(path, &name, position, roots) {
            Ok((target, target_path)) => {
                edges.push(ImportEdge {
                    name,
                    position,
                    target,
                    target_path,
                });
            }
            Err(diag) => diagnostics.push(*diag),
        }
    }

    if diagnostics.is_empty() {
        Ok(edges)
    } else {
        Err(diagnostics)
    }
}

fn resolve_import_path(
    source_path: &Path,
    name: &str,
    position: Position,
    roots: &[PathBuf],
) -> Result<(ModuleId, PathBuf), Box<Diagnostic>> {
    let candidates = module_name_candidates(name, roots);
    let mut matches = Vec::new();
    for candidate in candidates {
        if candidate.exists() {
            let canonical = fs::canonicalize(&candidate).unwrap_or(candidate);
            if !matches.iter().any(|p: &PathBuf| p == &canonical) {
                matches.push(canonical);
            }
        }
    }

    let import_path = match matches.len() {
        0 => {
            let error_spec = &IMPORT_NOT_FOUND;
            let hint = format!(
                "Looked for module `{}` under roots: {} (imported from {}).",
                name,
                roots
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                source_path.display()
            );
            let diag = Diagnostic::make_error(
                error_spec,
                &[name],
                source_path.display().to_string(),
                Span::new(position, position),
            ).with_hint_text(hint);
            return Err(Box::new(diag));
        }
        1 => matches.remove(0),
        _ => {
            let error_spec = &DUPLICATE_MODULE;
            let hint = format!(
                "Found: {}",
                matches
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let diag = Diagnostic::make_error(
                error_spec,
                &[name],
                source_path.display().to_string(),
                Span::new(position, position),
            ).with_hint_text(hint);
            return Err(Box::new(diag));
        }
    };

    let (id, canonical_path) = ModuleId::from_path(&import_path);
    Ok((id, canonical_path))
}

fn module_name_candidates(name: &str, roots: &[PathBuf]) -> Vec<PathBuf> {
    let segments: Vec<&str> = name.split('.').collect();
    let Some(file_stem) = segments.last() else {
        return Vec::new();
    };

    let mut paths = Vec::new();
    for root in roots {
        // Build directory path from all segments except the last
        let mut dir = root.clone();
        for segment in segments.iter().take(segments.len().saturating_sub(1)) {
            dir = dir.join(segment);
        }

        paths.push(dir.join(format!("{}.flx", file_stem)));
    }

    paths
}

fn normalize_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut normalized = Vec::new();
    for root in roots {
        let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if !normalized.iter().any(|p| p == &canonical) {
            normalized.push(canonical);
        }
    }
    normalized
}

fn validate_file_kind(
    path: &Path,
    program: &Program,
    is_entry: bool,
    roots: &[PathBuf],
) -> Result<(), Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut module_decls: Vec<(String, Position)> = Vec::new();

    for statement in &program.statements {
        if let Statement::Module { name, span, .. } = statement {
            module_decls.push((name.clone(), span.start));
        }
    }

    if module_decls.len() > 1 {
        let error_spec = &MULTIPLE_MODULES;
        let diag = Diagnostic::make_error(
            error_spec,
            &[],
            path.display().to_string(),
            Span::new(Position::default(), Position::default()),
        );
        diagnostics.push(diag);
        return Err(diagnostics);
    }

    if let Some((module_name, position)) = module_decls.first().cloned() {
        // Module file: only imports + the module declaration are allowed at top level.
        for statement in &program.statements {
            match statement {
                Statement::Import { .. } => {}
                Statement::Module { .. } => {}
                _ => {
                    let error_spec = &INVALID_MODULE_FILE;
                    let diag = Diagnostic::make_error(
                        error_spec,
                        &["Module files may only contain imports and a single module declaration"],
                        path.display().to_string(),
                        Span::new(statement.position(), statement.position()),
                    );
                    diagnostics.push(diag);
                    break;
                }
            }
        }

        if !is_valid_module_name(&module_name) {
            let error_spec = &INVALID_MODULE_NAME;
            let diag = Diagnostic::make_error(
                error_spec,
                &[&module_name],
                path.display().to_string(),
                Span::new(position, position),
            );
            diagnostics.push(diag);
        } else if !module_name_matches_path(&module_name, path, roots) {
            let error_spec = &MODULE_PATH_MISMATCH;
            let diag = Diagnostic::make_error(
                error_spec,
                &[&module_name, &path.display().to_string()],
                path.display().to_string(),
                Span::new(position, position),
            );
            diagnostics.push(diag);
        }
    } else if !is_entry {
        let error_spec = &SCRIPT_NOT_IMPORTABLE;
        let diag = Diagnostic::make_error(
            error_spec,
            &[&path.display().to_string()],
            path.display().to_string(),
            Span::new(Position::default(), Position::default()),
        );
        diagnostics.push(diag);
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn module_name_matches_path(name: &str, path: &Path, roots: &[PathBuf]) -> bool {
    let candidates = module_name_candidates(name, roots);
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    candidates.iter().any(|candidate| {
        let candidate = fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
        candidate == canonical
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

fn topo_order(
    nodes: &HashMap<ModuleId, ModuleNode>,
    entry: &ModuleId,
) -> Result<Vec<ModuleId>, Box<Diagnostic>> {
    let mut colors: HashMap<ModuleId, Color> = HashMap::new();
    let mut stack: Vec<ModuleId> = Vec::new();
    let mut order: Vec<ModuleId> = Vec::new();

    fn dfs(
        id: &ModuleId,
        nodes: &HashMap<ModuleId, ModuleNode>,
        colors: &mut HashMap<ModuleId, Color>,
        stack: &mut Vec<ModuleId>,
        order: &mut Vec<ModuleId>,
    ) -> Result<(), Vec<ModuleId>> {
        colors.insert(id.clone(), Color::Gray);
        stack.push(id.clone());

        if let Some(node) = nodes.get(id) {
            for edge in &node.imports {
                let next = &edge.target;
                match colors.get(next).copied().unwrap_or(Color::White) {
                    Color::White => dfs(next, nodes, colors, stack, order)?,
                    Color::Gray => {
                        if let Some(start) = stack.iter().position(|item| item == next) {
                            let mut cycle = stack[start..].to_vec();
                            cycle.push(next.clone());
                            return Err(cycle);
                        }
                    }
                    Color::Black => {}
                }
            }
        }

        stack.pop();
        colors.insert(id.clone(), Color::Black);
        order.push(id.clone());
        Ok(())
    }

    if let Err(cycle) = dfs(entry, nodes, &mut colors, &mut stack, &mut order) {
        let cycle_str = cycle
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join(" -> ");
        let error_spec = &IMPORT_CYCLE;
        let diag = Diagnostic::make_error(
            error_spec,
            &[&cycle_str],
            entry.as_str().to_string(),
            Span::new(Position::default(), Position::default()),
        );
        return Err(Box::new(diag));
    }

    Ok(order)
}
