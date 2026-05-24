//! `source.organizeImports` — sort the file's import block and drop unused
//! imports.
//!
//! Only the contiguous run of top-level `import`s at the start of the file is
//! reorganized: imports are sorted by module name and exact duplicates removed.
//! An import is dropped as unused only when it has no `exposing` clause (so its
//! sole binding is the module / alias name) and the linter's W003 "unused
//! import" check flags it — `exposing` imports are always kept, since their
//! unqualified bindings aren't tracked by that check, so removing them could
//! break the file.

use std::collections::HashSet;

use flux::syntax::linter::Linter;
use flux::syntax::statement::{ImportExposing, Statement};
use line_index::TextSize;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, DocumentChanges, OneOf,
    OptionalVersionedTextDocumentIdentifier, Range, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

use crate::snapshot::Snapshot;

/// Whether the client asked for `source.organizeImports` (directly or via its
/// parent `source` kind). Source actions are not offered for a bare-cursor
/// request (`only == None`), only when explicitly requested.
pub fn organize_imports_requested(only: Option<&[CodeActionKind]>) -> bool {
    let Some(only) = only else {
        return false;
    };
    only.iter().any(|k| {
        let s = k.as_str();
        s == "source" || s == "source.organizeImports"
    })
}

/// One import in the leading block, sliced from the original source.
struct ImportEntry {
    module_name: String,
    text: String,
    /// `(line, column)` of the statement start, to match the linter's W003.
    start_key: (usize, usize),
    start_off: usize,
    end_off: usize,
    /// No `exposing` clause → its only binding is the module/alias name, so the
    /// W003 check is reliable and the import may be dropped when unused.
    removable: bool,
}

/// Build the `source.organizeImports` action, or `None` when the file has no
/// reorganizable import block or it is already organized.
pub fn organize_imports_action(snapshot: &Snapshot, uri: &Uri) -> Option<CodeActionOrCommand> {
    let imports = leading_import_block(snapshot);
    if imports.is_empty() {
        return None;
    }
    let unused = unused_import_starts(snapshot);

    // Keep used imports and every `exposing` import; sort by module name and
    // drop exact-duplicate lines.
    let mut kept: Vec<(&str, &str)> = imports
        .iter()
        .filter(|imp| !(imp.removable && unused.contains(&imp.start_key)))
        .map(|imp| (imp.module_name.as_str(), imp.text.as_str()))
        .collect();
    kept.sort_by(|a, b| a.0.cmp(b.0));
    let mut seen = HashSet::new();
    let new_text = kept
        .into_iter()
        .filter(|(_, text)| seen.insert(*text))
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n");

    // No-op when the block is already organized.
    let block_start = imports.first()?.start_off;
    let block_end = imports.last()?.end_off;
    let original = snapshot.text.get(block_start..block_end)?;
    if original == new_text {
        return None;
    }

    let range = Range {
        start: snapshot
            .position_map
            .offset_to_lsp(TextSize::from(block_start as u32)),
        end: snapshot
            .position_map
            .offset_to_lsp(TextSize::from(block_end as u32)),
    };
    let edit = WorkspaceEdit {
        document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: None,
            },
            edits: vec![OneOf::Left(TextEdit { range, new_text })],
        }])),
        ..Default::default()
    };
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Organize imports".to_string(),
        kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
        edit: Some(edit),
        ..Default::default()
    }))
}

/// The maximal contiguous run of top-level `import` statements starting at the
/// file's first import. (Imports after a gap, or nested in a `module {}`, are
/// left untouched.)
fn leading_import_block(snapshot: &Snapshot) -> Vec<ImportEntry> {
    let pm = &snapshot.position_map;
    let mut out = Vec::new();
    let mut started = false;
    for stmt in &snapshot.program.statements {
        let Statement::Import {
            name,
            exposing,
            span,
            ..
        } = stmt
        else {
            if started {
                break;
            }
            continue;
        };
        started = true;
        let (Some(start), Some(end)) = (pm.flux_to_offset(span.start), pm.flux_to_offset(span.end))
        else {
            continue;
        };
        let start_off = u32::from(start) as usize;
        let end_off = u32::from(end) as usize;
        let Some(text) = snapshot.text.get(start_off..end_off) else {
            continue;
        };
        out.push(ImportEntry {
            module_name: snapshot
                .interner
                .try_resolve(*name)
                .unwrap_or("")
                .to_string(),
            text: text.to_string(),
            start_key: (span.start.line, span.start.column),
            start_off,
            end_off,
            removable: matches!(exposing, ImportExposing::None),
        });
    }
    out
}

/// `(line, column)` of every binding the linter flags with warning `code`.
/// The linter isn't part of the snapshot build, so each caller runs it on
/// demand; both unused-import (W003) and unused-`let` (W001) fixes share this.
pub(crate) fn warning_starts(snapshot: &Snapshot, code: &str) -> HashSet<(usize, usize)> {
    Linter::new(None, &snapshot.interner)
        .lint(&snapshot.program)
        .into_iter()
        .filter(|diag| diag.code() == Some(code))
        .filter_map(|diag| diag.span().map(|s| (s.start.line, s.start.column)))
        .collect()
}

/// `(line, column)` of every import the linter reports as unused (W003).
pub(crate) fn unused_import_starts(snapshot: &Snapshot) -> HashSet<(usize, usize)> {
    warning_starts(snapshot, "W003")
}

/// `(line, column)` of every `let` binding the linter reports as unused (W001).
pub(crate) fn unused_let_starts(snapshot: &Snapshot) -> HashSet<(usize, usize)> {
    warning_starts(snapshot, "W001")
}
