//! Auto-import quick fixes (rust-analyzer-style).
//!
//! Unlike the other [`super::code_action`] families, these are *not* anchored
//! on a diagnostic — the LSP runs only parse + inference and never flags an
//! unresolved module path. Instead the fix is offered on demand: when the
//! cursor sits on a module-qualified path (`List.reverse`,
//! `Modules.Math.square`) whose module prefix isn't bound by any `import` in
//! the buffer, but *is* a module known to the workspace (a Flow stdlib module
//! or a sibling user module), surface a code action that inserts the missing
//! `import`.
//!
//! The two reference styles Flux supports drive the two import shapes offered:
//!
//! - **aliased short name** — the user wrote `List.reverse`, a single-segment
//!   prefix. A bare `import Flow.List` would bind the *full* path
//!   `Flow.List.reverse`, not `List`, so to match what was typed the fix
//!   inserts `import Flow.List as List`;
//! - **full path** — the user wrote `Modules.Math.square`, a multi-segment
//!   prefix that is itself a module's declared name. The unaliased
//!   `import Modules.Math` binds exactly that path, so that is what is
//!   inserted.
//!
//! Detection is conservative: the action fires only when the prefix names a
//! known-but-unbound module, and modules are uppercase by convention while
//! locals are lowercase, so an ordinary record access (`alice.name`) never
//! matches.

use std::collections::HashSet;
use std::sync::Arc;

use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::block::Block;
use flux::syntax::expression::Expression;
use flux::syntax::interner::Interner;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, DocumentChanges, OneOf,
    OptionalVersionedTextDocumentIdentifier, Position, Range, TextDocumentEdit, TextEdit, Uri,
    WorkspaceEdit,
};

use crate::locator::position_in_span;
use crate::snapshot::Snapshot;

/// Append auto-import quick fixes for the qualified path under `range.start`
/// to `out`. A no-op when the cursor is not on a module-qualified path, or
/// the path's module prefix is already imported / defined in this buffer.
///
/// `workspace_files` is the `(uri, text)` of every file the workspace knows.
/// The snapshot's own `module_programs` already covers the Flow stdlib (always
/// indexed) and any *imported* sibling module, but a not-yet-imported sibling
/// is invisible there — the module graph only follows existing imports. So
/// `workspace_files` is scanned for module declarations to surface those too.
/// The scan is lazy: it runs only after a qualified path is found under the
/// cursor, so the common "no path here" case pays nothing.
pub fn import_actions(
    snapshot: &Snapshot,
    uri: &Uri,
    range: Range,
    workspace_files: &[(Uri, Arc<str>)],
    out: &mut Vec<CodeActionOrCommand>,
) {
    let Some(target) = snapshot.position_map.lsp_to_flux(range.start) else {
        return;
    };
    let Some(path) = qualified_path_at(&snapshot.program, &snapshot.interner, target) else {
        return;
    };

    // Names already bound in the buffer that a fix must not shadow: import
    // aliases / unaliased full names, plus every buffer-local declaration.
    let (bound, imported_full) = imported_modules(snapshot);
    let buffer_defined = buffer_defined_names(snapshot);
    // Every module the workspace knows about, by its declared full name —
    // the Flow stdlib + imported siblings (from the snapshot) merged with a
    // scan of every workspace file (for not-yet-imported siblings).
    let mut known = known_module_full_names(snapshot);
    known.extend(workspace_module_names(workspace_files));
    known.sort();
    known.dedup();

    // Try the path's prefixes longest-first (`A.B.C`, `A.B`, `A`) and stop at
    // the first that yields a fix — so `Modules.Math.square` offers
    // `import Modules.Math`, never `import Modules`.
    let segments: Vec<&str> = path.split('.').collect();
    for take in (1..=segments.len()).rev() {
        let prefix = segments[..take].join(".");
        if bound.contains(&prefix) || buffer_defined.contains(&prefix) {
            // Already reachable under this name — nothing to import.
            return;
        }
        let mut bodies: Vec<String> = Vec::new();

        // Full-path import: the prefix is itself a known module's full name.
        if known.contains(&prefix) && !imported_full.contains(&prefix) {
            bodies.push(prefix.clone());
        }

        // Aliased short import: a single-segment prefix that is the final
        // segment of one or more known modules. `import <full> as <prefix>`
        // binds exactly the name that was typed.
        if take == 1 {
            for full in &known {
                if full == &prefix || imported_full.contains(full) {
                    continue;
                }
                if last_segment(full) == prefix {
                    bodies.push(format!("{full} as {prefix}"));
                }
            }
        }

        if bodies.is_empty() {
            continue;
        }
        bodies.sort();
        bodies.dedup();
        let insertion = import_insertion(snapshot);
        for body in bodies {
            out.push(import_action(uri, &body, insertion));
        }
        return;
    }
}

