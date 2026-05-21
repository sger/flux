//! `textDocument/codeAction` — quick fixes derived from the snapshot's
//! diagnostics.
//!
//! Every action is anchored on a diagnostic whose range overlaps the
//! requested range. Three families of fix are produced:
//!
//! - **apply suggestion** — a diagnostic carrying a structured
//!   `InlineSuggestion` (its own span + replacement text), e.g. a
//!   misspelled keyword the parser recovered from;
//! - **did-you-mean** — a diagnostic whose hint text reads ``Did you mean
//!   `X`?`` for a single-token `X`; the flagged span is replaced with `X`;
//! - **add catch-all arm** — a non-exhaustive `match` (`E015`); a
//!   `_ -> ()` arm is inserted before the closing brace.
//!
//! Edits are computed purely from the diagnostic and the source text, so
//! the handler is a pure function safe to run on the worker thread.

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionResponse,
    Diagnostic as LspDiagnostic, DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, Range, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

use flux::diagnostics::Diagnostic as FluxDiagnostic;
use flux::diagnostics::position::Span as FluxSpan;

use crate::convert::diagnostic_to_lsp;
use crate::snapshot::Snapshot;

/// Build the quick-fix list for `range`. Walks every snapshot diagnostic,
/// keeps the ones whose range overlaps the request, and turns each into
/// zero or more `CodeAction`s, then appends diagnostic-independent
/// auto-import fixes. `workspace_modules` (every `module` name the workspace
/// declares, from the cached symbol index) lets the auto-import pass discover
/// not-yet-imported sibling modules.
pub fn code_actions(
    snapshot: &Snapshot,
    uri: &Uri,
    range: Range,
    workspace_modules: &[String],
    only: Option<&[CodeActionKind]>,
) -> CodeActionResponse {
    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    for diag in &snapshot.diagnostics {
        let Some(span) = diag.span() else {
            continue;
        };
        let diag_range = snapshot.position_map.flux_span_to_range(span);
        if !ranges_overlap(diag_range, range) {
            continue;
        }
        let lsp_diag = diagnostic_to_lsp(diag, &snapshot.position_map);
        suggestion_actions(snapshot, uri, &lsp_diag, diag, &mut actions);
        did_you_mean_actions(uri, &lsp_diag, diag, diag_range, &mut actions);
        if diag.code() == Some("E015")
            && let Some(action) = catchall_arm_action(snapshot, uri, &lsp_diag, span)
        {
            actions.push(action);
        }
    }
    // Diagnostic-independent: offer an `import` when the cursor is on a
    // module-qualified path whose module isn't imported yet.
    super::auto_import::import_actions(snapshot, uri, range, workspace_modules, &mut actions);
    // Source action: only when the client asks for `source.organizeImports`.
    if super::organize_imports::organize_imports_requested(only)
        && let Some(action) = super::organize_imports::organize_imports_action(snapshot, uri)
    {
        actions.push(action);
    }
    actions
}

// ─────────────────────────────────────────────────────────────────────────────
// Fix families
// ─────────────────────────────────────────────────────────────────────────────

/// Surface every structured `InlineSuggestion` carried by the diagnostic as
/// a quick fix. Each suggestion already names its own span and replacement.
fn suggestion_actions(
    snapshot: &Snapshot,
    uri: &Uri,
    lsp_diag: &LspDiagnostic,
    diag: &FluxDiagnostic,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for suggestion in diag.suggestions() {
        let range = snapshot.position_map.flux_span_to_range(suggestion.span);
        let title = suggestion
            .message
            .clone()
            .unwrap_or_else(|| format!("Replace with `{}`", suggestion.replacement));
        out.push(quick_fix(
            title,
            uri,
            vec![TextEdit {
                range,
                new_text: suggestion.replacement.clone(),
            }],
            lsp_diag,
        ));
    }
}

/// For a diagnostic whose hint text reads ``Did you mean `X`?``, offer
/// replacing the flagged span with `X`. Only single-token `X` is accepted —
/// a hint like ``Did you mean `with ..., e`?`` carries prose, not a
/// drop-in replacement, and is skipped.
fn did_you_mean_actions(
    uri: &Uri,
    lsp_diag: &LspDiagnostic,
    diag: &FluxDiagnostic,
    diag_range: Range,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for hint in diag.hints() {
        let Some(name) = parse_did_you_mean(&hint.text) else {
            continue;
        };
        out.push(quick_fix(
            format!("Change to `{name}`"),
            uri,
            vec![TextEdit {
                range: diag_range,
                new_text: name,
            }],
            lsp_diag,
        ));
    }
}

