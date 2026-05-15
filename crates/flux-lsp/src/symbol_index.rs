use std::collections::HashMap;

use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;

/// Index of top-level definitions in one file.
///
/// Maps the identifier symbol to (definition span, resolved name). The name is
/// retained so handlers can look up by string without round-tripping through
/// the interner.
pub struct SymbolIndex {
    by_id: HashMap<Identifier, Entry>,
    by_name: HashMap<String, Entry>,
}

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub span: FluxSpan,
}

impl SymbolIndex {
    pub fn build(program: &Program, interner: &Interner) -> Self {
        let mut by_id = HashMap::new();
        let mut by_name = HashMap::new();
        for stmt in &program.statements {
            if let Some((sym, span)) = top_level_definition(stmt)
                && let Some(name) = interner.try_resolve(sym)
                && !name.is_empty()
            {
                let entry = Entry {
                    name: name.to_string(),
                    span,
                };
                by_id.insert(sym, entry.clone());
                by_name.insert(name.to_string(), entry);
            }
        }
        Self { by_id, by_name }
    }

    pub fn lookup(&self, name: &str) -> Option<&Entry> {
        self.by_name.get(name)
    }

    pub fn lookup_id(&self, id: Identifier) -> Option<&Entry> {
        self.by_id.get(&id)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(|s| s.as_str())
    }
}

fn top_level_definition(stmt: &Statement) -> Option<(Identifier, FluxSpan)> {
    match stmt {
        Statement::Let { name, span, .. }
        | Statement::Function { name, span, .. }
        | Statement::Module { name, span, .. }
        | Statement::Data { name, span, .. }
        | Statement::EffectDecl { name, span, .. }
        | Statement::EffectAlias { name, span, .. }
        | Statement::Class { name, span, .. } => Some((*name, *span)),
        Statement::TypeAlias(alias) => Some((alias.name, alias.span)),
        _ => None,
    }
}

impl SymbolIndex {
    /// Build an extended index that also maps effect operation names and data
    /// variant names to their declaration spans. Used for cross-declaration
    /// goto-definition (e.g. F12 on `perform Emit(x)` → jumps to the `Emit`
    /// op inside its `effect` block).
    pub fn build_extended(program: &Program, interner: &Interner) -> Self {
        let mut idx = Self::build(program, interner);
        for stmt in &program.statements {
            match stmt {
                Statement::EffectDecl { ops, .. } => {
                    for op in ops {
                        if let Some(name) = interner.try_resolve(op.name) {
                            let entry = Entry { name: name.to_string(), span: op.span };
                            idx.by_id.insert(op.name, entry.clone());
                            idx.by_name.insert(name.to_string(), entry);
                        }
                    }
                }
                Statement::Data { variants, .. } => {
                    for v in variants {
                        if let Some(name) = interner.try_resolve(v.name) {
                            let entry = Entry { name: name.to_string(), span: v.span };
                            idx.by_id.insert(v.name, entry.clone());
                            idx.by_name.insert(name.to_string(), entry);
                        }
                    }
                }
                _ => {}
            }
        }
        idx
    }
}
