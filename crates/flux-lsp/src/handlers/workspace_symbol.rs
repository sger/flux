//! `workspace/symbol` — project-wide symbol search.
//!
//! Given a query string, scan every known project file and return the
//! declarations whose name contains the query (case-insensitive). Each file
//! is parsed fresh (parse-only, no inference) and its top-level declarations
//! — plus declarations nested one level inside a `module` block — are
//! emitted. The gather of `(uri, text)` pairs happens on the main thread;
//! this pure function runs on the worker, so the parsing never blocks the
//! edit loop.

use std::sync::Arc;

use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;
use flux::syntax::statement::Statement;
use lsp_types::{Location, OneOf, SymbolKind, Uri, WorkspaceSymbol};

use crate::line_index::{PositionEncoding, PositionMap};

/// Cap on returned symbols — a guard against a broad query (or an empty one)
/// flooding the client.
const MAX_RESULTS: usize = 256;

/// Collect every declaration across `files` whose name matches `query`.
/// An empty query matches everything (up to [`MAX_RESULTS`]).
pub fn workspace_symbols(
    files: &[(Uri, Arc<str>)],
    query: &str,
    encoding: PositionEncoding,
) -> Vec<WorkspaceSymbol> {
    let needle = query.to_ascii_lowercase();
    let mut out = Vec::new();
    for (uri, text) in files {
        if out.len() >= MAX_RESULTS {
            break;
        }
        collect_file(uri, text, &needle, encoding, &mut out);
    }
    out.truncate(MAX_RESULTS);
    out
}

/// Parse one file and push its matching declarations onto `out`.
fn collect_file(
    uri: &Uri,
    text: &Arc<str>,
    needle: &str,
    encoding: PositionEncoding,
    out: &mut Vec<WorkspaceSymbol>,
) {
    let lexer = Lexer::new(text.to_string());
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let position_map = PositionMap::new(Arc::clone(text), encoding);

    for stmt in &program.statements {
        push_symbol(stmt, &interner, &position_map, uri, None, needle, out);
        // Descend one level into `module { … }` blocks so a module's
        // members are searchable, with the module name as their container.
        if let Statement::Module { name, body, .. } = stmt {
            let container = interner.try_resolve(*name).map(str::to_string);
            for inner in &body.statements {
                push_symbol(
                    inner,
                    &interner,
                    &position_map,
                    uri,
                    container.as_deref(),
                    needle,
                    out,
                );
            }
        }
    }
}

/// Emit a [`WorkspaceSymbol`] for `stmt` when it is a declaration whose name
/// contains `needle`.
fn push_symbol(
    stmt: &Statement,
    interner: &flux::syntax::interner::Interner,
    position_map: &PositionMap,
    uri: &Uri,
    container: Option<&str>,
    needle: &str,
    out: &mut Vec<WorkspaceSymbol>,
) {
    let Some((name_id, kind, span)) = symbol_of(stmt) else {
        return;
    };
    let Some(name) = interner.try_resolve(name_id) else {
        return;
    };
    if name.is_empty() || (!needle.is_empty() && !name.to_ascii_lowercase().contains(needle)) {
        return;
    }
    out.push(WorkspaceSymbol {
        name: name.to_string(),
        kind,
        tags: None,
        container_name: container.map(str::to_string),
        location: OneOf::Left(Location {
            uri: uri.clone(),
            range: position_map.flux_span_to_range(span),
        }),
        data: None,
    });
}

/// Map a declaration statement to `(name, kind, span)`. Mirrors
/// [`crate::handlers::symbols`]'s document-symbol classification, minus
/// `import` (an `import` is a reference, not a definition).
fn symbol_of(stmt: &Statement) -> Option<(Identifier, SymbolKind, FluxSpan)> {
    Some(match stmt {
        Statement::Let { name, span, .. } | Statement::Assign { name, span, .. } => {
            (*name, SymbolKind::VARIABLE, *span)
        }
        Statement::Function { name, span, .. } => (*name, SymbolKind::FUNCTION, *span),
        Statement::Module { name, span, .. } => (*name, SymbolKind::MODULE, *span),
        Statement::Data { name, span, .. } => (*name, SymbolKind::STRUCT, *span),
        Statement::EffectDecl { name, span, .. } | Statement::EffectAlias { name, span, .. } => {
            (*name, SymbolKind::INTERFACE, *span)
        }
        Statement::TypeAlias(alias) => (alias.name, SymbolKind::TYPE_PARAMETER, alias.span),
        Statement::Class { name, span, .. } => (*name, SymbolKind::CLASS, *span),
        Statement::Instance {
            class_name, span, ..
        } => (*class_name, SymbolKind::CLASS, *span),
        _ => return None,
    })
}