/// For a non-exhaustive `match` (`E015`), insert a `_ -> ()` catch-all arm
/// just before the closing brace. The edit is computed textually from the
/// diagnostic span (which covers the whole `match … { … }` expression):
/// find the closing brace, back up over trailing whitespace to the last arm
/// content, add a leading `,` only when the previous arm is not already
/// comma-terminated, and indent the new arm one level past the brace.
fn catchall_arm_action(
    snapshot: &Snapshot,
    uri: &Uri,
    lsp_diag: &LspDiagnostic,
    span: FluxSpan,
) -> Option<CodeActionOrCommand> {
    let text = snapshot.text.as_ref();
    let bytes = text.as_bytes();
    let start: usize = snapshot.position_map.flux_to_offset(span.start)?.into();
    if start >= bytes.len() {
        return None;
    }
    // Closing brace of the match block: open at the first `{` from the
    // match keyword, then brace-depth-count to its mate. Record literals in
    // arm bodies are themselves balanced, so depth counting still lands on
    // the block's own closing brace.
    let open = start + bytes[start..].iter().position(|&b| b == b'{')?;
    let mut depth = 0i32;
    let mut brace = None;
    for (offset, &byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    brace = Some(offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let brace = brace?;
    // Last non-whitespace byte before the brace — the end of the final arm.
    let mut insert = brace;
    while insert > start && bytes[insert - 1].is_ascii_whitespace() {
        insert -= 1;
    }
    if insert == start {
        return None;
    }
    let prev = bytes[insert - 1];
    // `,` → already terminated; `{` → empty arm list. Either way no comma.
    let terminated = prev == b',' || prev == b'{';

    // Indentation: leading whitespace of the line the brace sits on.
    let line_start = bytes[..brace]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    let brace_indent: String = bytes[line_start..brace]
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .map(|&b| b as char)
        .collect();
    let arm_indent = format!("{brace_indent}    ");
    let new_text = if terminated {
        format!("\n{arm_indent}_ -> ()")
    } else {
        format!(",\n{arm_indent}_ -> ()")
    };

    let pos = snapshot
        .position_map
        .offset_to_lsp(u32::try_from(insert).ok()?.into());
    Some(quick_fix(
        "Add catch-all `_` arm".to_string(),
        uri,
        vec![TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text,
        }],
        lsp_diag,
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract the single-token suggestion from a ``Did you mean `X`?`` hint.
/// Returns `None` when the hint has no such phrase or the captured text is
/// empty / contains whitespace (i.e. it is prose, not a token).
fn parse_did_you_mean(hint: &str) -> Option<String> {
    let after = hint.split_once("Did you mean `")?.1;
    let token = after.split_once('`')?.0;
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return None;
    }
    Some(token.to_string())
}

/// Two LSP ranges overlap when neither ends strictly before the other
/// starts. A zero-width request range (a bare cursor) still matches a
/// diagnostic it sits inside.
fn ranges_overlap(a: Range, b: Range) -> bool {
    !(position_lt(a.end, b.start) || position_lt(b.end, a.start))
}

fn position_lt(a: Position, b: Position) -> bool {
    (a.line, a.character) < (b.line, b.character)
}

/// Build a single-file `QuickFix` code action as a `CodeActionOrCommand`.
fn quick_fix(
    title: String,
    uri: &Uri,
    edits: Vec<TextEdit>,
    diag: &LspDiagnostic,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(quick_fix_action(title, uri, edits, diag))
}

/// Build a single-file `QuickFix` [`CodeAction`].
///
/// The edit is expressed via `document_changes` rather than the simpler
/// `changes` map: `WorkspaceEdit::changes` is keyed by `Uri`, and `Uri` has
/// interior mutability, so a `HashMap<Uri, _>` trips `clippy::mutable_key_type`
/// — `rename.rs` builds its edits the same way for the same reason. The edit
/// is unversioned (`version: None`); the buffer the action was requested for
/// has not changed between request and apply.
fn quick_fix_action(
    title: String,
    uri: &Uri,
    edits: Vec<TextEdit>,
    diag: &LspDiagnostic,
) -> CodeAction {
    let edit = WorkspaceEdit {
        document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: None,
            },
            edits: edits.into_iter().map(OneOf::Left).collect(),
        }])),
        ..Default::default()
    };
    CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(edit),
        ..Default::default()
    }
}
