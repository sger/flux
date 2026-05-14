use flux::ast::visit::{self, Visitor};
use flux::diagnostics::position::Position as FluxPosition;
use flux::syntax::Identifier;
use flux::syntax::expression::Expression;
use flux::syntax::program::Program;
use lsp_types::{Location, Position, Uri};

use crate::convert::span_to_range;
use crate::snapshot::Snapshot;

pub fn goto_definition(snapshot: &Snapshot, uri: &Uri, position: Position) -> Option<Location> {
    let ident = identifier_at(&snapshot.program, position)?;
    let entry = snapshot.symbol_index.lookup_id(ident)?;
    Some(Location {
        uri: uri.clone(),
        range: span_to_range(entry.span),
    })
}

fn identifier_at(program: &Program, position: Position) -> Option<Identifier> {
    let target = FluxPosition::new(position.line as usize + 1, position.character as usize);
    let mut finder = IdentifierFinder {
        target,
        found: None,
    };
    finder.visit_program(program);
    finder.found
}

struct IdentifierFinder {
    target: FluxPosition,
    found: Option<Identifier>,
}

impl<'ast> Visitor<'ast> for IdentifierFinder {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Identifier { name, span, .. } = expr {
            let in_span = (span.start.line, span.start.column)
                <= (self.target.line, self.target.column)
                && (self.target.line, self.target.column) <= (span.end.line, span.end.column);
            if in_span {
                self.found = Some(*name);
            }
        }
        visit::walk_expr(self, expr);
    }
}
