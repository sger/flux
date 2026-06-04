//! Type hierarchy (`textDocument/prepareTypeHierarchy`,
//! `typeHierarchy/supertypes`, `typeHierarchy/subtypes`).
//!
//! Flux's type-level hierarchy is its type-class graph. For a `class C`:
//!
//! - **supertypes** — `C`'s declared superclasses (`class Sup<a> => C<a> { … }`);
//! - **subtypes** — classes that name `C` as a superclass, plus every `instance`
//!   of `C` (the types that implement it).
//!
//! Like call hierarchy and go-to-implementation, resolution spans the cursor
//! file's module-graph component and looks inside `module { … }` blocks, and
//! matches by name because a [`TypeHierarchyItem`] round-trips through the client
//! between the prepare and supertypes/subtypes requests.

use std::sync::Arc;

use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use flux::syntax::type_expr::TypeExpr;
use lsp_types::{Position, SymbolKind, TypeHierarchyItem, Uri};

use crate::handlers::references::node_identifier;
use crate::locator::find_at;
use crate::snapshot::Snapshot;
use crate::vfs::FileId;
use crate::workspace::Workspace;

/// One component-member file in a type-hierarchy search — owned, `Send` data the
/// worker walks without a `Workspace` borrow.
pub struct ScopeFile {
    pub uri: Uri,
    pub snapshot: Arc<Snapshot>,
}

