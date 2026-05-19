use lsp_types::{
    DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier, Position, TextDocumentEdit,
    TextEdit, WorkspaceEdit,
};

use crate::handlers::references::{RefBundle, collect_all_uses, gather};
use crate::vfs::FileId;
use crate::workspace::Workspace;

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
        let edits = spans
            .iter()
            .map(|span| {
                OneOf::Left(TextEdit {
                    range: file.snapshot.position_map.flux_span_to_range(*span),
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
