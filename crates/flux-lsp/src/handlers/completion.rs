use lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse, Position};

use crate::snapshot::Snapshot;

const KEYWORDS: &[&str] = &[
    "let", "fn", "if", "else", "match", "data", "effect", "alias", "class", "instance", "import",
    "module", "public", "perform", "handle", "do", "true", "false", "return",
];

pub fn complete(snapshot: &Snapshot, position: Position) -> CompletionResponse {
    // Look at the text just before the cursor to decide if we're in a
    // member-access context (`Foo.`). A precise AST-based detector is the
    // M4-followup; this string-based check covers the common cases the
    // user has reported (`String.`, `Array.`, ...) reliably enough.
    if let Some(module_name) = preceding_module_name(snapshot, position)
        && let Some(members) = snapshot.module_members.get(&module_name)
    {
        let items: Vec<CompletionItem> = members
            .iter()
            .map(|m| CompletionItem {
                label: m.clone(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(format!("{module_name}.{m}")),
                ..Default::default()
            })
            .collect();
        return CompletionResponse::Array(items);
    }

    let mut items: Vec<CompletionItem> = snapshot
        .symbol_index
        .names()
        .map(|name| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            ..Default::default()
        })
        .collect();
    items.extend(KEYWORDS.iter().map(|kw| CompletionItem {
        label: (*kw).to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        ..Default::default()
    }));
    CompletionResponse::Array(items)
}

/// Return the identifier-shaped token immediately preceding `position` if it
/// is followed by a `.`. Used to detect member-access completion context like
/// `String.|` (cursor at `|`).
fn preceding_module_name(snapshot: &Snapshot, position: Position) -> Option<String> {
    let offset = snapshot.position_map.lsp_to_offset(position)?;
    let off: usize = offset.into();
    let text = snapshot.text.as_ref();
    let bytes = text.as_bytes();
    if off == 0 || off > bytes.len() {
        return None;
    }
    // Walk back to the byte before the cursor. If it's a `.`, scan back
    // further to capture the identifier-shaped token before the dot.
    let mut i = off;
    if bytes[i - 1] != b'.' {
        return None;
    }
    i -= 1;
    let dot_pos = i;
    // Scan back identifier chars [A-Za-z0-9_].
    while i > 0 && is_ident_byte(bytes[i - 1]) {
        i -= 1;
    }
    if i == dot_pos {
        return None;
    }
    Some(std::str::from_utf8(&bytes[i..dot_pos]).ok()?.to_string())
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
