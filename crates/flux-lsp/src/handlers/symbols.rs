use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use lsp_types::{DocumentSymbol, SymbolKind};

use crate::handlers::references::occurrence_range;
use crate::line_index::PositionMap;

pub fn document_symbols(
    program: &Program,
    interner: &Interner,
    position_map: &PositionMap,
) -> Vec<DocumentSymbol> {
    program
        .statements
        .iter()
        .filter_map(|stmt| statement_to_symbol(stmt, interner, position_map))
        .collect()
}

fn statement_to_symbol(
    stmt: &Statement,
    interner: &Interner,
    position_map: &PositionMap,
) -> Option<DocumentSymbol> {
    // `name_span` is the precise name range when the AST records one (class /
    // instance heads, whose name can sit after `=>`); otherwise it's `None` and
    // the name is located textually within the statement span below.
    let (name, kind, span, detail, name_span): (_, _, _, _, Option<FluxSpan>) = match stmt {
        Statement::Let { name, span, .. } | Statement::Assign { name, span, .. } => {
            (*name, SymbolKind::VARIABLE, *span, None, None)
        }
        Statement::Function {
            name,
            span,
            parameters,
            ..
        } => {
            let detail = if parameters.is_empty() {
                None
            } else {
                Some(format!("{} param(s)", parameters.len()))
            };
            (*name, SymbolKind::FUNCTION, *span, detail, None)
        }
        Statement::Module { name, span, .. } => (*name, SymbolKind::MODULE, *span, None, None),
        Statement::Data { name, span, .. } => (*name, SymbolKind::STRUCT, *span, None, None),
        Statement::EffectDecl { name, span, .. } | Statement::EffectAlias { name, span, .. } => {
            (*name, SymbolKind::INTERFACE, *span, None, None)
        }
        Statement::TypeAlias(alias) => (
            alias.name,
            SymbolKind::TYPE_PARAMETER,
            alias.span,
            None,
            None,
        ),
        Statement::Class {
            name,
            span,
            name_span,
            ..
        } => (*name, SymbolKind::CLASS, *span, None, Some(*name_span)),
        Statement::Instance {
            class_name,
            span,
            name_span,
            ..
        } => (
            *class_name,
            SymbolKind::CLASS,
            *span,
            Some("instance".into()),
            Some(*name_span),
        ),
        Statement::Import { name, span, .. } => (
            *name,
            SymbolKind::MODULE,
            *span,
            Some("import".into()),
            None,
        ),
        _ => return None,
    };

    let resolved = interner.try_resolve(name)?.to_string();
    if resolved.is_empty() {
        return None;
    }

    let range = position_map.flux_span_to_range(span);
    // `selection_range` is the name (what the outline highlights on select), not
    // the whole declaration: use the AST name span when present, else locate the
    // name within the statement span.
    let selection_range = match name_span {
        Some(ns) => position_map.flux_span_to_range(ns),
        None => occurrence_range(position_map, span, &resolved),
    };
    #[allow(deprecated)]
    Some(DocumentSymbol {
        name: resolved,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children: None,
    })
}
