//! "Make imports explicit / Refine import" — the Flux analogue of the Haskell
//! LSP's explicit-imports plugin. Rewrites an `import Flow.List exposing (..)`
//! into the members actually used unqualified (`exposing (filter, map)`), and
//! trims an already-explicit `exposing (a, b, c)` down to its used subset.
//!
//! Offered as both a code action (cursor on the import) and a code lens (above
//! every refinable import). Complements organize-imports and remove-unused-import.

use std::collections::HashSet;

use flux::ast::{Visitor, walk_expr};
use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::expression::Expression;
use flux::syntax::interner::Interner;
use flux::syntax::statement::{ImportExposing, Statement};
use line_index::TextSize;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeLens, Command, DocumentChanges, OneOf,
    OptionalVersionedTextDocumentIdentifier, Range, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};
use serde_json::{Value, json};

use super::code_action::ranges_overlap;
use crate::snapshot::Snapshot;

/// A computed rewrite of one import's `exposing` clause.
struct ExposingRewrite {
    /// LSP range covering the `( … )` after `exposing` (what we replace).
    clause_range: Range,
    /// Replacement clause text, e.g. `(filter, map)`.
    new_text: String,
    /// Short module name (e.g. `List`), for the action/lens title.
    module: String,
    /// `true` when the source was `exposing (..)` (expand) rather than an
    /// explicit list (refine).
    was_wildcard: bool,
}

/// Collect every identifier used in *unqualified* expression position. Mirrors
/// the `free_vars` traversal: a bare `Expression::Identifier` is recorded, while
/// a `MemberAccess` member (the `map` in `Array.map`) reaches the no-op
/// `visit_identifier` and is excluded. Binding sites and the import statements
/// themselves aren't expressions, so they never count. Over-approximates (it
/// ignores shadowing) — which is safe, since we only ever keep names that are
/// genuinely exposed, so a redundant entry is valid but a needed one is never
/// dropped.
struct UseCollector {
    used: HashSet<Identifier>,
}

impl<'ast> Visitor<'ast> for UseCollector {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Identifier { name, .. } = expr {
            self.used.insert(*name);
        } else {
            walk_expr(self, expr);
        }
    }
}

fn collect_unqualified_use_names(snapshot: &Snapshot) -> HashSet<String> {
    let mut collector = UseCollector {
        used: HashSet::new(),
    };
    collector.visit_program(&snapshot.program);
    collector
        .used
        .iter()
        .filter_map(|id| snapshot.interner.try_resolve(*id).map(str::to_string))
        .collect()
}

/// Compute the rewrite for one import statement, given the buffer's
/// unqualified-use set (computed once by the caller).
fn compute_rewrite(
    snapshot: &Snapshot,
    stmt: &Statement,
    used_names: &HashSet<String>,
) -> Option<ExposingRewrite> {
    let Statement::Import {
        name,
        exposing,
        span,
        ..
    } = stmt
    else {
        return None;
    };
    let module_full = snapshot.interner.try_resolve(*name)?;
    let short = module_full.rsplit('.').next().unwrap_or(module_full);

    // The names currently brought in unqualified, and whether it's a wildcard.
    let (currently, was_wildcard): (Vec<String>, bool) = match exposing {
        // `(..)` — every public export. Read them from the module's parsed
        // program (precise: only its `public` declarations), not from
        // `module_members` (which is permissive for completion and includes
        // builtins like `print`). Skip when we don't have the module's source
        // (an unloaded external module) rather than guess.
        ImportExposing::All => (module_public_exports(snapshot, short)?, true),
        // Explicit list — the names are right here in the AST, no export data
        // needed.
        ImportExposing::Names(list) => (
            list.iter()
                .filter_map(|id| snapshot.interner.try_resolve(*id).map(str::to_string))
                .collect(),
            false,
        ),
        ImportExposing::None => return None,
    };

    let distinct: HashSet<&String> = currently.iter().collect();
    if distinct.is_empty() {
        return None;
    }

    let mut used: Vec<String> = distinct
        .iter()
        .filter(|n| used_names.contains(n.as_str()))
        .map(|n| (*n).clone())
        .collect();
    used.sort();

    // Nothing used unqualified → that's remove-unused / drop-the-clause
    // territory, out of scope here.
    if used.is_empty() {
        return None;
    }
    // Refining an explicit list only when it strictly trims; expanding `(..)`
    // is always a change (wildcard → explicit list).
    if !was_wildcard && used.len() == distinct.len() {
        return None;
    }

    Some(ExposingRewrite {
        clause_range: exposing_clause_range(snapshot, *span)?,
        new_text: format!("({})", used.join(", ")),
        module: short.to_string(),
        was_wildcard,
    })
}

