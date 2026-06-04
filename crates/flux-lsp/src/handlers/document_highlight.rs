//! `textDocument/documentHighlight` — highlight occurrences related to the
//! cursor, scoped to the current file. A pure function over one `Snapshot`,
//! safe on the worker thread.
//!
//! Three modes, in priority order:
//!
//! 1. **exit points** — on `return`/`fn`, every `return` and the tail
//!    expression of the enclosing function;
//! 2. **effect op** — on a `perform`/`handle` operation, every `perform` site
//!    and matching `handle` arm of that `(effect, op)` pair;
//! 3. **identifier** — every read/write occurrence of the symbol under the
//!    cursor (the single-file sibling of find-references), tagged `READ` or
//!    `WRITE`.

use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::expression::Expression;
use flux::syntax::statement::Statement;
use lsp_types::{DocumentHighlight, DocumentHighlightKind, Position};

use crate::handlers::references::{
    UseKind, collect_kinded_uses, node_identifier, occurrence_range,
};
use crate::keywords::{is_offset_in_comment_or_string, word_at_offset};
use crate::locator::{NodeRef, find_at, position_in_span};
use crate::snapshot::Snapshot;

/// Highlights related to the cursor at `position` in `snapshot`'s file.
/// Empty when nothing relevant is under the cursor.
pub fn document_highlights(snapshot: &Snapshot, position: Position) -> Vec<DocumentHighlight> {
    let Some(target) = snapshot.position_map.lsp_to_flux(position) else {
        return Vec::new();
    };

    // 1. Keyword-driven exit points: `return`/`fn` highlight every exit of the
    //    enclosing function. Runs before the AST locator, which returns the
    //    enclosing statement for a raw keyword.
    if let Some(offset) = snapshot.position_map.lsp_to_offset(position) {
        let off: usize = offset.into();
        if !is_offset_in_comment_or_string(snapshot.text.as_ref(), off)
            && let Some(word) = word_at_offset(snapshot.text.as_ref(), off)
            && matches!(word, "return" | "fn")
        {
            let exits = exit_point_highlights(snapshot, target);
            if !exits.is_empty() {
                return exits;
            }
        }
    }

    let Some(node) = find_at(&snapshot.program, &snapshot.interner, target) else {
        return Vec::new();
    };

    // 2. `perform` ↔ `handle` linking for an effect operation.
    if let Some(hl) = effect_op_highlights(snapshot, &node) {
        return hl;
    }

    // 3. Read/write occurrences of the identifier under the cursor.
    let Some(target_id) = node_identifier(&node) else {
        return Vec::new();
    };
    let name = snapshot.interner.try_resolve(target_id).unwrap_or("");
    let mut spans = Vec::new();
    collect_kinded_uses(&snapshot.program, target_id, &mut spans);
    spans
        .into_iter()
        .map(|(span, kind)| DocumentHighlight {
            // `collect_kinded_uses` reports a declaration's whole-statement
            // span; narrow each hit to the identifier name.
            range: occurrence_range(&snapshot.position_map, span, name),
            kind: Some(match kind {
                UseKind::Write => DocumentHighlightKind::WRITE,
                UseKind::Read => DocumentHighlightKind::READ,
            }),
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Exit points (`return` / `fn`)
// ─────────────────────────────────────────────────────────────────────────────

/// Every `return` keyword and the tail expression of the function enclosing
/// `pos`. Empty when `pos` is not inside a function body.
fn exit_point_highlights(snapshot: &Snapshot, pos: FluxPosition) -> Vec<DocumentHighlight> {
    let Some(body) = enclosing_fn_body(&snapshot.program.statements, pos) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_returns_in_block(body, snapshot, &mut out);
    // The body's final expression statement is the implicit return value.
    if let Some(Statement::Expression { expression, .. }) = body.statements.last() {
        out.push(text_highlight(snapshot, expression.span()));
    }
    out
}

/// The body of the innermost function whose signature or body contains `pos`.
/// Descends into `module` blocks and nested `fn` statements.
fn enclosing_fn_body(stmts: &[Statement], pos: FluxPosition) -> Option<&Block> {
    for stmt in stmts {
        match stmt {
            Statement::Function { span, body, .. } => {
                let here = position_in_span(pos, *span)
                    || body
                        .statements
                        .iter()
                        .any(|s| position_in_span(pos, s.span()));
                if here {
                    // Prefer a nested function whose body also contains `pos`.
                    return enclosing_fn_body(&body.statements, pos).or(Some(body));
                }
            }
            Statement::Module { body, .. } => {
                if let Some(b) = enclosing_fn_body(&body.statements, pos) {
                    return Some(b);
                }
            }
            _ => {}
        }
    }
    None
}

/// Collect `return` keyword highlights from a block, descending through
/// block-bearing expressions but *not* into nested functions / lambdas — their
/// returns are their own exits.
fn collect_returns_in_block(block: &Block, snapshot: &Snapshot, out: &mut Vec<DocumentHighlight>) {
    for stmt in &block.statements {
        match stmt {
            Statement::Return { span, .. } => {
                // The statement span starts at the `return` keyword (6 chars).
                let kw = FluxSpan {
                    start: span.start,
                    end: FluxPosition {
                        line: span.start.line,
                        column: span.start.column + "return".len(),
                    },
                };
                out.push(text_highlight(snapshot, kw));
            }
            Statement::Let { value, .. }
            | Statement::Assign { value, .. }
            | Statement::LetDestructure { value, .. } => {
                collect_returns_in_expr(value, snapshot, out)
            }
            Statement::Expression { expression, .. } => {
                collect_returns_in_expr(expression, snapshot, out)
            }
            // A nested `fn` statement owns its own returns — skip it.
            _ => {}
        }
    }
}

fn collect_returns_in_expr(
    expr: &Expression,
    snapshot: &Snapshot,
    out: &mut Vec<DocumentHighlight>,
) {
    match expr {
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            collect_returns_in_expr(condition, snapshot, out);
            collect_returns_in_block(consequence, snapshot, out);
            if let Some(b) = alternative {
                collect_returns_in_block(b, snapshot, out);
            }
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            collect_returns_in_expr(scrutinee, snapshot, out);
            for arm in arms {
                collect_returns_in_expr(&arm.body, snapshot, out);
            }
        }
        Expression::DoBlock { block, .. } => collect_returns_in_block(block, snapshot, out),
        Expression::Handle { expr, arms, .. } => {
            collect_returns_in_expr(expr, snapshot, out);
            for arm in arms {
                collect_returns_in_expr(&arm.body, snapshot, out);
            }
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            collect_returns_in_expr(function, snapshot, out);
            for a in arguments {
                collect_returns_in_expr(a, snapshot, out);
            }
        }
        Expression::Infix { left, right, .. } => {
            collect_returns_in_expr(left, snapshot, out);
            collect_returns_in_expr(right, snapshot, out);
        }
        Expression::Prefix { right, .. } => collect_returns_in_expr(right, snapshot, out),
        // `Expression::Function` (lambda) is intentionally not traversed.
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Effect op (`perform` ↔ `handle`)
// ─────────────────────────────────────────────────────────────────────────────

/// When `node` is an effect operation (a `perform` site, a `handle` arm, or the
/// `effect` declaration), highlight every `perform` of that `(effect, op)` pair
/// and every matching `handle` arm in the file.
fn effect_op_highlights(snapshot: &Snapshot, node: &NodeRef) -> Option<Vec<DocumentHighlight>> {
    let (op, effect) = match node {
        NodeRef::PerformOpName {
            name,
            parent_effect,
            ..
        }
        | NodeRef::HandleArmOpName {
            name,
            parent_effect,
            ..
        }
        | NodeRef::EffectOpName {
            name,
            parent_effect,
            ..
        } => (*name, *parent_effect),
        _ => return None,
    };
    let op_len = snapshot.interner.try_resolve(op)?.len();
    let effect_len = snapshot.interner.try_resolve(effect)?.len();

    let mut spans = Vec::new();
    for stmt in &snapshot.program.statements {
        collect_effect_sites_in_stmt(stmt, effect, op, effect_len, op_len, &mut spans);
    }
    if spans.is_empty() {
        return None;
    }
    Some(
        spans
            .into_iter()
            .map(|span| text_highlight(snapshot, span))
            .collect(),
    )
}

fn collect_effect_sites_in_stmt(
    stmt: &Statement,
    effect: Identifier,
    op: Identifier,
    effect_len: usize,
    op_len: usize,
    out: &mut Vec<FluxSpan>,
) {
    match stmt {
        Statement::Let { value, .. }
        | Statement::Assign { value, .. }
        | Statement::LetDestructure { value, .. } => {
            collect_effect_sites_in_expr(value, effect, op, effect_len, op_len, out)
        }
        Statement::Return { value: Some(v), .. } => {
            collect_effect_sites_in_expr(v, effect, op, effect_len, op_len, out)
        }
        Statement::Expression { expression, .. } => {
            collect_effect_sites_in_expr(expression, effect, op, effect_len, op_len, out)
        }
        Statement::Function { body, .. } | Statement::Module { body, .. } => {
            for s in &body.statements {
                collect_effect_sites_in_stmt(s, effect, op, effect_len, op_len, out);
            }
        }
        _ => {}
    }
}

fn collect_effect_sites_in_expr(
    expr: &Expression,
    effect: Identifier,
    op: Identifier,
    effect_len: usize,
    op_len: usize,
    out: &mut Vec<FluxSpan>,
) {
    let recur = |e: &Expression, out: &mut Vec<FluxSpan>| {
        collect_effect_sites_in_expr(e, effect, op, effect_len, op_len, out)
    };
    match expr {
        Expression::Perform {
            effect: e,
            operation,
            span,
            args,
            ..
        } => {
            if *e == effect && *operation == op {
                out.push(perform_op_span(*span, effect_len, op_len));
            }
            for a in args {
                recur(a, out);
            }
        }
        Expression::Handle {
            effect: e,
            expr,
            arms,
            ..
        } => {
            recur(expr, out);
            if *e == effect {
                for arm in arms {
                    if arm.operation_name == op {
                        out.push(arm_op_span(arm.span, op_len));
                    }
                    recur(&arm.body, out);
                }
            } else {
                for arm in arms {
                    recur(&arm.body, out);
                }
            }
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            recur(function, out);
            for a in arguments {
                recur(a, out);
            }
        }
        Expression::Infix { left, right, .. } => {
            recur(left, out);
            recur(right, out);
        }
        Expression::Prefix { right, .. } => recur(right, out),
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            recur(condition, out);
            for s in &consequence.statements {
                collect_effect_sites_in_stmt(s, effect, op, effect_len, op_len, out);
            }
            if let Some(b) = alternative {
                for s in &b.statements {
                    collect_effect_sites_in_stmt(s, effect, op, effect_len, op_len, out);
                }
            }
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            recur(scrutinee, out);
            for arm in arms {
                recur(&arm.body, out);
            }
        }
        Expression::DoBlock { block, .. } | Expression::Function { body: block, .. } => {
            for s in &block.statements {
                collect_effect_sites_in_stmt(s, effect, op, effect_len, op_len, out);
            }
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for e in elements {
                recur(e, out);
            }
        }
        Expression::Index { left, index, .. } => {
            recur(left, out);
            recur(index, out);
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => recur(value, out),
        Expression::Cons { head, tail, .. } => {
            recur(head, out);
            recur(tail, out);
        }
        _ => {}
    }
}

/// The op-name span inside `perform Effect.op(...)`, synthesized the way the
/// locator does (after `perform `, the effect name, and the `.`).
fn perform_op_span(perform_span: FluxSpan, effect_len: usize, op_len: usize) -> FluxSpan {
    let col = perform_span.start.column + "perform ".len() + effect_len + 1;
    FluxSpan {
        start: FluxPosition {
            line: perform_span.start.line,
            column: col,
        },
        end: FluxPosition {
            line: perform_span.start.line,
            column: col + op_len,
        },
    }
}

/// The op-name span at the start of a `handle` arm (`op(resume, …) -> …`).
fn arm_op_span(arm_span: FluxSpan, op_len: usize) -> FluxSpan {
    FluxSpan {
        start: arm_span.start,
        end: FluxPosition {
            line: arm_span.start.line,
            column: arm_span.start.column + op_len,
        },
    }
}

fn text_highlight(snapshot: &Snapshot, span: FluxSpan) -> DocumentHighlight {
    DocumentHighlight {
        range: snapshot.position_map.flux_span_to_range(span),
        kind: Some(DocumentHighlightKind::TEXT),
    }
}
