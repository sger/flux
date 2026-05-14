use flux::syntax::Identifier;
use flux::syntax::expression::Expression;
use lsp_types::{Location, Position, Uri};

use crate::locator::{NodeRef, find_at};
use crate::snapshot::Snapshot;

pub fn goto_definition(snapshot: &Snapshot, uri: &Uri, position: Position) -> Option<Location> {
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;
    let def_name = definition_name(&node)?;
    let entry = snapshot.symbol_index.lookup_id(def_name)?;
    Some(Location {
        uri: uri.clone(),
        range: snapshot.position_map.flux_span_to_range(entry.span),
    })
}

/// Map a `NodeRef` to the identifier whose definition F12 should jump to.
/// Returns `None` for nodes that don't represent a navigable reference.
fn definition_name(node: &NodeRef) -> Option<Identifier> {
    match node {
        NodeRef::Expr(Expression::Identifier { name, .. }) => Some(*name),
        NodeRef::NamedConstructorName { name, .. } => Some(*name),
        NodeRef::TypeExprNamed { name, .. } => Some(*name),
        NodeRef::EffectName { name, .. } => Some(*name),
        // Decl-site nodes: F12 stays on the same identifier (its own
        // definition is itself). Useful for round-tripping through the
        // symbol_index lookup, which preserves the binding span.
        NodeRef::DataName { name, .. }
        | NodeRef::EffectDeclName { name, .. }
        | NodeRef::DeclName { name, .. } => Some(*name),
        // Sub-positions handled in M4d (need parent-type lookup against
        // inference) — currently fall through.
        _ => None,
    }
}
