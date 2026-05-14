use flux::ast::type_infer::{display_infer_type, render_scheme_canonical};
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::hover_index::HoverTarget;
use crate::snapshot::Snapshot;

pub fn hover_at(snapshot: &Snapshot, position: Position) -> Option<Hover> {
    let target = snapshot
        .hover_index
        .target_at(position, &snapshot.position_map)?;
    let value = render(snapshot, target)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```flux\n{value}\n```"),
        }),
        range: None,
    })
}

fn render(snapshot: &Snapshot, target: &HoverTarget) -> Option<String> {
    match target {
        HoverTarget::Expr(id) => {
            let infer = snapshot.infer.as_ref()?;
            let ty = infer.expr_types.get(id)?;
            Some(display_infer_type(ty, &snapshot.interner))
        }
        HoverTarget::DeclName { name, binding_span } => {
            let key = (
                binding_span.start.line,
                binding_span.start.column,
                binding_span.end.line,
                binding_span.end.column,
            );
            if let Some(infer) = snapshot.infer.as_ref()
                && let Some(scheme) = infer.resolved_binding_schemes_by_span.get(&key)
            {
                return Some(render_scheme_canonical(&snapshot.interner, scheme));
            }
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("decl: {resolved}"))
        }
        HoverTarget::EffectName(name) => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("effect: {resolved}"))
        }
        HoverTarget::EffectRowVar(name) => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("row var: |{resolved}"))
        }
        HoverTarget::TypeName(name) => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("type: {resolved}"))
        }
    }
}
