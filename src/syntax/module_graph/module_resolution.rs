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
    MULTIPLE_MODULES, SCRIPT_NOT_IMPORTABLE,
    position::{Position, Span},
};

use super::{
    ImportEdge, ModuleId,
    module_binding::{is_valid_module_alias, is_valid_module_name},
};

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
    roots: &[PathBuf],
    interner: &Interner,
) -> Result<Vec<ImportEdge>, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut edges = Vec::new();

    for statement in &program.statements {
        let (name, alias, position) = match statement {
            Statement::Import { name, alias, span } => {
                let name_str = interner.resolve(*name).to_string();
                let alias_str = alias.map(|a| interner.resolve(a).to_string());
                (name_str, alias_str, span.start)
            }
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
            )
            .with_hint_text(hint);
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
            )
            .with_hint_text(hint);
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

pub(super) fn normalize_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut normalized = Vec::new();
    for root in roots {
        let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if !normalized.iter().any(|p| p == &canonical) {
            normalized.push(canonical);
        }
    }
    normalized
}

pub(super) fn validate_file_kind(
    path: &Path,
    program: &Program,
    is_entry: bool,
    roots: &[PathBuf],
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
