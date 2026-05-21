//! `workspace/willRenameFiles` — keep `import`s resolving when a `.flx` module
//! file is renamed or moved.
//!
//! A Flux module's dotted name mirrors its path under a search root: `module
//! A.B.C` must live at `<root>/A/B/C.flx`, and dependents reference it by that
//! dotted path — `import A.B.C` (always) and, for an *unaliased* import, in
//! qualified uses `A.B.C.member`. So renaming the file changes the module name,
//! and every one of those spellings has to move with it.
//!
//! For each rename we work out the old dotted name (from the file's own `module`
//! declaration) and the new one (from the new path, relative to the same root),
//! then return a [`WorkspaceEdit`] that rewrites, across the whole workspace:
//!
//! - the renamed file's own `module <old>` declaration → `module <new>`;
//! - every dependent's `import <old>` path → `import <new>`;
//! - every dependent's unaliased `<old>.member` reference → `<new>.member`
//!   (aliased imports rebind to the alias, so their uses need no change).
//!
//! The edit is applied before the rename, so all edits target the *old* URIs.
//! A rename that can't be expressed as a module move — an entry script with no
//! `module`, a move outside the root, or into a non-module directory — yields
//! no edits and the rename proceeds untouched.

use std::path::Path;
use std::str::FromStr;

use flux::syntax::statement::Statement;
use line_index::TextSize;
use lsp_types::{
    DocumentChanges, FileRename, OneOf, OptionalVersionedTextDocumentIdentifier, Range,
    TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

use crate::snapshot::Snapshot;
use crate::vfs::uri_to_path;
use crate::workspace::Workspace;

/// Build the import-fixing edit for a batch of file renames, or `None` when no
/// rename maps to a module move that any source references.
pub fn will_rename_files(workspace: &mut Workspace, files: &[FileRename]) -> Option<WorkspaceEdit> {
    // Resolve each rename to an (old dotted name → new dotted name) module move.
    let mappings: Vec<(String, String)> = files
        .iter()
        .filter_map(|f| module_rename(workspace, f))
        .collect();
    if mappings.is_empty() {
        return None;
    }

    // Rewrite every workspace file uniformly: the renamed file matches on its
    // own `module` declaration, dependents on their `import` / qualified uses.
    let mut changes: Vec<TextDocumentEdit> = Vec::new();
    for id in workspace.all_file_ids() {
        let Some(uri) = workspace.uri_of(id) else {
            continue;
        };
        let Some(snapshot) = workspace.ensure_snapshot(id).cloned() else {
            continue;
        };
        let mut edits: Vec<TextEdit> = Vec::new();
        for (old_name, new_name) in &mappings {
            collect_file_edits(&snapshot, old_name, new_name, &mut edits);
        }
        if edits.is_empty() {
            continue;
        }
        edits.sort_by_key(|e| (e.range.start.line, e.range.start.character));
        edits.dedup_by(|a, b| a.range == b.range && a.new_text == b.new_text);
        changes.push(TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier { uri, version: None },
            edits: edits.into_iter().map(OneOf::Left).collect(),
        });
    }
    if changes.is_empty() {
        return None;
    }
    Some(WorkspaceEdit {
        document_changes: Some(DocumentChanges::Edits(changes)),
        ..Default::default()
    })
}

/// Resolve one rename to the `(old, new)` dotted module names, or `None` when it
/// isn't a module move we can rewrite for.
fn module_rename(workspace: &mut Workspace, f: &FileRename) -> Option<(String, String)> {
    let old_uri = Uri::from_str(&f.old_uri).ok()?;
    let new_uri = Uri::from_str(&f.new_uri).ok()?;
    let old_path = uri_to_path(&old_uri)?;
    let new_path = uri_to_path(&new_uri)?;
    if !is_flx(&old_path) || !is_flx(&new_path) {
        return None;
    }

    // The file still lives at its old path (the rename hasn't happened), so its
    // own `module` declaration is the authoritative current name.
    let old_id = workspace.file_id(&old_uri)?;
    let old_name = declared_module_name(workspace.ensure_snapshot(old_id)?)?;

    // The dotted name has one segment per trailing path component, so stripping
    // that many ancestors off the old path lands on the search root the name is
    // anchored to. Re-rooting the new path there gives the new dotted name.
    let segments = old_name.split('.').count();
    let root = old_path.ancestors().nth(segments)?;
    let new_name = path_to_module_name(new_path.strip_prefix(root).ok()?)?;
    if new_name == old_name {
        return None;
    }
    Some((old_name, new_name))
}

