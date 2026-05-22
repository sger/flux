//! Doc-comment access over the parsed AST.
//!
//! The parser records `///` / `/** */` doc comments on the [`Program`] it
//! produces, keyed by the source line of the declaration each run sits above
//! (see `Program::doc_comments`). So hover, completion (`completionItem/resolve`)
//! and signature help recover a declaration's documentation with a line lookup
//! instead of re-scanning source text on every request.

use flux::diagnostics::position::Span;
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;

/// The doc comment attached to the declaration at `span` in `program`, if the
/// parser captured a `///`/`/** */` run immediately above it. Keyed by the
/// declaration's start line, so any span on that first line — the statement
/// keyword or the declared name — resolves the same doc.
pub fn doc_for(program: &Program, span: Span) -> Option<String> {
    program.doc_for_line(span.start.line).map(str::to_string)
}

/// The doc comment of the declaration of `member` in `program` — a top-level
/// statement or one nested directly in a `module` body. Combines
/// [`member_decl_span`] with [`doc_for`].
pub fn member_doc(program: &Program, interner: &Interner, member: &str) -> Option<String> {
    doc_for(program, member_decl_span(program, interner, member)?)
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
