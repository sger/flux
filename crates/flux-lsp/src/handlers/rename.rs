use lsp_types::{
    DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier, Position, TextDocumentEdit,
    TextEdit, Uri, WorkspaceEdit,
};

use crate::handlers::references::{collect_all_uses, node_identifier};
use crate::locator::find_at;
use crate::snapshot::Snapshot;

pub fn rename(
    snapshot: &Snapshot,
    uri: &Uri,
    version: i32,
    position: Position,
    new_name: String,
) -> Option<WorkspaceEdit> {
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;
    let target_id = node_identifier(&node)?;

    let mut spans = Vec::new();
    collect_all_uses(&snapshot.program, target_id, &mut spans);
    if spans.is_empty() {
        return None;
    }

    let edits: Vec<OneOf<TextEdit, lsp_types::AnnotatedTextEdit>> = spans
        .iter()
        .map(|span| {
            OneOf::Left(TextEdit {
                range: snapshot.position_map.flux_span_to_range(*span),
                new_text: new_name.clone(),
            })
        })
        .collect();

    Some(WorkspaceEdit {
        document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: Some(version),
            },
            edits,
        }])),
        ..Default::default()
    })
}
