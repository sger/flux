use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::expression::{Expression, Pattern};
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use lsp_types::{Location, Position, Uri};

use crate::line_index::PositionMap;
use crate::locator::{NodeRef, find_at};
use crate::snapshot::Snapshot;
use crate::symbol_index::SymbolIndex;

pub fn goto_definition(snapshot: &Snapshot, uri: &Uri, position: Position) -> Option<Location> {
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;

    // Cross-module: cursor on the member name of a module-qualified access
    // (e.g. `sqrt` in `Math.sqrt`). Look up the member in the prelude's cached
    // program for that module and jump to its definition there.
    if let NodeRef::MemberAccessMember { object, member, .. } = &node {
        if let Expression::Identifier { name: module_id, .. } = object {
            let module_name = snapshot.interner.try_resolve(*module_id)?;
            if let Some((mod_program, mod_source, mod_path)) =
                snapshot.module_programs.get(module_name)
            {
                let mod_index = SymbolIndex::build_extended(mod_program, &snapshot.interner);
                if let Some(entry) = mod_index.lookup_id(*member) {
                    let mod_map = PositionMap::new(
                        std::sync::Arc::from(mod_source.as_ref()),
                        snapshot.position_map.encoding(),
                    );
                    let module_uri = path_to_uri(mod_path)?;
                    return Some(Location {
                        uri: module_uri,
                        range: mod_map.flux_span_to_range(entry.span),
                    });
                }
            }
        }
    }

    let def_name = definition_name(&node)?;

    // Extended index covers top-level names + effect ops + data variants.
    let extended_index = SymbolIndex::build_extended(&snapshot.program, &snapshot.interner);
    if let Some(entry) = extended_index.lookup_id(def_name) {
        return Some(Location {
            uri: uri.clone(),
            range: snapshot.position_map.flux_span_to_range(entry.span),
        });
    }

    // Local binding walk (let/fn/parameter inside a function body).
    let local_span = find_local_definition(&snapshot.program, def_name)?;
    Some(Location {
        uri: uri.clone(),
        range: snapshot.position_map.flux_span_to_range(local_span),
    })
}

fn path_to_uri(path: &std::path::Path) -> Option<Uri> {
    let s = format!("file://{}", path.display());
    s.parse().ok()
}

fn find_local_definition(program: &Program, target: Identifier) -> Option<FluxSpan> {
    for stmt in &program.statements {
        if let Some(span) = find_in_stmt(stmt, target) {
            return Some(span);
        }
    }
    None
}

fn find_in_stmt(stmt: &Statement, target: Identifier) -> Option<FluxSpan> {
    match stmt {
        Statement::Let { name, span, .. } if *name == target => Some(*span),
        Statement::LetDestructure { pattern, span, .. } => {
            find_in_pattern(pattern, target, *span)
        }
        Statement::Function { name, span, parameters, body, .. } => {
            if *name == target {
                return Some(*span);
            }
            if parameters.iter().any(|p| *p == target) {
                return Some(*span);
            }
            find_in_block(body, target)
        }
        Statement::Module { body, .. } => find_in_block(body, target),
        _ => None,
    }
}

fn find_in_pattern(pat: &Pattern, target: Identifier, binding_span: FluxSpan) -> Option<FluxSpan> {
    match pat {
        Pattern::Identifier { name, .. } if *name == target => Some(binding_span),
        Pattern::Tuple { elements, .. } => {
            elements.iter().find_map(|e| find_in_pattern(e, target, binding_span))
        }
        Pattern::Constructor { fields, .. } => {
            fields.iter().find_map(|f| find_in_pattern(f, target, binding_span))
        }
        Pattern::NamedConstructor { fields, .. } => fields.iter().find_map(|f| {
            f.pattern.as_ref().and_then(|p| find_in_pattern(p, target, binding_span))
        }),
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => find_in_pattern(pattern, target, binding_span),
        Pattern::Cons { head, tail, .. } => find_in_pattern(head, target, binding_span)
            .or_else(|| find_in_pattern(tail, target, binding_span)),
        _ => None,
    }
}

fn find_in_block(block: &Block, target: Identifier) -> Option<FluxSpan> {
    for stmt in &block.statements {
        if let Some(span) = find_in_stmt(stmt, target) {
            return Some(span);
        }
    }
    None
}

/// Map a `NodeRef` to the identifier whose definition F12 should jump to.
/// Returns `None` for nodes that don't represent a navigable reference.
fn definition_name(node: &NodeRef) -> Option<Identifier> {
    match node {
        NodeRef::Expr(Expression::Identifier { name, .. }) => Some(*name),
        NodeRef::Pattern(Pattern::Identifier { name, .. }) => Some(*name),
        NodeRef::NamedConstructorName { name, .. } => Some(*name),
        NodeRef::TypeExprNamed { name, .. } => Some(*name),
        NodeRef::EffectName { name, .. } => Some(*name),
        // Effect operation references — jump to op declaration in effect block.
        NodeRef::PerformOpName { name, .. }
        | NodeRef::HandleArmOpName { name, .. }
        | NodeRef::EffectOpName { name, .. } => Some(*name),
        // Data variant references — jump to variant declaration in data block.
        NodeRef::DataVariantName { name, .. } => Some(*name),
        // Decl-site nodes: F12 stays on the same identifier (its own
        // definition is itself).
        NodeRef::DataName { name, .. }
        | NodeRef::EffectDeclName { name, .. }
        | NodeRef::DeclName { name, .. } => Some(*name),
        _ => None,
    }
}
