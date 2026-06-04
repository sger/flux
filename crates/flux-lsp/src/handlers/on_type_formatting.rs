//! `textDocument/onTypeFormatting` — re-indent the current line as you type.
//!
//! Two triggers: a newline indents the fresh line to the enclosing brace depth,
//! and `}` dedents its own line to line up with the opener. Indentation is
//! computed lexically (counting `{`/`}` tokens before the cursor, so braces in
//! strings/comments don't count), which stays robust while the buffer is
//! mid-edit and syntactically incomplete.
//!
//! VS Code already does this natively from `language-configuration.json`; the
//! handler computes the same indent, so it's a no-op there (an already-correct
//! line yields no edit) and brings the behaviour to LSP clients that have no
//! such config.

use flux::syntax::lexer::Lexer;
use flux::syntax::token_type::TokenType;
use line_index::TextSize;
use lsp_types::{Position, Range, TextEdit};

use crate::snapshot::Snapshot;

pub fn on_type_format(
    snapshot: &Snapshot,
    position: Position,
    ch: &str,
    tab_size: u32,
    insert_spaces: bool,
) -> Vec<TextEdit> {
    let depth = brace_depth_before(snapshot, position);
    let units = match ch {
        // The line opened by Enter sits inside whatever braces are still open at
        // the cursor — unless it leads with `}` (Enter pressed in `{|}`), which
        // aligns with the opener instead.
        "\n" => {
            if line_leads_with_close_brace(snapshot, position.line) {
                depth - 1
            } else {
                depth
            }
        }
        // The typed `}` already decremented `depth` (it sits before the cursor),
        // so it now matches its opener's level.
        "}" => depth,
        _ => return Vec::new(),
    };
    set_line_indent(
        snapshot,
        position.line,
        units.max(0) as u32,
        tab_size,
        insert_spaces,
    )
}

/// Net `{` minus `}` among the tokens that start before `position`. Clamped at
/// zero so an unbalanced buffer never produces negative indentation.
fn brace_depth_before(snapshot: &Snapshot, position: Position) -> i32 {
    let Some(cursor) = snapshot.position_map.lsp_to_flux(position) else {
        return 0;
    };
    let mut depth: i32 = 0;
    for tok in Lexer::new(snapshot.text.to_string()).tokenize() {
        let p = tok.position;
        if (p.line, p.column) >= (cursor.line, cursor.column) {
            break;
        }
        match tok.token_type {
            TokenType::LBrace => depth += 1,
            TokenType::RBrace => depth -= 1,
            _ => {}
        }
    }
    depth.max(0)
}

/// Whether line `line`'s first non-whitespace character is `}`.
fn line_leads_with_close_brace(snapshot: &Snapshot, line: u32) -> bool {
    let Some(start) = snapshot
        .position_map
        .lsp_to_offset(Position { line, character: 0 })
    else {
        return false;
    };
    let bytes = snapshot.text.as_bytes();
    let mut i = u32::from(start) as usize;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    bytes.get(i) == Some(&b'}')
}

/// Replace line `line`'s leading whitespace with `units` levels of indentation.
/// Returns no edit when the indent is already correct, so the handler never
/// fights an editor that has already indented the line.
fn set_line_indent(
    snapshot: &Snapshot,
    line: u32,
    units: u32,
    tab_size: u32,
    insert_spaces: bool,
) -> Vec<TextEdit> {
    let desired = if insert_spaces {
        " ".repeat((units * tab_size) as usize)
    } else {
        "\t".repeat(units as usize)
    };
    let line_start = Position { line, character: 0 };
    let Some(start_off) = snapshot.position_map.lsp_to_offset(line_start) else {
        return Vec::new();
    };
    let start = u32::from(start_off) as usize;
    let text = snapshot.text.as_ref();
    let bytes = text.as_bytes();
    let mut end = start;
    while end < bytes.len() && (bytes[end] == b' ' || bytes[end] == b'\t') {
        end += 1;
    }
    if text[start..end] == desired {
        return Vec::new();
    }
    let end_pos = snapshot
        .position_map
        .offset_to_lsp(TextSize::from(end as u32));
    vec![TextEdit {
        range: Range {
            start: line_start,
            end: end_pos,
        },
        new_text: desired,
    }]
}
