//! Call hierarchy (`textDocument/prepareCallHierarchy`,
//! `callHierarchy/incomingCalls`, `callHierarchy/outgoingCalls`).
//!
//! The cursor's function becomes a [`CallHierarchyItem`]; from there the editor
//! navigates **incoming** calls (who calls this function) and **outgoing** calls
//! (what this function calls). All three requests share one extraction pass:
//! [`function_decls`] walks a program into the named function declarations it
//! contains, each carrying the call sites in its body. Calls inside an anonymous
//! lambda are attributed to the nearest enclosing *named* function, the same way
//! rust-analyzer folds closure bodies into their host.
//!
//! Resolution is by **name within the module-graph component** — the same scope
//! find-references uses — so a call resolves to a declaration in a sibling
//! module, but only to functions the workspace actually declares (calls to
//! builtins/stdlib that have no in-scope `fn` are not listed as outgoing edges).
//! Names are matched as strings because a [`CallHierarchyItem`] round-trips
//! through the client between the prepare and incoming/outgoing requests, and an
//! interned id is not stable across snapshot rebuilds.

use std::collections::HashMap;
use std::sync::Arc;

use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::expression::Expression;
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position, Range,
    SymbolKind, Uri,
};

use crate::handlers::references::node_identifier;
use crate::locator::{decl_name_start, find_at};
use crate::snapshot::Snapshot;
use crate::vfs::FileId;
use crate::workspace::Workspace;

/// One file in a call-hierarchy search — owned, `Send` data the worker thread
/// walks without a `Workspace` borrow.
pub struct ScopeFile {
    pub uri: Uri,
    pub snapshot: Arc<Snapshot>,
}

