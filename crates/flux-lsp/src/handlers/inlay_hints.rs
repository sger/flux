use flux::ast::type_infer::{display_infer_type, render_scheme_canonical};
use flux::diagnostics::position::Position as FluxPosition;
use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::expression::Pattern;
use flux::syntax::statement::Statement;
use flux::types::infer_type::InferType;
use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel};
use line_index::TextSize;

use crate::snapshot::Snapshot;

pub fn inlay_hints(snapshot: &Snapshot) -> Vec<InlayHint> {
    let Some(infer) = snapshot.infer.as_ref() else {
        return vec![];
    };
    let mut hints = Vec::new();
    collect_from_stmts(&snapshot.program.statements, snapshot, infer, &mut hints);
    hints
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
                if type_annotation.is_none() {
                    if let Some(ty) = infer.expr_types.get(&value.expr_id()) {
                        let label = format!(": {}", display_infer_type(ty, &snapshot.interner));
                        let name_text =
                            snapshot.interner.try_resolve(*name).unwrap_or("");
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
                        out.push(InlayHint {
                            position,
                            label: InlayHintLabel::String(label),
                            kind: Some(InlayHintKind::TYPE),
                            text_edits: None,
                            tooltip: None,
                            padding_left: None,
                            padding_right: None,
                            data: None,
                        });
                    }
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
                if let Some(scheme) = infer.resolved_binding_schemes_by_span.get(&key) {
                    if let InferType::Fun(param_infer_types, _, _) = &scheme.infer_type {
                        for (idx, (param, annotated)) in
                            parameters.iter().zip(parameter_types.iter()).enumerate()
                        {
                            if annotated.is_some() {
                                continue;
                            }
                            let Some(param_ty) = param_infer_types.get(idx) else {
                                continue;
                            };
                            let label =
                                format!(": {}", display_infer_type(param_ty, &snapshot.interner));
                            let Some(position) =
                                find_param_hint_position(snapshot, *span, *param)
                            else {
                                continue;
                            };
                            out.push(InlayHint {
                                position,
                                label: InlayHintLabel::String(label),
                                kind: Some(InlayHintKind::TYPE),
                                text_edits: None,
                                tooltip: None,
                                padding_left: None,
                                padding_right: None,
                                data: None,
                            });
                        }
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
                let label = format!(": {}", render_scheme_canonical(&snapshot.interner, scheme));
                let name_text = snapshot.interner.try_resolve(*name).unwrap_or("");
                let name_end = FluxPosition {
                    line: span.start.line,
                    column: span.start.column + name_text.len(),
                };
                let position = snapshot.position_map.flux_to_lsp(name_end);
                out.push(InlayHint {
                    position,
                    label: InlayHintLabel::String(label),
                    kind: Some(InlayHintKind::TYPE),
                    text_edits: None,
                    tooltip: None,
                    padding_left: None,
                    padding_right: None,
                    data: None,
                });
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
    let close_paren = after_fn[paren_pos..].find(')').unwrap_or(after_fn.len() - paren_pos);
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
