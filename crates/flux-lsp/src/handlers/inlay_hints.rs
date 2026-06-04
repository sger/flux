use flux::ast::type_infer::{display_infer_type, render_scheme_canonical};
use flux::diagnostics::position::Position as FluxPosition;
use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::expression::Pattern;
use flux::syntax::statement::Statement;
use flux::types::infer_type::InferType;
use line_index::TextSize;
use lsp_types::{
    InlayHint, InlayHintKind, InlayHintLabel, InlayHintTooltip, MarkupContent, MarkupKind,
    Position, Range, TextEdit,
};

use crate::snapshot::Snapshot;

pub fn inlay_hints(snapshot: &Snapshot) -> Vec<InlayHint> {
    let Some(infer) = snapshot.infer.as_ref() else {
        return vec![];
    };
    let mut hints = Vec::new();
    collect_from_stmts(&snapshot.program.statements, snapshot, infer, &mut hints);
    hints
}

/// Build a type inlay hint at `position` labelled `: <ty_text>`.
///
/// The tooltip and the "insert this annotation" text edit are *not* filled in
/// here — they ride lazily on `inlayHint/resolve` (see [`resolve`]), so the
/// initial `textDocument/inlayHint` response stays small even for a file with
/// hundreds of hints. The data needed to reconstruct them later (the rendered
/// type, and whether the site can take an inline annotation) is stashed in
/// `data`. `editable` is true for `let`/parameter hints (where `: T` can be
/// written in source) and false for destructuring-pattern bindings.
fn type_hint(position: Position, ty_text: String, editable: bool) -> InlayHint {
    let data = serde_json::json!({ "type": ty_text, "editable": editable });
    InlayHint {
        position,
        label: InlayHintLabel::String(format!(": {ty_text}")),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: None,
        data: Some(data),
    }
}

