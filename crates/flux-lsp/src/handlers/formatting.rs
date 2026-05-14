use flux::syntax::formatter::format_source;
use lsp_types::TextEdit;

use crate::snapshot::Snapshot;

pub fn format(snapshot: &Snapshot) -> Vec<TextEdit> {
    let formatted = format_source(snapshot.text.as_ref());
    if formatted == snapshot.text.as_ref() {
        return Vec::new();
    }
    vec![TextEdit {
        range: snapshot.position_map.full_document_range(),
        new_text: formatted,
    }]
}
