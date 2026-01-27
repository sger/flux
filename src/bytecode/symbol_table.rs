use std::collections::HashMap;

use crate::bytecode::{symbol::Symbol, symbol_scope::SymbolScope};

#[derive(Debug, Clone)]
pub struct SymbolTable {
    pub outer: Option<Box<SymbolTable>>,
    store: HashMap<String, Symbol>,
    pub num_definitions: usize,
    pub free_symbols: Vec<Symbol>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            outer: None,
            store: HashMap::new(),
            num_definitions: 0,
            free_symbols: Vec::new(),
        }
    }

    pub fn new_enclosed(outer: SymbolTable) -> Self {
        Self {
            outer: Some(Box::new(outer)),
            store: HashMap::new(),
            num_definitions: 0,
            free_symbols: Vec::new(),
        }
    }

    pub fn define(&mut self, name: impl Into<String>) -> Symbol {
        let name = name.into();
        let scope = if self.outer.is_none() {
            SymbolScope::Global
        } else {
            SymbolScope::Local
        };

        let symbol = Symbol::new(name.clone(), scope, self.num_definitions);
        self.store.insert(name, symbol.clone());
        self.num_definitions += 1;
        symbol
    }

    pub fn define_builtin(&mut self, index: usize, name: impl Into<String>) -> Symbol {
        let name = name.into();
        let symbol = Symbol::new(name.clone(), SymbolScope::Builtin, index);
        self.store.insert(name, symbol.clone());
        symbol
    }

    pub fn define_function_name(&mut self, name: impl Into<String>) -> Symbol {
        let name = name.into();
        let symbol = Symbol::new(name.clone(), SymbolScope::Function, 0);
        self.store.insert(name, symbol.clone());
        symbol
    }

    pub fn resolve(&mut self, name: &str) -> Option<Symbol> {
        match self.store.get(name) {
            Some(symbol) => Some(symbol.clone()),
            None => {
                if let Some(outer) = &mut self.outer {
                    let obj = outer.resolve(name)?;
                    if obj.symbol_scope == SymbolScope::Global
                        || obj.symbol_scope == SymbolScope::Builtin
                    {
                        return Some(obj);
                    }
                    Some(self.define_free(obj))
                } else {
                    None
                }
            }
        }
    }

    pub fn define_free(&mut self, original: Symbol) -> Symbol {
        self.free_symbols.push(original.clone());
        let symbol = Symbol::new(
            original.name,
            SymbolScope::Free,
            self.free_symbols.len() - 1,
        );
        self.store.insert(symbol.name.clone(), symbol.clone());
        symbol
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}
