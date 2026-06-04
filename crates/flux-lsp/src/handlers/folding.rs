//! `textDocument/foldingRange` — structural code folding.
//!
//! Emits one folding range per multi-line declaration: top-level statements
//! and the statements nested one level inside a `module` block. Editors
//! already fold by indentation; this adds declaration-aware regions so a
//! whole `fn` / `data` / `module` collapses to its first line.

use flux::syntax::statement::Statement;
use lsp_types::FoldingRange;

use crate::snapshot::Snapshot;

/// Folding ranges for every multi-line declaration in the file.
pub fn folding_ranges(snapshot: &Snapshot) -> Vec<FoldingRange> {
    let mut out = Vec::new();
    for stmt in &snapshot.program.statements {
        push_fold(stmt, snapshot, &mut out);
        // One level into a `module` body so its members fold independently.
        if let Statement::Module { body, .. } = stmt {
            for inner in &body.statements {
                push_fold(inner, snapshot, &mut out);
            }
        }
    }
    out
}

/// Emit a folding range for `stmt` when it spans more than one line.
///
/// A function's statement span covers only its *signature* (the parser keeps
/// the body as a separate `Block`), so the fold is widened to the body block;
/// `data` / `class` / `effect` / `instance` statement spans already cover
/// their whole block.
fn push_fold(stmt: &Statement, snapshot: &Snapshot, out: &mut Vec<FoldingRange>) {
    let head = stmt.span();
    let tail = match stmt {
        Statement::Function { body, .. } | Statement::Module { body, .. } => body.span,
        _ => head,
    };
    let start = snapshot.position_map.flux_span_to_range(head).start;
    let end = snapshot.position_map.flux_span_to_range(tail).end;
    if end.line > start.line {
        out.push(FoldingRange {
            start_line: start.line,
            end_line: end.line,
            ..FoldingRange::default()
        });
    }
}
