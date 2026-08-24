use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::syntax::{
    interner::Interner, lexer::Lexer, parser::Parser, program::Program, statement::Statement,
};

use crate::diagnostics::{
    DUPLICATE_MODULE, Diagnostic, DiagnosticBuilder, IMPORT_NOT_FOUND, IMPORT_READ_FAILED,
    INVALID_MODULE_ALIAS, INVALID_MODULE_FILE, INVALID_MODULE_NAME, MODULE_PATH_MISMATCH,
    MULTIPLE_MODULES, NAMESPACE_COLLISION, SCRIPT_NOT_IMPORTABLE,
    position::{Position, Span},
    render_display_path,
};

use super::{
    ImportEdge, ModuleId,
    module_binding::{is_valid_module_alias, is_valid_module_name},
};

/// A module search root, optionally scoped to a package namespace.
///
/// An unscoped root (`namespace: None`) satisfies any import — this is script
/// mode and the `--root` escape hatch, where a bare directory is searched for
/// whatever is imported.
///
/// A scoped root belongs to a resolved package and may only satisfy imports
/// whose first segment is that package's namespace.
/// This is what lets two packages each ship a `Json` module: their roots are
/// scoped to different namespaces, so `A.Json` and `B.Json` never collide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleRoot {
    pub path: PathBuf,
    /// Package namespace this root serves, if any.
    pub namespace: Option<String>,
    /// Declared package name, used to name the packages in a collision. Falls
    /// back to the namespace when a root is scoped without one.
    pub package: Option<String>,
}

impl ModuleRoot {
    /// An unscoped root that satisfies any import.
    pub fn unscoped(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            namespace: None,
            package: None,
        }
    }

    /// A root scoped to `namespace`, satisfying only imports beneath it.
    pub fn scoped(path: impl Into<PathBuf>, namespace: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            namespace: Some(namespace.into()),
            package: None,
        }
    }

    /// A root scoped to `namespace` and owned by the named package.
    pub fn package(
        path: impl Into<PathBuf>,
        namespace: impl Into<String>,
        package: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            namespace: Some(namespace.into()),
            package: Some(package.into()),
        }
    }

    /// How this root is named in diagnostics: its package, else its namespace.
    fn label(&self) -> &str {
        self.package
            .as_deref()
            .or(self.namespace.as_deref())
            .unwrap_or("<unscoped>")
    }

    /// Whether this root may satisfy an import of `name`.
    ///
    /// A scoped root matches when the import's first segment equals its
    /// namespace: root `Json` serves `Json` and `Json.Parse`, but not `Toml`.
    /// The namespace segment is *not* stripped from the path — a package
    /// namespaced `Json` lays its modules out as `<root>/Json/Parse.flx`, so
    /// the namespace is a directory like any other segment.
    pub(super) fn serves(&self, name: &str) -> bool {
        match &self.namespace {
            None => true,
            Some(namespace) => {
                name == namespace
                    || name
                        .split_once('.')
                        .is_some_and(|(first, _)| first == namespace)
            }
        }
    }
}

impl From<PathBuf> for ModuleRoot {
    fn from(path: PathBuf) -> Self {
        Self::unscoped(path)
    }
}

impl From<&Path> for ModuleRoot {
    fn from(path: &Path) -> Self {
        Self::unscoped(path.to_path_buf())
    }
}

pub(super) fn parse_program(path: &Path) -> Result<Program, Vec<Diagnostic>> {
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

pub(super) fn parse_program_with_interner(
    path: &Path,
    interner: Interner,
) -> (Option<Program>, Vec<Diagnostic>, Interner) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) => {
            let error_spec = &IMPORT_READ_FAILED;
            let diag = Diagnostic::make_error(
                error_spec,
                &[&path.display().to_string(), &err.to_string()],
                path.display().to_string(),
                Span::new(Position::default(), Position::default()),
            );
            return (None, vec![diag], interner);
        }
    };

    let lexer = Lexer::new_with_interner(source, interner);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut diagnostics = parser.take_warnings();
    if !parser.errors.is_empty() {
        diagnostics.append(&mut parser.errors);
        for diag in &mut diagnostics {
            diag.set_file(path.display().to_string());
        }
        return (None, diagnostics, interner);
    }

    for diag in &mut diagnostics {
        if diag.file().is_none() {
            diag.set_file(path.display().to_string());
        }
    }
    (Some(program), diagnostics, interner)
}

