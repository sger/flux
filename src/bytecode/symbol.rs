use crate::bytecode::symbol_scope::SymbolScope;

#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub symbol_scope: SymbolScope,
    pub index: usize,
    pub is_assigned: bool,
}

impl Symbol {
    pub fn new(name: impl Into<String>, symbol_scope: SymbolScope, index: usize) -> Self {
        Self {
            name: name.into(),
            symbol_scope,
            index,
            is_assigned: false,
        }
    }

    pub fn mark_assigned(&mut self) {
        self.is_assigned = true;
    }
}
