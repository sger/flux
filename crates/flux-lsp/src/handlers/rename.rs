use lsp_types::{
    DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier, Position,
    PrepareRenameResponse, TextDocumentEdit, TextEdit, WorkspaceEdit,
};

use crate::global_state::cursor_word_range;
use crate::handlers::references::{
    RefBundle, collect_all_uses, gather, node_identifier, occurrence_range,
};
use crate::locator::find_at;
use crate::snapshot::Snapshot;
use crate::vfs::FileId;
use crate::workspace::Workspace;

/// Handle `textDocument/prepareRename`: validate that the cursor sits on a
/// renameable identifier and return the exact range the editor should make
/// editable. `None` tells the client rename is not valid here (cursor on a
/// keyword, literal, or whitespace).
pub fn prepare_rename(snapshot: &Snapshot, position: Position) -> Option<PrepareRenameResponse> {
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;
    // Only nodes that resolve to an identifier can be renamed.
    node_identifier(&node)?;
    let range = cursor_word_range(snapshot, position)?;
    Some(PrepareRenameResponse::Range(range))
}

/// Pure pass: turn a gathered reference bundle into a multi-file
/// `WorkspaceEdit`. Safe to run on a worker thread — no `Workspace` access.
pub fn compute_workspace_edit(bundle: &RefBundle, new_name: &str) -> Option<WorkspaceEdit> {
    let mut document_changes: Vec<TextDocumentEdit> = Vec::new();
    for file in &bundle.files {
        let mut spans = Vec::new();
        collect_all_uses(&file.snapshot.program, bundle.target_id, &mut spans);
        if spans.is_empty() {
            continue;
        }
        // `collect_all_uses` reports a declaration's whole-statement span; the
        // edit must touch only the name, or renaming `fn twice(n)` would
        // replace the entire signature.
        let name = file
            .snapshot
            .interner
            .try_resolve(bundle.target_id)
            .unwrap_or("");
        let edits = spans
            .iter()
            .map(|span| {
                OneOf::Left(TextEdit {
                    range: occurrence_range(&file.snapshot.position_map, *span, name),
                    new_text: new_name.to_string(),
                })
            })
            .collect();
        document_changes.push(TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: file.uri.clone(),
                // Open buffers carry a version; on-disk dependency modules
                // do not — `None` is a valid unversioned edit.
                version: file.version,
            },
            edits,
        });
    }

    if document_changes.is_empty() {
        return None;
    }
    Some(WorkspaceEdit {
        document_changes: Some(DocumentChanges::Edits(document_changes)),
        ..Default::default()
    })
}

/// Rename the identifier under the cursor.
///
/// A top-level / module-level symbol is renamed across its whole
/// module-graph component, producing one `TextDocumentEdit` per affected
/// file; a purely local binding is renamed only within `file`.
pub fn rename(
    workspace: &mut Workspace,
    file: FileId,
    position: Position,
    new_name: String,
) -> Option<WorkspaceEdit> {
    let bundle = gather(workspace, file, position)?;
    compute_workspace_edit(&bundle, &new_name)
}