/// The `additionalTextEdits` entry that makes `ref_name` resolve as a module
/// by importing `full_name` — used by completion so accepting an un-imported
/// module name inserts its `import` in the same step. `None` when `ref_name`
/// is already bound by an import or a buffer declaration (nothing to add).
///
/// Mirrors [`import_actions`]' binding rules: when the typed name equals the
/// module's full declared name an unaliased `import` binds it; otherwise an
/// `as`-alias binds the typed name (e.g. `import Flow.Array as Array`).
pub(crate) fn import_edit_for(
    snapshot: &Snapshot,
    full_name: &str,
    ref_name: &str,
) -> Option<TextEdit> {
    let (bound, _imported_full) = imported_modules(snapshot);
    if bound.contains(ref_name) || buffer_defined_names(snapshot).contains(ref_name) {
        return None;
    }
    let body = if full_name == ref_name {
        format!("import {full_name}")
    } else {
        format!("import {full_name} as {ref_name}")
    };
    let insertion = import_insertion(snapshot);
    let new_text = if insertion.after_existing {
        format!("\n{body}")
    } else {
        format!("{body}\n")
    };
    Some(TextEdit {
        range: Range {
            start: insertion.position,
            end: insertion.position,
        },
        new_text,
    })
}

/// Where a new `import` line should go and the text framing it: after the
/// last existing import (prefixed with a newline), or at the very top of the
/// file (suffixed with a newline) when there are no imports yet.
#[derive(Clone, Copy)]
struct Insertion {
    position: Position,
    /// `true` → emit `\nimport …` (append after an existing import line);
    /// `false` → emit `import …\n` (prepend at file top).
    after_existing: bool,
}

fn import_insertion(snapshot: &Snapshot) -> Insertion {
    let mut last_import: Option<FluxSpan> = None;
    for stmt in &snapshot.program.statements {
        if let Statement::Import { span, .. } = stmt {
            last_import = Some(*span);
        }
    }
    match last_import {
        Some(span) => Insertion {
            position: snapshot.position_map.flux_span_to_range(span).end,
            after_existing: true,
        },
        None => Insertion {
            position: Position {
                line: 0,
                character: 0,
            },
            after_existing: false,
        },
    }
}

/// Build the single-edit `QuickFix` code action that inserts `import {body}`.
/// Unlike [`super::code_action`] fixes there is no associated diagnostic, so
/// `diagnostics` is left `None`. The edit uses `document_changes` for the same
/// reason `code_action.rs` does — a `HashMap<Uri, _>` trips
/// `clippy::mutable_key_type`.
fn import_action(uri: &Uri, body: &str, insertion: Insertion) -> CodeActionOrCommand {
    let new_text = if insertion.after_existing {
        format!("\nimport {body}")
    } else {
        format!("import {body}\n")
    };
    let edit = WorkspaceEdit {
        document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: None,
            },
            edits: vec![OneOf::Left(TextEdit {
                range: Range {
                    start: insertion.position,
                    end: insertion.position,
                },
                new_text,
            })],
        }])),
        ..Default::default()
    };
    CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Import `{body}`"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(edit),
        ..Default::default()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Buffer / workspace facts
// ─────────────────────────────────────────────────────────────────────────────

/// `(bound, imported_full)` where `bound` is every module name reachable
/// unqualified-as-a-prefix in this buffer — an import's alias, or its full
/// qualified name when unaliased — and `imported_full` is the qualified name
/// of every import regardless of alias.
fn imported_modules(snapshot: &Snapshot) -> (HashSet<String>, HashSet<String>) {
    let mut bound = HashSet::new();
    let mut imported_full = HashSet::new();
    for stmt in &snapshot.program.statements {
        if let Statement::Import { name, alias, .. } = stmt
            && let Some(qualified) = snapshot.interner.try_resolve(*name)
        {
            imported_full.insert(qualified.to_string());
            match alias.and_then(|a| snapshot.interner.try_resolve(a)) {
                Some(alias_text) => {
                    bound.insert(alias_text.to_string());
                }
                None => {
                    bound.insert(qualified.to_string());
                }
            }
        }
    }
    (bound, imported_full)
}

