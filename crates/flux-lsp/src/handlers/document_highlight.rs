//! `textDocument/documentHighlight` — highlight every occurrence of the
//! symbol under the cursor, scoped to the current file.
//!
//! This is the single-file sibling of find-references: it resolves the
//! cursor's target identifier the same way, then walks only this file's
//! program. A pure function over one `Snapshot`, safe on the worker thread.

use lsp_types::{DocumentHighlight, DocumentHighlightKind, Position};

use crate::handlers::references::{collect_all_uses, node_identifier};
use crate::locator::find_at;
use crate::snapshot::Snapshot;

/// Every occurrence of the identifier under `position` in `snapshot`'s file.
/// Empty when the cursor is not on a navigable identifier.
pub fn document_highlights(snapshot: &Snapshot, position: Position) -> Vec<DocumentHighlight> {
    let Some(target) = snapshot.position_map.lsp_to_flux(position) else {
        return Vec::new();
    };
    let Some(node) = find_at(&snapshot.program, &snapshot.interner, target) else {
        return Vec::new();
    };
    let Some(target_id) = node_identifier(&node) else {
        return Vec::new();
    };

    let mut spans = Vec::new();
    collect_all_uses(&snapshot.program, target_id, &mut spans);
    spans
        .into_iter()
        .map(|span| DocumentHighlight {
            range: snapshot.position_map.flux_span_to_range(span),
            // The compiler frontend does not distinguish read vs. write
            // occurrences, so every hit is reported as `TEXT`.
            kind: Some(DocumentHighlightKind::TEXT),
        })
        .collect()
}
