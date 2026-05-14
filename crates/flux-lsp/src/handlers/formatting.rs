use flux::syntax::formatter::format_source;
use lsp_types::{Position, Range, TextEdit};

use crate::snapshot::Snapshot;

pub fn format(snapshot: &Snapshot) -> Vec<TextEdit> {
    let formatted = format_source(snapshot.text.as_ref());
    if formatted == snapshot.text.as_ref() {
        return Vec::new();
    }
    let line_count = snapshot.text.lines().count() as u32;
    let range = Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: line_count + 1,
            character: 0,
        },
    };
    vec![TextEdit {
        range,
        new_text: formatted,
    }]
}