/// The public export names of a loaded module, read from its parsed program
/// (`snapshot.module_programs`). This is what `exposing (..)` actually brings in —
/// the module's `public fn` / `public let` / `public data` (with its variants) —
/// and is precise, unlike `module_members` which is permissive for completion.
fn module_public_exports(snapshot: &Snapshot, short: &str) -> Option<Vec<String>> {
    let (program, _, _) = snapshot.module_programs.get(short)?;
    let mut names = Vec::new();
    collect_public_exports(&program.statements, &snapshot.interner, &mut names);
    Some(names)
}

fn collect_public_exports(stmts: &[Statement], interner: &Interner, out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Statement::Function {
                is_public: true,
                name,
                ..
            }
            | Statement::Let {
                is_public: true,
                name,
                ..
            } => push_name(*name, interner, out),
            Statement::Data {
                is_public: true,
                name,
                variants,
                ..
            } => {
                push_name(*name, interner, out);
                for variant in variants {
                    push_name(variant.name, interner, out);
                }
            }
            // Recurse into `module Flow.X { … }` blocks (the stdlib wraps its
            // exports this way); not into function bodies (locals aren't exports).
            Statement::Module { body, .. } => {
                collect_public_exports(&body.statements, interner, out);
            }
            _ => {}
        }
    }
}

fn push_name(id: Identifier, interner: &Interner, out: &mut Vec<String>) {
    if let Some(name) = interner.try_resolve(id) {
        out.push(name.to_string());
    }
}

/// The LSP range covering the parenthesized clause after `exposing` in the
/// import at `span`. Only the whole-statement span exists in the AST, so we scan
/// the statement's own source text — `exposing` is a keyword (never an
/// identifier) and `except` uses `[...]`, so the first `(`…`)` after `exposing`
/// is the exposing list. Replacing just `(…)` preserves the rest of the line.
fn exposing_clause_range(snapshot: &Snapshot, span: FluxSpan) -> Option<Range> {
    let start = usize::from(snapshot.position_map.flux_to_offset(span.start)?);
    let end = usize::from(snapshot.position_map.flux_to_offset(span.end)?);
    let text = snapshot.text.get(start..end)?;
    let kw = text.find("exposing")?;
    let after = &text[kw..];
    let open_rel = after.find('(')?;
    let close_rel = after[open_rel..].find(')')? + open_rel;
    let open_abs = start + kw + open_rel;
    let close_abs = start + kw + close_rel;
    let start_pos = snapshot
        .position_map
        .offset_to_lsp(TextSize::try_from(open_abs).ok()?);
    let end_pos = snapshot
        .position_map
        .offset_to_lsp(TextSize::try_from(close_abs + 1).ok()?);
    Some(Range {
        start: start_pos,
        end: end_pos,
    })
}

/// Cursor-driven code action: for each import overlapping `range`, offer the
/// exposing-clause rewrite.
pub fn actions(snapshot: &Snapshot, uri: &Uri, range: Range, out: &mut Vec<CodeActionOrCommand>) {
    let used_names = collect_unqualified_use_names(snapshot);
    for stmt in &snapshot.program.statements {
        let Statement::Import { span, .. } = stmt else {
            continue;
        };
        if !ranges_overlap(snapshot.position_map.flux_span_to_range(*span), range) {
            continue;
        }
        let Some(rewrite) = compute_rewrite(snapshot, stmt, &used_names) else {
            continue;
        };
        out.push(rewrite_action(uri, &rewrite));
    }
}

/// File-wide code lenses: a lens above every refinable import.
pub fn lenses(snapshot: &Snapshot, uri_arg: &Value, out: &mut Vec<CodeLens>) {
    let used_names = collect_unqualified_use_names(snapshot);
    for stmt in &snapshot.program.statements {
        let Statement::Import { span, .. } = stmt else {
            continue;
        };
        let Some(rewrite) = compute_rewrite(snapshot, stmt, &used_names) else {
            continue;
        };
        let title = if rewrite.was_wildcard {
            "▶ Make explicit"
        } else {
            "▶ Refine imports"
        };
        out.push(CodeLens {
            range: snapshot.position_map.flux_span_to_range(*span),
            command: Some(Command {
                title: title.to_string(),
                command: "flux.makeImportsExplicit".to_string(),
                // The extension applies `edit.replace(clause_range, new_text)`.
                arguments: Some(vec![
                    uri_arg.clone(),
                    json!(rewrite.clause_range),
                    json!(rewrite.new_text),
                ]),
            }),
            data: None,
        });
    }
}

fn rewrite_action(uri: &Uri, rewrite: &ExposingRewrite) -> CodeActionOrCommand {
    let title = if rewrite.was_wildcard {
        format!("Make `{}` import explicit", rewrite.module)
    } else {
        format!("Refine `{}` exposing list", rewrite.module)
    };
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        edit: Some(WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: None,
                },
                edits: vec![OneOf::Left(TextEdit {
                    range: rewrite.clause_range,
                    new_text: rewrite.new_text.clone(),
                })],
            }])),
            ..Default::default()
        }),
        ..Default::default()
    })
}
