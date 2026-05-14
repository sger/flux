use lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse};

use crate::snapshot::Snapshot;

const KEYWORDS: &[&str] = &[
    "let", "fn", "if", "else", "match", "data", "effect", "alias", "class", "instance", "import",
    "module", "public", "perform", "handle", "do", "true", "false", "return",
];

pub fn complete(snapshot: &Snapshot) -> CompletionResponse {
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
