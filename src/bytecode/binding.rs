use crate::bytecode::symbol_scope::SymbolScope;
use crate::syntax::position::Span;
use crate::syntax::symbol::Symbol;

#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub name: Symbol,
    pub symbol_scope: SymbolScope,
    pub index: usize,
    pub is_assigned: bool,
    pub span: Span,
}

impl Binding {
    pub fn new(name: Symbol, symbol_scope: SymbolScope, index: usize, span: Span) -> Self {
        Self {
            name,
            symbol_scope,
            index,
            is_assigned: false,
            span,
        }
    }

    pub fn mark_assigned(&mut self) {
        self.is_assigned = true;
    }
}
