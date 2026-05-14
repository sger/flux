use flux::ast::type_infer::display_infer_type;
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::snapshot::Snapshot;

pub fn hover_at(snapshot: &Snapshot, position: Position) -> Option<Hover> {
    let id = snapshot.span_index.expr_at(position)?;
    let infer = snapshot.infer.as_ref()?;
    let ty = infer.expr_types.get(&id)?;
    let rendered = display_infer_type(ty, &snapshot.interner);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```flux\n{rendered}\n```"),
        }),
        range: None,
    })
}