/// Top-level declaration names defined in the buffer itself — values,
/// types, modules — so a path whose prefix is locally defined never offers
/// an import. Module declarations contribute both their full name and final
/// segment.
fn buffer_defined_names(snapshot: &Snapshot) -> HashSet<String> {
    let interner = &snapshot.interner;
    let mut names = HashSet::new();
    let add = |names: &mut HashSet<String>, id| {
        if let Some(text) = interner.try_resolve(id) {
            names.insert(text.to_string());
        }
    };
    for stmt in &snapshot.program.statements {
        match stmt {
            Statement::Function { name, .. }
            | Statement::Let { name, .. }
            | Statement::EffectDecl { name, .. }
            | Statement::EffectAlias { name, .. }
            | Statement::Class { name, .. } => add(&mut names, *name),
            Statement::Data { name, variants, .. } => {
                add(&mut names, *name);
                for v in variants {
                    add(&mut names, v.name);
                }
            }
            Statement::TypeAlias(alias) => add(&mut names, alias.name),
            Statement::Module { name, .. } => {
                add(&mut names, *name);
                if let Some(text) = interner.try_resolve(*name) {
                    names.insert(last_segment(text).to_string());
                }
            }
            _ => {}
        }
    }
    names
}

/// Every module known to this session, by declared full name (`Flow.List`,
/// `Modules.Math`, …) — read from the `module` declaration inside each cached
/// module program. These are the import candidates.
fn known_module_full_names(snapshot: &Snapshot) -> Vec<String> {
    let mut names = Vec::new();
    for (program, _, _) in snapshot.module_programs.values() {
        for stmt in &program.statements {
            if let Statement::Module { name, .. } = stmt
                && let Some(text) = snapshot.interner.try_resolve(*name)
            {
                names.push(text.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Top-level `module` declaration names across every workspace file. Parsed
/// standalone (a throwaway interner per file — only the module-name strings
/// are kept), this is what surfaces a sibling module that the edited buffer
/// has not imported yet, so its `module_graph` node never loaded.
fn workspace_module_names(files: &[(Uri, Arc<str>)]) -> Vec<String> {
    let mut names = Vec::new();
    for (_, text) in files {
        let lexer = Lexer::new(text.to_string());
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();
        for stmt in &program.statements {
            if let Statement::Module { name, .. } = stmt
                && let Some(text) = interner.try_resolve(*name)
            {
                names.push(text.to_string());
            }
        }
    }
    names
}

fn last_segment(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

// ─────────────────────────────────────────────────────────────────────────────
// Cursor → qualified path
// ─────────────────────────────────────────────────────────────────────────────

/// The widest pure module-qualified path (`A`, `A.B`, `A.B.C`) whose span
/// contains `target`. "Pure" means built only from identifiers and member
/// accesses — a call, index, or literal anywhere in the chain disqualifies it,
/// so `Modules.Math.square(5)` yields `Modules.Math.square` (the call's
/// function) while the call itself is skipped. Widest wins so the cursor may
/// sit on any segment.
fn qualified_path_at(
    program: &Program,
    interner: &Interner,
    target: FluxPosition,
) -> Option<String> {
    let mut best: Option<(String, u64)> = None;
    for stmt in &program.statements {
        walk_stmt(stmt, interner, target, &mut best);
    }
    best.map(|(path, _)| path)
}

fn consider(
    expr: &Expression,
    interner: &Interner,
    target: FluxPosition,
    best: &mut Option<(String, u64)>,
) {
    if !position_in_span(target, expr.span()) {
        return;
    }
    if let Some(path) = module_path_string(expr, interner) {
        let size = span_size(expr.span());
        if best.as_ref().is_none_or(|(_, b)| size > *b) {
            *best = Some((path, size));
        }
    }
}

/// Flatten a pure `A.B.C` chain to its dotted string; `None` for anything
/// else. Mirrors the helper in [`super::definition`].
fn module_path_string(expr: &Expression, interner: &Interner) -> Option<String> {
    match expr {
        Expression::Identifier { name, .. } => interner.try_resolve(*name).map(str::to_string),
        Expression::MemberAccess { object, member, .. } => {
            let base = module_path_string(object, interner)?;
            let segment = interner.try_resolve(*member)?;
            Some(format!("{base}.{segment}"))
        }
        _ => None,
    }
}

fn span_size(span: FluxSpan) -> u64 {
    let lines = span.end.line.saturating_sub(span.start.line) as u64;
    lines
        .saturating_mul(1_000_000)
        .saturating_add(span.end.column as u64)
}

fn walk_block(
    block: &Block,
    interner: &Interner,
    target: FluxPosition,
    best: &mut Option<(String, u64)>,
) {
    for stmt in &block.statements {
        walk_stmt(stmt, interner, target, best);
    }
}

fn walk_stmt(
    stmt: &Statement,
    interner: &Interner,
    target: FluxPosition,
    best: &mut Option<(String, u64)>,
) {
    match stmt {
        Statement::Let { value, .. }
        | Statement::Assign { value, .. }
        | Statement::LetDestructure { value, .. } => walk_expr(value, interner, target, best),
        Statement::Return { value: Some(v), .. } => walk_expr(v, interner, target, best),
        Statement::Expression { expression, .. } => walk_expr(expression, interner, target, best),
        Statement::Function { body, .. } | Statement::Module { body, .. } => {
            walk_block(body, interner, target, best)
        }
        _ => {}
    }
}

fn walk_expr(
    expr: &Expression,
    interner: &Interner,
    target: FluxPosition,
    best: &mut Option<(String, u64)>,
) {
    consider(expr, interner, target, best);
    match expr {
        Expression::Prefix { right, .. } => walk_expr(right, interner, target, best),
        Expression::Infix { left, right, .. } => {
            walk_expr(left, interner, target, best);
            walk_expr(right, interner, target, best);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            walk_expr(condition, interner, target, best);
            walk_block(consequence, interner, target, best);
            if let Some(alt) = alternative {
                walk_block(alt, interner, target, best);
            }
        }
        Expression::DoBlock { block, .. } | Expression::Function { body: block, .. } => {
            walk_block(block, interner, target, best)
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            walk_expr(function, interner, target, best);
            for a in arguments {
                walk_expr(a, interner, target, best);
            }
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for e in elements {
                walk_expr(e, interner, target, best);
            }
        }
        Expression::Index { left, index, .. } => {
            walk_expr(left, interner, target, best);
            walk_expr(index, interner, target, best);
        }
        Expression::Hash { pairs, .. } => {
            for (k, v) in pairs {
                walk_expr(k, interner, target, best);
                walk_expr(v, interner, target, best);
            }
        }
        Expression::MemberAccess { object, .. } => walk_expr(object, interner, target, best),
        Expression::TupleFieldAccess { object, .. } => walk_expr(object, interner, target, best),
        Expression::Match {
            scrutinee, arms, ..
        } => {
            walk_expr(scrutinee, interner, target, best);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr(g, interner, target, best);
                }
                walk_expr(&arm.body, interner, target, best);
            }
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => walk_expr(value, interner, target, best),
        Expression::Cons { head, tail, .. } => {
            walk_expr(head, interner, target, best);
            walk_expr(tail, interner, target, best);
        }
        Expression::Perform { args, .. } => {
            for a in args {
                walk_expr(a, interner, target, best);
            }
        }
        Expression::Handle {
            expr,
            parameter,
            arms,
            ..
        } => {
            walk_expr(expr, interner, target, best);
            if let Some(p) = parameter {
                walk_expr(p, interner, target, best);
            }
            for arm in arms {
                walk_expr(&arm.body, interner, target, best);
            }
        }
        Expression::Sealing { expr, .. } => walk_expr(expr, interner, target, best),
        Expression::NamedConstructor { fields, .. } => {
            for field in fields {
                if let Some(v) = &field.value {
                    walk_expr(v, interner, target, best);
                }
            }
        }
        Expression::Spread {
            base, overrides, ..
        } => {
            walk_expr(base, interner, target, best);
            for field in overrides {
                if let Some(v) = &field.value {
                    walk_expr(v, interner, target, best);
                }
            }
        }
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let flux::syntax::expression::StringPart::Interpolation(e) = part {
                    walk_expr(e, interner, target, best);
                }
            }
        }
        _ => {}
    }
}
