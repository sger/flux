use lsp_types::{
    DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier, Position, TextDocumentEdit,
    TextEdit, WorkspaceEdit,
};

use crate::handlers::references::{collect_all_uses, node_identifier, reference_scope};
use crate::locator::find_at;
use crate::vfs::FileId;
use crate::workspace::Workspace;

/// Rename the identifier under the cursor.
///
/// A top-level / module-level symbol is renamed across its whole
/// module-graph component, producing one `TextDocumentEdit` per affected
/// file; a purely local binding is renamed only within `file`. See
/// [`reference_scope`] for the scoping rule.
pub fn rename(
    workspace: &Workspace,
    file: FileId,
    position: Position,
    new_name: String,
) -> Option<WorkspaceEdit> {
    let snapshot = workspace.snapshot(file)?;
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;
    let target_id = node_identifier(&node)?;

    let mut document_changes: Vec<TextDocumentEdit> = Vec::new();
    for fid in reference_scope(workspace, file, target_id) {
        let Some(snap) = workspace.snapshot(fid) else {
            continue;
        };
        let Some(uri) = workspace.uri_of(fid) else {
            continue;
        };
        let mut spans = Vec::new();
        collect_all_uses(&snap.program, target_id, &mut spans);
        if spans.is_empty() {
            continue;
        }
        let edits = spans
            .iter()
            .map(|span| {
                OneOf::Left(TextEdit {
                    range: snap.position_map.flux_span_to_range(*span),
                    new_text: new_name.clone(),
                })
            })
            .collect();
        document_changes.push(TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri,
                // Open buffers carry a version; on-disk dependency modules
                // do not — `None` is a valid unversioned edit.
                version: workspace.version(fid),
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
