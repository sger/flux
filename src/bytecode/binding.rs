use crate::bytecode::symbol_scope::SymbolScope;
use crate::frontend::position::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub name: String,
    pub symbol_scope: SymbolScope,
    pub index: usize,
    pub is_assigned: bool,
    pub span: Span,
}

impl Binding {
    pub fn new(
        name: impl Into<String>,
        symbol_scope: SymbolScope,
        index: usize,
        span: Span,
    ) -> Self {
        Self {
            name: name.into(),
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
