//! `textDocument/implementation` — from a `class` to its `instance` blocks.
//!
//! rust-analyzer's "Go to Implementations" on a trait jumps to its `impl`s;
//! the Flux analogue jumps from a `class` to every `instance` of it. The
//! cursor may sit on the `class` declaration name or on any reference to the
//! class (e.g. the `Show` in `instance Show<Int>`).

use lsp_types::{GotoDefinitionResponse, Location, Position, Uri};

use flux::syntax::statement::Statement;

use crate::handlers::references::node_identifier;
use crate::locator::find_at;
use crate::snapshot::Snapshot;

/// Locations of every `instance` block implementing the class under the
/// cursor. `None` when the cursor is not on a class name, or the class has
/// no instances in this file.
pub fn goto_implementation(
    snapshot: &Snapshot,
    uri: &Uri,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;
    let class_id = node_identifier(&node)?;

    // The locator emits `DataName` for `data`, `class`, and `instance`
    // names alike — only proceed when the identifier actually names a
    // `class` declared in this file.
    let names_a_class = snapshot
        .program
        .statements
        .iter()
        .any(|stmt| matches!(stmt, Statement::Class { name, .. } if *name == class_id));
    if !names_a_class {
        return None;
    }

    let locations: Vec<Location> = snapshot
        .program
        .statements
        .iter()
        .filter_map(|stmt| match stmt {
            Statement::Instance {
                class_name, span, ..
            } if *class_name == class_id => Some(Location {
                uri: uri.clone(),
                range: snapshot.position_map.flux_span_to_range(*span),
            }),
            _ => None,
        })
        .collect();

    (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
}
