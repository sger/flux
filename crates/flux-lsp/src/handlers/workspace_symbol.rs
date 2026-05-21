//! `workspace/symbol` — project-wide symbol search.
//!
//! Declarations are extracted once per file into a [`FileSymbols`] cache held
//! by the [`Workspace`](crate::workspace::Workspace) and refreshed only when
//! that file changes. A query is then a pure name filter over the cache — no
//! re-parse per keystroke in the search box. Each file's top-level declarations
//! (plus those nested one level inside a `module` block, with the module as
//! their container) are indexed, with their LSP ranges resolved at index time
//! so the query carries no `PositionMap` work either.

use std::sync::Arc;

use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;
use flux::syntax::statement::Statement;
use lsp_types::{Location, OneOf, Range, SymbolKind, Uri, WorkspaceSymbol};

use crate::line_index::{PositionEncoding, PositionMap};

/// Cap on returned symbols — a guard against a broad query (or an empty one)
/// flooding the client.
const MAX_RESULTS: usize = 256;

/// One file's pre-extracted declarations — the unit the workspace caches so
/// `workspace/symbol` answers without re-parsing.
pub struct FileSymbols {
    pub uri: Uri,
    pub entries: Vec<SymbolEntry>,
}

/// A single searchable declaration, with everything a [`WorkspaceSymbol`] needs
/// already resolved so a query only filters by name.
pub struct SymbolEntry {
    pub name: String,
    pub kind: SymbolKind,
    pub range: Range,
    /// Enclosing `module` name for a member declared inside a module block.
    pub container: Option<String>,
}

/// Parse `text` and extract every searchable declaration in it — top-level,
/// plus declarations nested one level inside a `module { … }` block (carrying
/// the module name as their container). Run once per file whenever its content
/// changes; the result is cached.
pub fn index_file(uri: &Uri, text: &Arc<str>, encoding: PositionEncoding) -> FileSymbols {
    let lexer = Lexer::new(text.to_string());
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let position_map = PositionMap::new(Arc::clone(text), encoding);

    let mut entries = Vec::new();
    for stmt in &program.statements {
        push_entry(stmt, &interner, &position_map, None, &mut entries);
        // Descend one level into `module { … }` blocks so a module's members are
        // searchable, with the module name as their container.
        if let Statement::Module { name, body, .. } = stmt {
            let container = interner.try_resolve(*name).map(str::to_string);
            for inner in &body.statements {
                push_entry(
                    inner,
                    &interner,
                    &position_map,
                    container.as_deref(),
                    &mut entries,
                );
            }
        }
    }
    FileSymbols {
        uri: uri.clone(),
        entries,
    }
}

/// Filter the cached declarations across `files` by `query` (case-insensitive
/// substring; an empty query matches everything), capped at [`MAX_RESULTS`].
pub fn query(files: &[Arc<FileSymbols>], query: &str) -> Vec<WorkspaceSymbol> {
    let needle = query.to_ascii_lowercase();
    let mut out = Vec::new();
    for file in files {
        for entry in &file.entries {
            if !needle.is_empty() && !entry.name.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            out.push(WorkspaceSymbol {
                name: entry.name.clone(),
                kind: entry.kind,
                tags: None,
                container_name: entry.container.clone(),
                location: OneOf::Left(Location {
                    uri: file.uri.clone(),
                    range: entry.range,
                }),
                data: None,
            });
            if out.len() >= MAX_RESULTS {
                return out;
            }
        }
    }
    out
}

/// Add a [`SymbolEntry`] for `stmt` when it is a named declaration.
fn push_entry(
    stmt: &Statement,
    interner: &flux::syntax::interner::Interner,
    position_map: &PositionMap,
    container: Option<&str>,
    out: &mut Vec<SymbolEntry>,
) {
    let Some((name_id, kind, span)) = symbol_of(stmt) else {
        return;
    };
    let Some(name) = interner.try_resolve(name_id) else {
        return;
    };
    if name.is_empty() {
        return;
    }
    out.push(SymbolEntry {
        name: name.to_string(),
        kind,
        range: position_map.flux_span_to_range(span),
        container: container.map(str::to_string),
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
