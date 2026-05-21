//! `textDocument/linkedEditingRange` — the same-file occurrences of the
//! identifier under the cursor, so the editor can edit them in lockstep (type
//! once, every occurrence updates) without invoking a full rename.
//!
//! Like `documentHighlight`, this is current-file only: it resolves the
//! cursor's identifier the same way and collects this file's occurrences by
//! interned id. A symbol with cross-file uses is therefore only linked *within*
//! this file — use Rename (F2) for a project-wide change.
//!
//! The protocol requires every returned range to hold identical text. The
//! reference collector records a *declaration*'s whole-statement span (e.g.
//! `let count = 1`), so each span is narrowed to the identifier name itself
//! (via [`name_range_in_span`]) before it is reported.

use lsp_types::{LinkedEditingRanges, Position, Range};

use crate::handlers::references::{collect_all_uses, name_range_in_span, node_identifier};
use crate::locator::find_at;
use crate::snapshot::Snapshot;

/// A Flux identifier: a letter or underscore, then letters/digits/underscores.
/// Constrains the linked edit so it ends once the typed text stops being a
/// valid identifier.
const IDENT_PATTERN: &str = "[A-Za-z_][A-Za-z0-9_]*";

/// Ranges of every occurrence of the identifier under `position` in this file,
/// or `None` when the cursor is not on a navigable identifier.
pub fn linked_editing_ranges(
    snapshot: &Snapshot,
    position: Position,
) -> Option<LinkedEditingRanges> {
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;
    let target_id = node_identifier(&node)?;
    let name = snapshot.interner.try_resolve(target_id)?;
    if name.is_empty() {
        return None;
    }

    let mut spans = Vec::new();
    collect_all_uses(&snapshot.program, target_id, &mut spans);

    // Narrow to the name so every range has identical text (the protocol
    // requirement); drop any span the name can't be located in.
    let mut ranges: Vec<Range> = spans
        .into_iter()
        .filter_map(|span| name_range_in_span(&snapshot.position_map, span, name))
        .collect();
    if ranges.is_empty() {
        return None;
    }
    // The protocol forbids overlapping/duplicate ranges; declarations and uses
    // can't overlap once narrowed, but a name could be recorded twice.
    ranges.sort_by_key(|r| (r.start.line, r.start.character));
    ranges.dedup();

    Some(LinkedEditingRanges {
        ranges,
        word_pattern: Some(IDENT_PATTERN.to_string()),
    })
}
