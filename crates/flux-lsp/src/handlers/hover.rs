use flux::ast::type_infer::{display_infer_type, render_scheme_canonical};
use flux::syntax::expression::Expression;
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::keywords::{is_offset_in_comment_or_string, keyword_doc, word_at_offset};
use crate::locator::{NodeRef, find_at};
use crate::snapshot::Snapshot;

pub fn hover_at(snapshot: &Snapshot, position: Position) -> Option<Hover> {
    // Keyword check before AST lookup: the locator returns the enclosing
    // statement for keywords (since they don't have their own AST node),
    // which produces less-useful hover content than dedicated docs.
    // Reject hover-in-comment/string so prose like `// use let to bind`
    // doesn't surface keyword docs for the prose mention.
    if let Some(offset) = snapshot.position_map.lsp_to_offset(position) {
        let off: usize = offset.into();
        if !is_offset_in_comment_or_string(snapshot.text.as_ref(), off)
            && let Some(word) = word_at_offset(snapshot.text.as_ref(), off)
            && let Some(doc) = keyword_doc(word)
        {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: doc.to_string(),
                }),
                range: None,
            });
        }
    }

    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;
    let value = render(snapshot, &node)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```flux\n{value}\n```"),
        }),
        range: None,
    })
}

fn render(snapshot: &Snapshot, node: &NodeRef) -> Option<String> {
    match node {
        NodeRef::Expr(expr) => render_expr(snapshot, expr),
        NodeRef::Pattern(_) => None, // Filled in M4d.
        NodeRef::Statement(_) => None,
        NodeRef::TypeExprNamed { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("type: {resolved}"))
        }
        NodeRef::EffectName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("effect: {resolved}"))
        }
        NodeRef::EffectRowVar { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("row var: |{resolved}"))
        }
        NodeRef::MemberAccessMember { member, .. } => {
            let resolved = snapshot.interner.try_resolve(*member)?;
            Some(format!("field: {resolved}"))
        }
        NodeRef::NamedFieldInitName { field_name, .. } => {
            let resolved = snapshot.interner.try_resolve(*field_name)?;
            Some(format!("field: {resolved}"))
        }
        NodeRef::NamedConstructorName { name, expr_id, .. } => {
            if let Some(infer) = snapshot.infer.as_ref()
                && let Some(ty) = infer.expr_types.get(expr_id)
            {
                return Some(display_infer_type(ty, &snapshot.interner));
            }
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("constructor: {resolved}"))
        }
        NodeRef::PerformOpName { name, .. } | NodeRef::HandleArmOpName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("operation: {resolved}"))
        }
        NodeRef::ImportName { qualified, .. } => {
            let resolved = snapshot.interner.try_resolve(*qualified)?;
            Some(format!("module: {resolved}"))
        }
        NodeRef::ImportAlias {
            alias, qualified, ..
        } => {
            let a = snapshot.interner.try_resolve(*alias)?;
            let q = snapshot.interner.try_resolve(*qualified)?;
            Some(format!("alias {a} = {q}"))
        }
        NodeRef::DataName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("data: {resolved}"))
        }
        NodeRef::DataVariantName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("variant: {resolved}"))
        }
        NodeRef::DataFieldName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("field: {resolved}"))
        }
        NodeRef::EffectDeclName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("effect: {resolved}"))
        }
        NodeRef::EffectOpName { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("op: {resolved}"))
        }
        NodeRef::DeclName { name, binding_span } => {
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
        NodeRef::FunctionParameter { name, .. } => {
            let resolved = snapshot.interner.try_resolve(*name)?;
            Some(format!("parameter: {resolved}"))
        }
    }
}

fn render_expr(snapshot: &Snapshot, expr: &Expression) -> Option<String> {
    if let Some(infer) = snapshot.infer.as_ref()
        && let Some(ty) = infer.expr_types.get(&expr.expr_id())
    {
        return Some(display_infer_type(ty, &snapshot.interner));
    }
    // Module-name fallback: identifier expression on the LHS of a member
    // access whose name matches a loaded module. The locator emits a more
    // specific `MemberAccessMember` for the RHS; this branch handles the
    // LHS (the bare identifier).
    if let Expression::Identifier { name, .. } = expr
        && let Some(resolved) = snapshot.interner.try_resolve(*name)
        && snapshot.module_short_names.contains(resolved)
    {
        return Some(format!("module: {resolved}"));
    }
    None
}
