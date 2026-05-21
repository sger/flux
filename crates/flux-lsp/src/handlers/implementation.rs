//! `textDocument/implementation` — from a `class` to its `instance` blocks.
//!
//! rust-analyzer's "Go to Implementations" on a trait jumps to its `impl`s;
//! the Flux analogue jumps from a `class` to every `instance` of it. The
//! cursor may sit on the `class` declaration name or on any reference to the
//! class (e.g. the `Show` in `instance Show<Int>`).
//!
//! The search spans the cursor file's whole module-graph component, and looks
//! both at top-level statements and inside `module { … }` blocks — so an
//! instance declared in a sibling module (or wrapped in a module block in the
//! same file) is found, not just top-level instances in the current file. Like
//! find-references, this relies on the component sharing one interner, so the
//! class identifier resolved at the cursor matches across every member file.

use std::sync::Arc;

use lsp_types::{GotoDefinitionResponse, Location, Position, Uri};

use flux::syntax::Identifier;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;

use crate::handlers::references::node_identifier;
use crate::locator::find_at;
use crate::snapshot::Snapshot;
use crate::vfs::FileId;
use crate::workspace::Workspace;

/// One component-member file in an implementation search — owned, `Send` data
/// the worker walks without a `Workspace` borrow.
pub struct ImplFile {
    pub uri: Uri,
    pub snapshot: Arc<Snapshot>,
}

/// Main-thread "gather" result: the class identifier under the cursor plus the
/// component scope to search. Handed to the (off-thread) [`goto_implementation`].
pub struct ImplBundle {
    class_id: Identifier,
    files: Vec<ImplFile>,
}

/// Resolve the identifier under the cursor and collect the component scope.
/// `None` when the cursor is not on a name.
pub fn gather(workspace: &mut Workspace, file: FileId, position: Position) -> Option<ImplBundle> {
    // The `workspace` borrow ends with this block so the scope collection can
    // re-borrow `&mut` to lazily build closed-member snapshots.
    let class_id = {
        let snapshot = workspace.ensure_snapshot(file)?;
        let target = snapshot.position_map.lsp_to_flux(position)?;
        let node = find_at(&snapshot.program, &snapshot.interner, target)?;
        node_identifier(&node)?
    };

    let mut files = Vec::new();
    for fid in workspace.component_scope(file) {
        if let Some(snapshot) = workspace.ensure_snapshot(fid).cloned()
            && let Some(uri) = workspace.uri_of(fid)
        {
            files.push(ImplFile { uri, snapshot });
        }
    }
    Some(ImplBundle { class_id, files })
}

/// Locations of every `instance` block implementing the class under the cursor,
/// across the component. `None` when the identifier names no `class` in the
/// component, or that class has no instances.
pub fn goto_implementation(bundle: &ImplBundle) -> Option<GotoDefinitionResponse> {
    let class_id = bundle.class_id;

    // The locator emits `DataName` for `data`, `class`, and `instance` names
    // alike — only proceed when the identifier actually names a `class`
    // somewhere in the component.
    let names_a_class = bundle
        .files
        .iter()
        .any(|f| declares_class(&f.snapshot.program, class_id));
    if !names_a_class {
        return None;
    }

    let mut locations: Vec<Location> = Vec::new();
    for f in &bundle.files {
        collect_instances(&f.snapshot, &f.uri, class_id, &mut locations);
    }
    (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
}

/// Whether `program` declares a `class` named `class_id`, top-level or nested
/// one level inside a `module { … }` block.
fn declares_class(program: &Program, class_id: Identifier) -> bool {
    program.statements.iter().any(|stmt| match stmt {
        Statement::Class { name, .. } => *name == class_id,
        Statement::Module { body, .. } => body
            .statements
            .iter()
            .any(|s| matches!(s, Statement::Class { name, .. } if *name == class_id)),
        _ => false,
    })
}

/// Push a [`Location`] for every `instance` of `class_id` in `snapshot`'s
/// program — top-level and nested one level inside `module { … }` blocks.
fn collect_instances(
    snapshot: &Snapshot,
    uri: &Uri,
    class_id: Identifier,
    out: &mut Vec<Location>,
) {
    for stmt in &snapshot.program.statements {
        push_if_instance(stmt, snapshot, uri, class_id, out);
        if let Statement::Module { body, .. } = stmt {
            for inner in &body.statements {
                push_if_instance(inner, snapshot, uri, class_id, out);
            }
        }
    }
}

fn push_if_instance(
    stmt: &Statement,
    snapshot: &Snapshot,
    uri: &Uri,
    class_id: Identifier,
    out: &mut Vec<Location>,
) {
    if let Statement::Instance {
        class_name, span, ..
    } = stmt
        && *class_name == class_id
    {
        out.push(Location {
            uri: uri.clone(),
            range: snapshot.position_map.flux_span_to_range(*span),
        });
    }
}