/// The name of the first `module` a file declares — the module it provides.
/// `None` for an entry script (no module, hence nothing imports it).
fn declared_module_name(snapshot: &Snapshot) -> Option<String> {
    snapshot
        .program
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            Statement::Module { name, .. } => {
                snapshot.interner.try_resolve(*name).map(str::to_string)
            }
            _ => None,
        })
}

/// A path relative to a search root as a dotted module name (`A/B/C.flx` →
/// `A.B.C`). `None` if any component isn't a valid module segment (`[A-Z][A-Za-z0-9]*`)
/// — e.g. the file was moved into a lowercase directory, where it can't be a module.
fn path_to_module_name(rel: &Path) -> Option<String> {
    let comps: Vec<&str> = rel
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<_>>()?;
    let (last, dirs) = comps.split_last()?;
    let stem = last.strip_suffix(".flx")?;
    let mut segments: Vec<&str> = dirs.to_vec();
    segments.push(stem);
    if !segments.iter().all(|s| is_module_segment(s)) {
        return None;
    }
    Some(segments.join("."))
}

fn is_module_segment(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphanumeric())
}

fn is_flx(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("flx")
}

/// Append the edits that rewrite `old_name` → `new_name` in this file: its
/// `module` declaration, any `import` of it, and any unaliased qualified use.
fn collect_file_edits(
    snapshot: &Snapshot,
    old_name: &str,
    new_name: &str,
    out: &mut Vec<TextEdit>,
) {
    let interner = &snapshot.interner;
    let rewrite = |range: Range| TextEdit {
        range,
        new_text: new_name.to_string(),
    };

    for stmt in &snapshot.program.statements {
        match stmt {
            // `import <old> [as ..]` — rewrite just the path after `import`.
            Statement::Import { name, span, .. }
                if interner.try_resolve(*name) == Some(old_name) =>
            {
                if let Some(range) = name_range(snapshot, *span, "import", old_name) {
                    out.push(rewrite(range));
                }
            }
            // The renamed file's own `module <old>` declaration.
            Statement::Module { name, span, .. }
                if interner.try_resolve(*name) == Some(old_name) =>
            {
                if let Some(range) = name_range(snapshot, *span, "module", old_name) {
                    out.push(rewrite(range));
                }
            }
            _ => {}
        }
    }

    // Unaliased imports bind the full dotted path, so qualified uses spell it
    // out (`A.B.member`); the cumulative-prefix span lets us replace just the
    // module part. Aliased uses go through the alias and never match `old_name`.
    let mut paths = Vec::new();
    for stmt in &snapshot.program.statements {
        super::auto_import::collect_paths_stmt(stmt, interner, &mut paths);
    }
    for path in paths {
        for (name, span) in path {
            if name == old_name {
                out.push(rewrite(snapshot.position_map.flux_span_to_range(span)));
            }
        }
    }
}

/// The LSP range of the first occurrence of `name` after `keyword` within the
/// statement spanning `stmt` — used to target just the module path inside an
/// `import` / `module` statement, whose own span covers far more.
fn name_range(
    snapshot: &Snapshot,
    stmt: flux::diagnostics::position::Span,
    keyword: &str,
    name: &str,
) -> Option<Range> {
    let start = u32::from(snapshot.position_map.flux_to_offset(stmt.start)?) as usize;
    let end = u32::from(snapshot.position_map.flux_to_offset(stmt.end)?) as usize;
    let text = snapshot.text.as_ref();
    let slice = text.get(start..end.min(text.len()))?;
    let after = slice.find(keyword).map(|i| i + keyword.len()).unwrap_or(0);
    let rel = slice.get(after..)?.find(name)? + after;
    let name_start = start + rel;
    Some(Range {
        start: snapshot
            .position_map
            .offset_to_lsp(TextSize::from(name_start as u32)),
        end: snapshot
            .position_map
            .offset_to_lsp(TextSize::from((name_start + name.len()) as u32)),
    })
}