pub(super) fn resolve_imports(
    path: &Path,
    program: &Program,
    roots: &[ModuleRoot],
    interner: &Interner,
) -> Result<Vec<ImportEdge>, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut edges = Vec::new();

    for statement in &program.statements {
        let (name, alias, position) = match statement {
            Statement::Import {
                name,
                alias,
                except: _,
                exposing: _,
                span,
            } => {
                let name_str = interner.resolve(*name).to_string();
                let alias_str = alias.map(|a| interner.resolve(a).to_string());
                (name_str, alias_str, span.start)
            }
            _ => continue,
        };

        // `Flow` is synthetic and does not resolve to a filesystem module.
        if name == "Flow" {
            continue;
        }

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
    roots: &[ModuleRoot],
) -> Result<(ModuleId, PathBuf), Box<Diagnostic>> {
    let candidates = module_name_candidates(name, roots);
    // Each match remembers the root that produced it, so an ambiguity between
    // two *packages* can be reported as a namespace collision rather than as a
    // bare duplicate-module error naming only the files.
    let mut matches: Vec<(&ModuleRoot, PathBuf)> = Vec::new();
    for (root, candidate) in candidates {
        if candidate.exists() {
            let canonical = fs::canonicalize(&candidate).unwrap_or(candidate);
            if !matches.iter().any(|(_, p)| p == &canonical) {
                matches.push((root, canonical));
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
                    .map(|root| render_display_path(&root.path.display().to_string()).into_owned())
                    .collect::<Vec<_>>()
                    .join(", "),
                render_display_path(&source_path.display().to_string())
            );
            let diag = Diagnostic::make_error(
                error_spec,
                &[name],
                render_display_path(&source_path.display().to_string()).into_owned(),
                Span::new(position, position),
            )
            .with_hint_text(hint);
            return Err(Box::new(diag));
        }
        1 => matches.remove(0).1,
        _ => {
            // Two scoped roots claiming one namespace is a packaging error, and
            // naming the packages explains it far better than listing files.
            let scoped: Vec<&ModuleRoot> = matches
                .iter()
                .map(|(root, _)| *root)
                .filter(|root| root.namespace.is_some())
                .collect();
            if scoped.len() == matches.len()
                && let [first, second, ..] = scoped.as_slice()
            {
                let (first, second) = (first.label(), second.label());
                let claimed = name.split_once('.').map_or(name, |(head, _)| head);
                let error_spec = &NAMESPACE_COLLISION;
                let hint = format!(
                    "Found: {}",
                    matches
                        .iter()
                        .map(|(_, path)| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                let diag = Diagnostic::make_error(
                    error_spec,
                    &[first, second, claimed],
                    render_display_path(&source_path.display().to_string()).into_owned(),
                    Span::new(position, position),
                )
                .with_hint_text(hint);
                return Err(Box::new(diag));
            }

            let error_spec = &DUPLICATE_MODULE;
            let hint = format!(
                "Found: {}",
                matches
                    .iter()
                    .map(|(_, path)| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let diag = Diagnostic::make_error(
                error_spec,
                &[name],
                source_path.display().to_string(),
                Span::new(position, position),
            )
            .with_hint_text(hint);
            return Err(Box::new(diag));
        }
    };

    let (id, canonical_path) = ModuleId::from_path(&import_path);
    Ok((id, canonical_path))
}

/// Candidate file paths for `name`, paired with the root that produced each.
///
/// Roots scoped to a package namespace are skipped unless the import falls
/// beneath that namespace, so a package's root cannot satisfy an unrelated
/// import.
fn module_name_candidates<'a>(
    name: &str,
    roots: &'a [ModuleRoot],
) -> Vec<(&'a ModuleRoot, PathBuf)> {
    let segments: Vec<&str> = name.split('.').collect();
    let Some(file_stem) = segments.last() else {
        return Vec::new();
    };

    let mut paths = Vec::new();
    for root in roots {
        if !root.serves(name) {
            continue;
        }
        // Build directory path from all segments except the last
        let mut dir = root.path.clone();
        for segment in segments.iter().take(segments.len().saturating_sub(1)) {
            dir = dir.join(segment);
        }

        paths.push((root, dir.join(format!("{}.flx", file_stem))));
    }

    paths
}

pub(super) fn normalize_roots(roots: &[ModuleRoot]) -> Vec<ModuleRoot> {
    let mut normalized: Vec<ModuleRoot> = Vec::new();
    for root in roots {
        let canonical = fs::canonicalize(&root.path).unwrap_or_else(|_| root.path.to_path_buf());
        if !normalized
            .iter()
            .any(|r| r.path == canonical && r.namespace == root.namespace)
        {
            normalized.push(ModuleRoot {
                path: canonical,
                namespace: root.namespace.clone(),
                package: root.package.clone(),
            });
        }
    }
    normalized
}

pub(super) fn validate_file_kind(
    path: &Path,
    program: &Program,
    is_entry: bool,
    roots: &[ModuleRoot],
    interner: &Interner,
) -> Result<(), Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut module_decls: Vec<(String, Position)> = Vec::new();

    for statement in &program.statements {
        if let Statement::Module { name, span, .. } = statement {
            module_decls.push((interner.resolve(*name).to_string(), span.start));
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
            let display_path = render_display_path(&path.display().to_string()).into_owned();
            let replacements = [&module_name[..], &display_path[..]];
            let diag = Diagnostic::make_error(
                error_spec,
                &replacements,
                display_path.clone(),
                Span::new(position, position),
            );
            diagnostics.push(diag);
        }
    } else if !is_entry {
        let error_spec = &SCRIPT_NOT_IMPORTABLE;
        let display_path = render_display_path(&path.display().to_string()).into_owned();
        let replacements = [&display_path[..]];
        let diag = Diagnostic::make_error(
            error_spec,
            &replacements,
            display_path.clone(),
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

fn module_name_matches_path(name: &str, path: &Path, roots: &[ModuleRoot]) -> bool {
    let candidates = module_name_candidates(name, roots);
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    candidates.iter().any(|(_, candidate)| {
        let candidate = fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
        candidate == canonical
    })
}
