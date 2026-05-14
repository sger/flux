use flux::ast::visit::{self, Visitor};
use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::expression::{ExprId, Expression};
use flux::syntax::program::Program;
use lsp_types::Position as LspPosition;

/// Reverse index from source position to AST `ExprId`.
///
/// Entries are stored in document order (start position ascending). Lookup
/// returns the innermost expression whose span contains the cursor.
pub struct SpanIndex {
    entries: Vec<Entry>,
}

#[derive(Clone, Copy)]
struct Entry {
    span: FluxSpan,
    id: ExprId,
}

impl SpanIndex {
    pub fn build(program: &Program) -> Self {
        let mut builder = Builder { entries: vec![] };
        builder.visit_program(program);
        builder.entries.sort_by_key(|e| {
            (
                e.span.start.line,
                e.span.start.column,
                // Larger spans first so innermost wins after the scan.
                std::cmp::Reverse((e.span.end.line, e.span.end.column)),
            )
        });
        Self {
            entries: builder.entries,
        }
    }

    /// Return the innermost expression covering `position`, or `None` if none.
    pub fn expr_at(&self, position: LspPosition) -> Option<ExprId> {
        let target = FluxPosition::new(position.line as usize + 1, position.character as usize);
        // Scan all entries whose start is at or before `target`, keep the
        // one with the latest start whose end is past `target`. Since the
        // list is sorted by (start asc, end desc), iterating in reverse and
        // picking the first containing entry yields the innermost.
        self.entries
            .iter()
            .rev()
            .find(|e| span_contains(&e.span, target))
            .map(|e| e.id)
    }
}

fn span_contains(span: &FluxSpan, p: FluxPosition) -> bool {
    let after_start = (span.start.line, span.start.column) <= (p.line, p.column);
    let before_end = (p.line, p.column) <= (span.end.line, span.end.column);
    after_start && before_end
}

struct Builder {
    entries: Vec<Entry>,
}

impl<'ast> Visitor<'ast> for Builder {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        self.entries.push(Entry {
            span: expr.span(),
            id: expr.expr_id(),
        });
        visit::walk_expr(self, expr);
    }
}