/// Main-thread "gather" result: the class name in question plus the component
/// scope to search.
pub struct TypeHierarchyBundle {
    target: String,
    files: Vec<ScopeFile>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Main-thread gather
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve the identifier under the cursor and collect the component scope.
/// `None` when the cursor is not on a name.
pub fn prepare_gather(
    workspace: &mut Workspace,
    file: FileId,
    position: Position,
) -> Option<TypeHierarchyBundle> {
    let target = {
        let snapshot = workspace.ensure_snapshot(file)?;
        let pos = snapshot.position_map.lsp_to_flux(position)?;
        let node = find_at(&snapshot.program, &snapshot.interner, pos)?;
        let id = node_identifier(&node)?;
        snapshot.interner.try_resolve(id)?.to_string()
    };
    Some(TypeHierarchyBundle {
        target,
        files: scope_files(workspace, file),
    })
}

/// Collect the component scope for a supertypes/subtypes request, keyed off the
/// item the client round-tripped back.
pub fn item_gather(
    workspace: &mut Workspace,
    item: &TypeHierarchyItem,
) -> Option<TypeHierarchyBundle> {
    let file = workspace.file_id(&item.uri)?;
    Some(TypeHierarchyBundle {
        target: item.name.clone(),
        files: scope_files(workspace, file),
    })
}

fn scope_files(workspace: &mut Workspace, file: FileId) -> Vec<ScopeFile> {
    let mut files = Vec::new();
    for fid in workspace.component_scope(file) {
        if let Some(snapshot) = workspace.ensure_snapshot(fid).cloned()
            && let Some(uri) = workspace.uri_of(fid)
        {
            files.push(ScopeFile { uri, snapshot });
        }
    }
    files
}

// ─────────────────────────────────────────────────────────────────────────────
// Off-thread compute
// ─────────────────────────────────────────────────────────────────────────────

/// The class declarations named `bundle.target` across the scope — the
/// `prepareTypeHierarchy` result. Only a `class` forms a type hierarchy.
pub fn prepare_items(bundle: &TypeHierarchyBundle) -> Vec<TypeHierarchyItem> {
    let mut out = Vec::new();
    for f in &bundle.files {
        for stmt in decls(&f.snapshot.program) {
            if let Statement::Class {
                name,
                span,
                name_span,
                ..
            } = stmt
                && f.snapshot.interner.try_resolve(*name) == Some(bundle.target.as_str())
            {
                out.push(class_item(
                    &f.uri,
                    &f.snapshot,
                    &bundle.target,
                    *span,
                    *name_span,
                ));
            }
        }
    }
    out
}

/// `bundle.target`'s declared superclasses, as class items.
pub fn supertypes(bundle: &TypeHierarchyBundle) -> Vec<TypeHierarchyItem> {
    let mut super_names: Vec<String> = Vec::new();
    for f in &bundle.files {
        for stmt in decls(&f.snapshot.program) {
            if let Statement::Class {
                name, superclasses, ..
            } = stmt
                && f.snapshot.interner.try_resolve(*name) == Some(bundle.target.as_str())
            {
                for constraint in superclasses {
                    if let Some(s) = f.snapshot.interner.try_resolve(constraint.class_name) {
                        super_names.push(s.to_string());
                    }
                }
            }
        }
    }
    super_names.sort();
    super_names.dedup();
    super_names
        .iter()
        .filter_map(|name| find_class_item(bundle, name))
        .collect()
}

/// `bundle.target`'s subtypes: classes that name it as a superclass, plus every
/// `instance` of it (the implementing types).
pub fn subtypes(bundle: &TypeHierarchyBundle) -> Vec<TypeHierarchyItem> {
    let mut out = Vec::new();
    for f in &bundle.files {
        for stmt in decls(&f.snapshot.program) {
            match stmt {
                // A subclass: lists the target among its superclasses.
                Statement::Class {
                    name,
                    superclasses,
                    span,
                    name_span,
                    ..
                } if superclasses.iter().any(|c| {
                    f.snapshot.interner.try_resolve(c.class_name) == Some(bundle.target.as_str())
                }) =>
                {
                    if let Some(nm) = f.snapshot.interner.try_resolve(*name) {
                        let nm = nm.to_string();
                        out.push(class_item(&f.uri, &f.snapshot, &nm, *span, *name_span));
                    }
                }
                // An instance: the type implementing the target class.
                Statement::Instance {
                    class_name,
                    type_args,
                    span,
                    name_span,
                    ..
                } if f.snapshot.interner.try_resolve(*class_name)
                    == Some(bundle.target.as_str()) =>
                {
                    out.push(instance_item(
                        &f.uri,
                        &f.snapshot,
                        &bundle.target,
                        type_args,
                        *span,
                        *name_span,
                    ));
                }
                _ => {}
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// First class item named `name` across the scope — how supertypes resolve a
/// superclass name to its declaration site.
fn find_class_item(bundle: &TypeHierarchyBundle, name: &str) -> Option<TypeHierarchyItem> {
    for f in &bundle.files {
        for stmt in decls(&f.snapshot.program) {
            if let Statement::Class {
                name: id,
                span,
                name_span,
                ..
            } = stmt
                && f.snapshot.interner.try_resolve(*id) == Some(name)
            {
                return Some(class_item(&f.uri, &f.snapshot, name, *span, *name_span));
            }
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Item construction + traversal
// ─────────────────────────────────────────────────────────────────────────────

fn class_item(
    uri: &Uri,
    snapshot: &Snapshot,
    name: &str,
    full: FluxSpan,
    focus: FluxSpan,
) -> TypeHierarchyItem {
    TypeHierarchyItem {
        name: name.to_string(),
        kind: SymbolKind::INTERFACE,
        tags: None,
        detail: Some("class".to_string()),
        uri: uri.clone(),
        range: snapshot.position_map.flux_span_to_range(full),
        selection_range: snapshot.position_map.flux_span_to_range(focus),
        data: None,
    }
}

fn instance_item(
    uri: &Uri,
    snapshot: &Snapshot,
    class: &str,
    type_args: &[TypeExpr],
    full: FluxSpan,
    focus: FluxSpan,
) -> TypeHierarchyItem {
    let head = type_args
        .first()
        .and_then(head_ctor)
        .and_then(|id| snapshot.interner.try_resolve(id).map(str::to_string));
    // The implementing type names the subtype; fall back to the class name.
    let name = head.clone().unwrap_or_else(|| class.to_string());
    let detail = match &head {
        Some(h) => Some(format!("instance {class}<{h}>")),
        None => Some(format!("instance {class}")),
    };
    TypeHierarchyItem {
        name,
        kind: SymbolKind::CLASS,
        tags: None,
        detail,
        uri: uri.clone(),
        range: snapshot.position_map.flux_span_to_range(full),
        // `focus` is the parsed head class name span — precise even with a
        // context constraint (`instance Eq<a> => Eq<List<a>>`).
        selection_range: snapshot.position_map.flux_span_to_range(focus),
        data: None,
    }
}

fn head_ctor(t: &TypeExpr) -> Option<Identifier> {
    match t {
        TypeExpr::Named { name, .. } => Some(*name),
        _ => None,
    }
}

/// Top-level statements plus those nested one level inside a `module { … }`
/// block — user-module classes/instances live in module blocks.
fn decls(program: &Program) -> Vec<&Statement> {
    let mut out = Vec::new();
    for stmt in &program.statements {
        out.push(stmt);
        if let Statement::Module { body, .. } = stmt {
            for inner in &body.statements {
                out.push(inner);
            }
        }
    }
    out
}