/// The main-thread "gather" result: the function name in question plus the
/// component scope to search. Handed to the (off-thread) compute step.
pub struct CallHierarchyBundle {
    /// The function name the request is about.
    pub target: String,
    /// Owned snapshots for every file in the component scope.
    pub files: Vec<ScopeFile>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Main-thread gather
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve the identifier under the cursor and collect the component scope —
/// the basis for a `prepareCallHierarchy` response. `None` when the cursor is
/// not on a name.
pub fn prepare_gather(
    workspace: &mut Workspace,
    file: FileId,
    position: Position,
) -> Option<CallHierarchyBundle> {
    // The `workspace` borrow ends with this block so `scope_files` can re-borrow
    // `&mut` to lazily build closed-member snapshots.
    let target = {
        let snapshot = workspace.ensure_snapshot(file)?;
        let pos = snapshot.position_map.lsp_to_flux(position)?;
        let node = find_at(&snapshot.program, &snapshot.interner, pos)?;
        let id = node_identifier(&node)?;
        snapshot.interner.try_resolve(id)?.to_string()
    };
    let files = scope_files(workspace, file);
    Some(CallHierarchyBundle { target, files })
}

/// Collect the component scope for an incoming/outgoing request, keyed off the
/// item the client round-tripped back. `None` when the item's file is unknown.
pub fn item_gather(
    workspace: &mut Workspace,
    item: &CallHierarchyItem,
) -> Option<CallHierarchyBundle> {
    let file = workspace.file_id(&item.uri)?;
    let files = scope_files(workspace, file);
    Some(CallHierarchyBundle {
        target: item.name.clone(),
        files,
    })
}

/// Owned snapshot + uri for every file in `file`'s module-graph component,
/// building those not yet analyzed so a closed sibling is still searched.
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

/// Every function declaration named `bundle.target` across the scope, as call
/// hierarchy items — the `prepareCallHierarchy` result.
pub fn prepare_items(bundle: &CallHierarchyBundle) -> Vec<CallHierarchyItem> {
    let mut items = Vec::new();
    for f in &bundle.files {
        for decl in function_decls(&f.snapshot.program, &f.snapshot.interner) {
            if decl.name == bundle.target {
                items.push(make_item(&f.uri, &f.snapshot, &decl));
            }
        }
    }
    items
}

/// Callers of `bundle.target`: every function in scope whose body calls it,
/// each with the call-site ranges within that caller.
pub fn incoming_calls(bundle: &CallHierarchyBundle) -> Vec<CallHierarchyIncomingCall> {
    let mut out = Vec::new();
    for f in &bundle.files {
        for decl in function_decls(&f.snapshot.program, &f.snapshot.interner) {
            let from_ranges: Vec<Range> = decl
                .calls
                .iter()
                .filter(|c| c.callee == bundle.target)
                .map(|c| f.snapshot.position_map.flux_span_to_range(c.span))
                .collect();
            if !from_ranges.is_empty() {
                out.push(CallHierarchyIncomingCall {
                    from: make_item(&f.uri, &f.snapshot, &decl),
                    from_ranges,
                });
            }
        }
    }
    out.sort_by(|a, b| a.from.name.cmp(&b.from.name));
    out
}

/// Callees of the function the `item` denotes: each in-scope function it calls,
/// with the call-site ranges within the caller's own file.
pub fn outgoing_calls(
    bundle: &CallHierarchyBundle,
    item: &CallHierarchyItem,
) -> Vec<CallHierarchyOutgoingCall> {
    // The caller's body lives in the item's own file; pin the exact declaration
    // by name + selection range (two same-named functions in one file are
    // disambiguated by where their name sits).
    let Some(f) = bundle.files.iter().find(|f| f.uri == item.uri) else {
        return vec![];
    };
    let decls = function_decls(&f.snapshot.program, &f.snapshot.interner);
    let Some(decl) = decls
        .iter()
        .find(|d| {
            d.name == bundle.target
                && f.snapshot.position_map.flux_span_to_range(d.focus_span) == item.selection_range
        })
        .or_else(|| decls.iter().find(|d| d.name == bundle.target))
    else {
        return vec![];
    };

    // Group the caller's call sites by callee name.
    let mut grouped: HashMap<&str, Vec<Range>> = HashMap::new();
    for call in &decl.calls {
        grouped
            .entry(call.callee.as_str())
            .or_default()
            .push(f.snapshot.position_map.flux_span_to_range(call.span));
    }

    let items = decl_items_by_name(bundle);
    let mut out = Vec::new();
    for (callee, from_ranges) in grouped {
        if let Some(to) = items.get(callee) {
            out.push(CallHierarchyOutgoingCall {
                to: to.clone(),
                from_ranges,
            });
        }
    }
    out.sort_by(|a, b| a.to.name.cmp(&b.to.name));
    out
}

/// First call hierarchy item for each function name across the scope — the
/// resolution table outgoing calls map a callee name through.
fn decl_items_by_name(bundle: &CallHierarchyBundle) -> HashMap<String, CallHierarchyItem> {
    let mut map = HashMap::new();
    for f in &bundle.files {
        for decl in function_decls(&f.snapshot.program, &f.snapshot.interner) {
            map.entry(decl.name.clone())
                .or_insert_with(|| make_item(&f.uri, &f.snapshot, &decl));
        }
    }
    map
}

fn make_item(uri: &Uri, snapshot: &Snapshot, decl: &FnDecl) -> CallHierarchyItem {
    CallHierarchyItem {
        name: decl.name.clone(),
        kind: SymbolKind::FUNCTION,
        tags: None,
        detail: decl.detail.clone(),
        uri: uri.clone(),
        range: snapshot.position_map.flux_span_to_range(decl.full_span),
        selection_range: snapshot.position_map.flux_span_to_range(decl.focus_span),
        data: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Declaration + call-site extraction
// ─────────────────────────────────────────────────────────────────────────────

/// A named function declaration and the call sites inside its body.
struct FnDecl {
    name: String,
    /// Parameter list (`(x, y)`) shown as the item's detail.
    detail: Option<String>,
    /// Whole `fn …` declaration extent — the item's `range`.
    full_span: FluxSpan,
    /// The name identifier alone — the item's `selection_range`.
    focus_span: FluxSpan,
    calls: Vec<CallSite>,
}

struct CallSite {
    /// Resolved name of the called function (`foo`, or `foo` for `M.foo`).
    callee: String,
    /// Span of the callee at the call site, for `from_ranges`.
    span: FluxSpan,
}

/// Walk a program into its named function declarations, each carrying the calls
/// made directly in its body (calls in nested lambdas fold into it; nested
/// `fn`s become their own declarations).
fn function_decls(program: &Program, interner: &Interner) -> Vec<FnDecl> {
    let mut out = Vec::new();
    for stmt in &program.statements {
        collect_decls_stmt(stmt, interner, None, &mut out);
    }
    out
}

/// `current` is the index in `out` of the function whose body we are inside,
/// or `None` at module/top level — call sites are attributed to it.
fn collect_decls_stmt(
    stmt: &Statement,
    interner: &Interner,
    current: Option<usize>,
    out: &mut Vec<FnDecl>,
) {
    match stmt {
        Statement::Function {
            is_public,
            name,
            parameters,
            body,
            span,
            ..
        } => {
            let body_owner = match interner.try_resolve(*name) {
                Some(name_str) => {
                    let focus_start = decl_name_start(span.start, *is_public, "fn");
                    let focus_span = FluxSpan {
                        start: focus_start,
                        end: FluxPosition {
                            line: focus_start.line,
                            column: focus_start.column + name_str.len(),
                        },
                    };
                    let params: Vec<&str> = parameters
                        .iter()
                        .filter_map(|p| interner.try_resolve(*p))
                        .collect();
                    let idx = out.len();
                    out.push(FnDecl {
                        name: name_str.to_string(),
                        detail: Some(format!("({})", params.join(", "))),
                        full_span: *span,
                        focus_span,
                        calls: Vec::new(),
                    });
                    Some(idx)
                }
                // Unresolvable name — keep attributing to the parent rather than
                // dropping the body's calls.
                None => current,
            };
            for s in &body.statements {
                collect_decls_stmt(s, interner, body_owner, out);
            }
        }
        Statement::Module { body, .. } => {
            for s in &body.statements {
                collect_decls_stmt(s, interner, current, out);
            }
        }
        Statement::Let { value, .. }
        | Statement::Assign { value, .. }
        | Statement::LetDestructure { value, .. } => {
            collect_calls_expr(value, interner, current, out)
        }
        Statement::Return { value: Some(v), .. } => collect_calls_expr(v, interner, current, out),
        Statement::Expression { expression, .. } => {
            collect_calls_expr(expression, interner, current, out)
        }
        _ => {}
    }
}

fn collect_calls_expr(
    expr: &Expression,
    interner: &Interner,
    current: Option<usize>,
    out: &mut Vec<FnDecl>,
) {
    match expr {
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            if let Some(idx) = current
                && let Some((callee, span)) = callee_name(function, interner)
            {
                out[idx].calls.push(CallSite { callee, span });
            }
            collect_calls_expr(function, interner, current, out);
            for a in arguments {
                collect_calls_expr(a, interner, current, out);
            }
        }
        // A lambda body folds into the enclosing named function.
        Expression::Function { body, .. } | Expression::DoBlock { block: body, .. } => {
            for s in &body.statements {
                collect_decls_stmt(s, interner, current, out);
            }
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            collect_calls_expr(condition, interner, current, out);
            for s in &consequence.statements {
                collect_decls_stmt(s, interner, current, out);
            }
            if let Some(alt) = alternative {
                for s in &alt.statements {
                    collect_decls_stmt(s, interner, current, out);
                }
            }
        }
        Expression::Prefix { right, .. } => collect_calls_expr(right, interner, current, out),
        Expression::Infix { left, right, .. } => {
            collect_calls_expr(left, interner, current, out);
            collect_calls_expr(right, interner, current, out);
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            collect_calls_expr(scrutinee, interner, current, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_calls_expr(g, interner, current, out);
                }
                collect_calls_expr(&arm.body, interner, current, out);
            }
        }
        Expression::MemberAccess { object, .. } | Expression::TupleFieldAccess { object, .. } => {
            collect_calls_expr(object, interner, current, out)
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for e in elements {
                collect_calls_expr(e, interner, current, out);
            }
        }
        Expression::Index { left, index, .. } => {
            collect_calls_expr(left, interner, current, out);
            collect_calls_expr(index, interner, current, out);
        }
        Expression::Hash { pairs, .. } => {
            for (k, v) in pairs {
                collect_calls_expr(k, interner, current, out);
                collect_calls_expr(v, interner, current, out);
            }
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => collect_calls_expr(value, interner, current, out),
        Expression::Cons { head, tail, .. } => {
            collect_calls_expr(head, interner, current, out);
            collect_calls_expr(tail, interner, current, out);
        }
        Expression::Perform { args, .. } => {
            for a in args {
                collect_calls_expr(a, interner, current, out);
            }
        }
        Expression::Handle {
            expr,
            parameter,
            arms,
            ..
        } => {
            collect_calls_expr(expr, interner, current, out);
            if let Some(p) = parameter {
                collect_calls_expr(p, interner, current, out);
            }
            for arm in arms {
                collect_calls_expr(&arm.body, interner, current, out);
            }
        }
        Expression::Sealing { expr, .. } => collect_calls_expr(expr, interner, current, out),
        Expression::NamedConstructor { fields, .. } => {
            for field in fields {
                if let Some(v) = &field.value {
                    collect_calls_expr(v, interner, current, out);
                }
            }
        }
        Expression::Spread {
            base, overrides, ..
        } => {
            collect_calls_expr(base, interner, current, out);
            for field in overrides {
                if let Some(v) = &field.value {
                    collect_calls_expr(v, interner, current, out);
                }
            }
        }
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let flux::syntax::expression::StringPart::Interpolation(e) = part {
                    collect_calls_expr(e, interner, current, out);
                }
            }
        }
        _ => {}
    }
}

/// The called function's name and the span to highlight at the call site —
/// a direct call (`foo(..)`) or a qualified one (`M.foo(..)`). `None` for a
/// computed callee (`getFn()(..)`); the inner call is still recorded by the
/// surrounding walk.
fn callee_name(func: &Expression, interner: &Interner) -> Option<(String, FluxSpan)> {
    match func {
        Expression::Identifier { name, span, .. } => {
            interner.try_resolve(*name).map(|s| (s.to_string(), *span))
        }
        Expression::MemberAccess { member, span, .. } => interner
            .try_resolve(*member)
            .map(|s| (s.to_string(), *span)),
        _ => None,
    }
}
