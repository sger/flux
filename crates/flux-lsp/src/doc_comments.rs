//! Shared `///` doc-comment extraction.
//!
//! The parser drops doc comments — they survive only in the source text — so
//! both completion (`completionItem/resolve`) and hover recover them by
//! scanning the source above a declaration. Kept here so the two handlers and
//! the [`Workspace`](crate::workspace::Workspace) share one implementation.

use std::sync::Arc;

use flux::diagnostics::position::Span;
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;

use crate::line_index::{PositionEncoding, PositionMap};

/// Collect the run of `///` doc-comment lines immediately above the
/// declaration at `span` in `source`, returned as markdown (the `///` markers
/// and one following space stripped). `None` when no doc comment precedes it.
///
/// Scans by byte offset (via a `PositionMap`) so it's independent of how the
/// span's line/column are based, and stops at the first non-`///` line so a
/// blank line or code above the comment block ends the run.
pub fn doc_comment_above(source: &str, span: Span, encoding: PositionEncoding) -> Option<String> {
    let map = PositionMap::new(Arc::from(source), encoding);
    let offset: usize = map.flux_to_offset(span.start)?.into();
    let bytes = source.as_bytes();
    let mut line_start = bytes[..offset.min(bytes.len())]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    let mut lines: Vec<&str> = Vec::new();
    while line_start > 0 {
        let prev_end = line_start - 1; // the '\n' terminating the previous line
        let prev_start = bytes[..prev_end]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |p| p + 1);
        match source[prev_start..prev_end]
            .trim_start()
            .strip_prefix("///")
        {
            Some(rest) => {
                lines.push(rest.strip_prefix(' ').unwrap_or(rest));
                line_start = prev_start;
            }
            None => break,
        }
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    Some(lines.join("\n"))
}

/// Span of the declaration of `member` in `program` — a top-level statement or
/// one nested directly in a `module` body (where module members live). Covers
/// the declaration kinds that are externally referenceable as members.
pub fn member_decl_span(program: &Program, interner: &Interner, member: &str) -> Option<Span> {
    fn matches(stmt: &Statement, interner: &Interner, member: &str) -> Option<Span> {
        let (name, span) = match stmt {
            Statement::Function { name, span, .. }
            | Statement::Let { name, span, .. }
            | Statement::Data { name, span, .. }
            | Statement::Class { name, span, .. }
            | Statement::EffectDecl { name, span, .. }
            | Statement::EffectAlias { name, span, .. } => (*name, *span),
            Statement::TypeAlias(a) => (a.name, a.span),
            _ => return None,
        };
        (interner.try_resolve(name) == Some(member)).then_some(span)
    }
    for stmt in &program.statements {
        if let Some(span) = matches(stmt, interner, member) {
            return Some(span);
        }
        if let Statement::Module { body, .. } = stmt {
            for inner in &body.statements {
                if let Some(span) = matches(inner, interner, member) {
                    return Some(span);
                }
            }
        }
    }
    None
}
