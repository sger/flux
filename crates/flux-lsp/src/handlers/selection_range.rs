//! `textDocument/selectionRange` — smart "expand selection".
//!
//! For each requested position, returns a chain of nested ranges from the
//! innermost AST node containing the cursor outward to the whole file. The
//! editor walks the chain on Shift+Alt+→ / ←.
//!
//! Implemented by collecting the span of every statement, block, and
//! expression that contains the position, then ordering them by size — the
//! collected spans all contain the cursor, so in a well-formed tree they are
//! strictly nested.

use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::block::Block;
use flux::syntax::expression::Expression;
use flux::syntax::statement::Statement;
use lsp_types::{Position, Range, SelectionRange};

use crate::locator::position_in_span;
use crate::snapshot::Snapshot;

/// One [`SelectionRange`] chain per requested position, in the same order.
pub fn selection_ranges(snapshot: &Snapshot, positions: &[Position]) -> Vec<SelectionRange> {
    positions
        .iter()
        .map(|&p| selection_range_at(snapshot, p))
        .collect()
}

/// Build the nested-range chain for a single position.
fn selection_range_at(snapshot: &Snapshot, position: Position) -> SelectionRange {
    let mut spans: Vec<FluxSpan> = Vec::new();
    if let Some(target) = snapshot.position_map.lsp_to_flux(position) {
        for stmt in &snapshot.program.statements {
            collect_stmt(stmt, target, &mut spans);
        }
    }

    // All collected spans contain the cursor, so sorting by length yields the
    // nesting order (innermost first). Equal spans — e.g. an expression
    // statement and its sole expression — collapse to one chain link.
    spans.sort_by_key(|s| span_len(snapshot, *s));
    spans.dedup();

    // Wrap outermost-first so each link's `parent` is the next larger span.
    let mut node: Option<SelectionRange> = None;
    for span in spans.iter().rev() {
        node = Some(SelectionRange {
            range: snapshot.position_map.flux_span_to_range(*span),
            parent: node.map(Box::new),
        });
    }
    node.unwrap_or(SelectionRange {
        range: Range {
            start: position,
            end: position,
        },
        parent: None,
    })
}

/// Source length of `span` in bytes — an exact, total ordering for nesting.
fn span_len(snapshot: &Snapshot, span: FluxSpan) -> usize {
    let offset = |p| {
        snapshot
            .position_map
            .flux_to_offset(p)
            .map_or(0, usize::from)
    };
    offset(span.end).saturating_sub(offset(span.start))
}

fn collect_stmt(stmt: &Statement, target: FluxPosition, out: &mut Vec<FluxSpan>) {
    match stmt {
        // A function's statement span covers only the signature; the body is
        // a separate block. Synthesize a whole-declaration span so "expand
        // selection" has a step covering the entire `fn`/`module`.
        Statement::Function { body, span, .. } | Statement::Module { body, span, .. } => {
            let whole = FluxSpan {
                start: span.start,
                end: body.span.end,
            };
            if position_in_span(target, whole) {
                out.push(whole);
            }
            if position_in_span(target, *span) {
                out.push(*span);
            }
            collect_block(body, target, out);
        }
        other => {
            if !position_in_span(target, other.span()) {
                return;
            }
            out.push(other.span());
            match other {
                Statement::Let { value, .. }
                | Statement::Assign { value, .. }
                | Statement::LetDestructure { value, .. } => collect_expr(value, target, out),
                Statement::Return { value: Some(v), .. } => collect_expr(v, target, out),
                Statement::Expression { expression, .. } => collect_expr(expression, target, out),
                _ => {}
            }
        }
    }
}

fn collect_block(block: &Block, target: FluxPosition, out: &mut Vec<FluxSpan>) {
    if !position_in_span(target, block.span) {
        return;
    }
    out.push(block.span);
    for stmt in &block.statements {
        collect_stmt(stmt, target, out);
    }
}

fn collect_expr(expr: &Expression, target: FluxPosition, out: &mut Vec<FluxSpan>) {
    if !position_in_span(target, expr.span()) {
        return;
    }
    out.push(expr.span());
    match expr {
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            collect_expr(function, target, out);
            for a in arguments {
                collect_expr(a, target, out);
            }
        }
        Expression::Infix { left, right, .. } => {
            collect_expr(left, target, out);
            collect_expr(right, target, out);
        }
        Expression::Prefix { right, .. } => collect_expr(right, target, out),
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            collect_expr(condition, target, out);
            collect_block(consequence, target, out);
            if let Some(alt) = alternative {
                collect_block(alt, target, out);
            }
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            collect_expr(scrutinee, target, out);
            for arm in arms {
                collect_expr(&arm.body, target, out);
            }
        }
        Expression::MemberAccess { object, .. } | Expression::TupleFieldAccess { object, .. } => {
            collect_expr(object, target, out)
        }
        Expression::Index { left, index, .. } => {
            collect_expr(left, target, out);
            collect_expr(index, target, out);
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => collect_expr(value, target, out),
        Expression::Cons { head, tail, .. } => {
            collect_expr(head, target, out);
            collect_expr(tail, target, out);
        }
        Expression::NamedConstructor { fields, .. } => {
            for f in fields {
                if let Some(v) = &f.value {
                    collect_expr(v, target, out);
                }
            }
        }
        Expression::Function { body, .. } | Expression::DoBlock { block: body, .. } => {
            collect_block(body, target, out)
        }
        Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. }
        | Expression::TupleLiteral { elements, .. } => {
            for e in elements {
                collect_expr(e, target, out);
            }
        }
        Expression::Handle { expr, arms, .. } => {
            collect_expr(expr, target, out);
            for arm in arms {
                collect_expr(&arm.body, target, out);
            }
        }
        Expression::Perform { args, .. } => {
            for a in args {
                collect_expr(a, target, out);
            }
        }
        Expression::Spread {
            base, overrides, ..
        } => {
            collect_expr(base, target, out);
            for f in overrides {
                if let Some(v) = &f.value {
                    collect_expr(v, target, out);
                }
            }
        }
        Expression::Hash { pairs, .. } => {
            for (k, v) in pairs {
                collect_expr(k, target, out);
                collect_expr(v, target, out);
            }
        }
        // Leaves (identifiers, literals) contribute only their own span.
        _ => {}
    }
}
