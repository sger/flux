//! "Convert number format" — the Flux analogue of the Haskell LSP's
//! alternate-number-format plugin. With the cursor on an integer literal, it
//! offers rewriting it between decimal, hexadecimal (`0x…`), binary (`0b…`) and
//! underscore-grouped decimal forms — every form but the one already written.
//!
//! The literal's value is the source of truth: the AST keeps only the parsed
//! `i64` (never the spelling), so we reformat from it and read the current
//! spelling out of the buffer to drop the no-op form. Integer literals only —
//! `0x…`/`0b…` floats aren't valid Flux.

use flux::syntax::expression::Expression;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, DocumentChanges, OneOf,
    OptionalVersionedTextDocumentIdentifier, Range, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

use crate::locator::{NodeRef, find_at};
use crate::snapshot::Snapshot;

/// Cursor-driven code action: if `range`'s start sits on an integer literal,
/// offer converting it to each alternate representation.
pub fn actions(snapshot: &Snapshot, uri: &Uri, range: Range, out: &mut Vec<CodeActionOrCommand>) {
    let Some(target) = snapshot.position_map.lsp_to_flux(range.start) else {
        return;
    };
    let Some(NodeRef::Expr(expr)) = find_at(&snapshot.program, &snapshot.interner, target) else {
        return;
    };
    let Expression::Integer { value, span, .. } = expr else {
        return;
    };
    let value = *value;
    let span = *span;

    // The literal as currently written, so we can skip the form already in use.
    let (Some(start), Some(end)) = (
        snapshot.position_map.flux_to_offset(span.start),
        snapshot.position_map.flux_to_offset(span.end),
    ) else {
        return;
    };
    let Some(current) = snapshot.text.get(usize::from(start)..usize::from(end)) else {
        return;
    };
    let lsp_range = snapshot.position_map.flux_span_to_range(span);

    for (label, new_text) in alternate_formats(value) {
        if new_text == current {
            continue;
        }
        out.push(convert_action(uri, label, &new_text, lsp_range));
    }
}

/// The representations offered for `value` (a non-negative integer literal — the
/// `-` of a negative number is a separate prefix expression). The caller filters
/// out whichever one matches the current spelling.
fn alternate_formats(value: i64) -> Vec<(&'static str, String)> {
    let decimal = value.to_string();
    let mut forms = vec![
        ("decimal", decimal.clone()),
        ("hexadecimal", format!("0x{value:X}")),
        ("binary", format!("0b{value:b}")),
    ];
    let grouped = group_decimal(&decimal);
    if grouped != decimal {
        forms.push(("decimal with separators", grouped));
    }
    forms
}

/// Group decimal `digits` into runs of three with `_` separators
/// (`1000` → `1_000`). Input is the plain `i64::to_string` of a non-negative
/// value, so it has no sign or separators of its own.
fn group_decimal(digits: &str) -> String {
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push('_');
        }
        out.push(ch);
    }
    out
}

fn convert_action(uri: &Uri, label: &str, new_text: &str, range: Range) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Convert to {label} (`{new_text}`)"),
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        edit: Some(WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: None,
                },
                edits: vec![OneOf::Left(TextEdit {
                    range,
                    new_text: new_text.to_string(),
                })],
            }])),
            ..Default::default()
        }),
        ..Default::default()
    })
}