/// `inlayHint/resolve` — fill in a hint's tooltip and (for editable hints) the
/// text edit that turns the inferred type into an explicit annotation.
///
/// Stateless: everything needed is already on the hint — the rendered type and
/// the `editable` flag ride in `data`, and the edit is inserted at the hint's
/// own `position`. A hint that already has a tooltip (or carries no `data`) is
/// returned unchanged, so re-resolving is idempotent.
pub fn resolve(mut hint: InlayHint) -> InlayHint {
    if hint.tooltip.is_some() {
        return hint;
    }
    let data = hint.data.as_ref();
    let Some(ty_text) = data
        .and_then(|d| d.get("type"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return hint;
    };
    let editable = data
        .and_then(|d| d.get("editable"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let note = if editable {
        "Inferred type — accept the hint to insert this annotation."
    } else {
        "Inferred type."
    };
    hint.tooltip = Some(InlayHintTooltip::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: format!("```flux\n: {ty_text}\n```\n\n{note}"),
    }));
    if editable {
        hint.text_edits = Some(vec![TextEdit {
            range: Range {
                start: hint.position,
                end: hint.position,
            },
            new_text: format!(": {ty_text}"),
        }]);
    }
    hint
}

fn collect_from_stmts(
    stmts: &[Statement],
    snapshot: &Snapshot,
    infer: &flux::ast::type_infer::InferProgramResult,
    out: &mut Vec<InlayHint>,
) {
    for stmt in stmts {
        match stmt {
            Statement::Let {
                name,
                value,
                type_annotation,
                span,
                is_public,
                ..
            } => {
                if type_annotation.is_none()
                    && let Some(ty) = infer.expr_types.get(&value.expr_id())
                {
                    let ty_text = display_infer_type(ty, &snapshot.interner);
                    let name_text = snapshot.interner.try_resolve(*name).unwrap_or("");
                    let keyword_len = if *is_public {
                        "public let ".len()
                    } else {
                        "let ".len()
                    };
                    let name_end = FluxPosition {
                        line: span.start.line,
                        column: span.start.column + keyword_len + name_text.len(),
                    };
                    let position = snapshot.position_map.flux_to_lsp(name_end);
                    out.push(type_hint(position, ty_text, true));
                }
            }
            Statement::Function {
                span,
                parameters,
                parameter_types,
                body,
                ..
            } => {
                collect_from_block(body, snapshot, infer, out);

                let key = (
                    span.start.line,
                    span.start.column,
                    span.end.line,
                    span.end.column,
                );
                if let Some(scheme) = infer.resolved_binding_schemes_by_span.get(&key)
                    && let InferType::Fun(param_infer_types, _, _) = &scheme.infer_type
                {
                    for (idx, (param, annotated)) in
                        parameters.iter().zip(parameter_types.iter()).enumerate()
                    {
                        if annotated.is_some() {
                            continue;
                        }
                        let Some(param_ty) = param_infer_types.get(idx) else {
                            continue;
                        };
                        let ty_text = display_infer_type(param_ty, &snapshot.interner);
                        let Some(position) = find_param_hint_position(snapshot, *span, *param)
                        else {
                            continue;
                        };
                        out.push(type_hint(position, ty_text, true));
                    }
                }
            }
            Statement::Module { body, .. } => {
                collect_from_block(body, snapshot, infer, out);
            }
            Statement::LetDestructure { pattern, .. } => {
                collect_pattern_hints(pattern, snapshot, infer, out);
            }
            _ => {}
        }
    }
}

fn collect_from_block(
    block: &Block,
    snapshot: &Snapshot,
    infer: &flux::ast::type_infer::InferProgramResult,
    out: &mut Vec<InlayHint>,
) {
    collect_from_stmts(&block.statements, snapshot, infer, out);
}

fn collect_pattern_hints(
    pat: &Pattern,
    snapshot: &Snapshot,
    infer: &flux::ast::type_infer::InferProgramResult,
    out: &mut Vec<InlayHint>,
) {
    match pat {
        Pattern::Identifier { name, span } => {
            if let Some(scheme) = infer.resolved_binding_schemes.get(name) {
                let ty_text = render_scheme_canonical(&snapshot.interner, scheme);
                let name_text = snapshot.interner.try_resolve(*name).unwrap_or("");
                let name_end = FluxPosition {
                    line: span.start.line,
                    column: span.start.column + name_text.len(),
                };
                let position = snapshot.position_map.flux_to_lsp(name_end);
                // A binding inside a destructuring pattern (`let (a, b) = …`)
                // can't carry an inline `: T` annotation, so the hint is
                // tooltip-only — not editable.
                out.push(type_hint(position, ty_text, false));
            }
        }
        Pattern::Tuple { elements, .. } => {
            for e in elements {
                collect_pattern_hints(e, snapshot, infer, out);
            }
        }
        Pattern::Constructor { fields, .. } => {
            for f in fields {
                collect_pattern_hints(f, snapshot, infer, out);
            }
        }
        Pattern::NamedConstructor { fields, .. } => {
            for f in fields {
                if let Some(p) = &f.pattern {
                    collect_pattern_hints(p, snapshot, infer, out);
                }
            }
        }
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => {
            collect_pattern_hints(pattern, snapshot, infer, out);
        }
        _ => {}
    }
}

/// Find the LSP position just after the end of a parameter name in the
/// function's source text. Scans from the opening `(` to the first `)`,
/// locating `param_name` as a whole word.
fn find_param_hint_position(
    snapshot: &Snapshot,
    fn_span: FluxSpan,
    param: Identifier,
) -> Option<lsp_types::Position> {
    let param_name = snapshot.interner.try_resolve(param)?;
    let fn_start_offset = snapshot
        .position_map
        .flux_to_offset(fn_span.start)
        .map(usize::from)?;
    let text = snapshot.text.as_ref();
    let after_fn = text.get(fn_start_offset..)?;
    let paren_pos = after_fn.find('(')?;
    let close_paren = after_fn[paren_pos..]
        .find(')')
        .unwrap_or(after_fn.len() - paren_pos);
    let params_region = after_fn.get(paren_pos..paren_pos + close_paren)?;
    let rel = find_word_end(params_region, param_name)?;
    let byte_offset = fn_start_offset + paren_pos + rel;
    let ts = TextSize::try_from(byte_offset).ok()?;
    Some(snapshot.position_map.offset_to_lsp(ts))
}

/// Returns the byte offset just past the end of `word` when found as a whole
/// word (not part of a longer identifier) in `text`. Returns `None` if not found.
fn find_word_end(text: &str, word: &str) -> Option<usize> {
    let mut start = 0;
    while let Some(pos) = text[start..].find(word) {
        let abs = start + pos;
        let before_ok =
            abs == 0 || !text[..abs].ends_with(|c: char| c.is_alphanumeric() || c == '_');
        let after_end = abs + word.len();
        let after_ok = after_end >= text.len()
            || !text[after_end..].starts_with(|c: char| c.is_alphanumeric() || c == '_');
        if before_ok && after_ok {
            return Some(after_end);
        }
        start = abs + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_editable_hint_adds_tooltip_and_annotation_edit() {
        let pos = Position {
            line: 0,
            character: 5,
        };
        let resolved = resolve(type_hint(pos, "Int".to_string(), true));

        match resolved.tooltip {
            Some(InlayHintTooltip::MarkupContent(m)) => {
                assert!(m.value.contains(": Int"));
                assert!(m.value.contains("accept the hint"));
            }
            other => panic!("expected a markdown tooltip, got {other:?}"),
        }
        let edits = resolved.text_edits.expect("an annotation text edit");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, ": Int");
        // The annotation is inserted at the hint's own position (zero-width).
        assert_eq!(edits[0].range.start, pos);
        assert_eq!(edits[0].range.end, pos);
    }

    #[test]
    fn resolve_non_editable_hint_has_tooltip_but_no_edit() {
        let pos = Position {
            line: 1,
            character: 3,
        };
        let resolved = resolve(type_hint(pos, "String".to_string(), false));

        assert!(resolved.tooltip.is_some(), "tooltip is always filled in");
        assert!(
            resolved.text_edits.is_none(),
            "a destructuring-pattern hint offers no insert-annotation edit"
        );
    }

    #[test]
    fn resolve_is_idempotent() {
        let pos = Position {
            line: 0,
            character: 0,
        };
        let once = resolve(type_hint(pos, "Bool".to_string(), true));
        let twice = resolve(once.clone());
        // `InlayHintTooltip` has no `PartialEq`; compare the markdown bodies.
        assert_eq!(tooltip_value(&once), tooltip_value(&twice));
        assert_eq!(once.text_edits, twice.text_edits);
    }

    fn tooltip_value(hint: &InlayHint) -> Option<String> {
        match hint.tooltip.as_ref()? {
            InlayHintTooltip::MarkupContent(m) => Some(m.value.clone()),
            InlayHintTooltip::String(s) => Some(s.clone()),
        }
    }
}
