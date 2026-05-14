use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use lsp_types::{DocumentSymbol, SymbolKind};

use crate::convert::span_to_range;

pub fn document_symbols(program: &Program, interner: &Interner) -> Vec<DocumentSymbol> {
    program
        .statements
        .iter()
        .filter_map(|stmt| statement_to_symbol(stmt, interner))
        .collect()
}

fn statement_to_symbol(stmt: &Statement, interner: &Interner) -> Option<DocumentSymbol> {
    let (name, kind, span, detail) = match stmt {
        Statement::Let { name, span, .. } | Statement::Assign { name, span, .. } => {
            (*name, SymbolKind::VARIABLE, *span, None)
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
            (*name, SymbolKind::FUNCTION, *span, detail)
        }
        Statement::Module { name, span, .. } => (*name, SymbolKind::MODULE, *span, None),
        Statement::Data { name, span, .. } => (*name, SymbolKind::STRUCT, *span, None),
        Statement::EffectDecl { name, span, .. } | Statement::EffectAlias { name, span, .. } => {
            (*name, SymbolKind::INTERFACE, *span, None)
        }
        Statement::TypeAlias(alias) => (alias.name, SymbolKind::TYPE_PARAMETER, alias.span, None),
        Statement::Class { name, span, .. } => (*name, SymbolKind::CLASS, *span, None),
        Statement::Instance {
            class_name, span, ..
        } => (
            *class_name,
            SymbolKind::CLASS,
            *span,
            Some("instance".into()),
        ),
        Statement::Import { name, span, .. } => {
            (*name, SymbolKind::MODULE, *span, Some("import".into()))
        }
        _ => return None,
    };

    let resolved = interner.try_resolve(name)?.to_string();
    if resolved.is_empty() {
        return None;
    }

    let range = span_to_range(span);
    #[allow(deprecated)]
    Some(DocumentSymbol {
        name: resolved,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: None,
    })
}
