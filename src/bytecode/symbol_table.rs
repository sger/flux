use std::collections::HashMap;

use crate::bytecode::{symbol::Symbol, symbol_scope::SymbolScope};
use crate::frontend::position::Span;

#[derive(Debug, Clone)]
pub struct SymbolTable {
    pub outer: Option<Box<SymbolTable>>,
    store: HashMap<String, Symbol>,
    pub num_definitions: usize,
    pub free_symbols: Vec<Symbol>,
    allow_free: bool,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            outer: None,
            store: HashMap::new(),
            num_definitions: 0,
            free_symbols: Vec::new(),
            allow_free: true,
        }
    }

    pub fn new_enclosed(outer: SymbolTable) -> Self {
        Self {
            outer: Some(Box::new(outer)),
            store: HashMap::new(),
            num_definitions: 0,
            free_symbols: Vec::new(),
            allow_free: true,
        }
    }

    pub fn new_block(outer: SymbolTable) -> Self {
        Self {
            outer: Some(Box::new(outer)),
            store: HashMap::new(),
            num_definitions: 0,
            free_symbols: Vec::new(),
            allow_free: false,
        }
    }

    pub fn define(&mut self, name: impl Into<String>, span: Span) -> Symbol {
        let name = name.into();
        let scope = if self.outer.is_none() {
            SymbolScope::Global
        } else {
            SymbolScope::Local
        };

        if let Some(_existing) = self.store.get(&name) {
            // Variable already defined in this scope - this would be caught during compilation
            // The compiler will handle the error message
        }

        let symbol = Symbol::new(name.clone(), scope, self.num_definitions, span);
        self.store.insert(name, symbol.clone());
        self.num_definitions += 1;
        symbol
    }

    pub fn exists_in_current_scope(&self, name: &str) -> bool {
        self.store.contains_key(name)
    }

    pub fn mark_assigned(&mut self, name: &str) -> Result<(), String> {
        if let Some(symbol) = self.store.get_mut(name) {
            symbol.mark_assigned();
            Ok(())
        } else {
            Err(format!("Variable {} not found", name))
        }
    }

    pub fn define_builtin(&mut self, index: usize, name: impl Into<String>) -> Symbol {
        let name = name.into();
        let symbol = Symbol::new(name.clone(), SymbolScope::Builtin, index, Span::default());
        self.store.insert(name, symbol.clone());
        symbol
    }

    pub fn define_function_name(&mut self, name: impl Into<String>, span: Span) -> Symbol {
        let name = name.into();
        let symbol = Symbol::new(name.clone(), SymbolScope::Function, 0, span);
        self.store.insert(name, symbol.clone());
        symbol
    }

    pub fn define_temp(&mut self) -> Symbol {
        let scope = if self.outer.is_none() {
            SymbolScope::Global
        } else {
            SymbolScope::Local
        };
        let symbol = Symbol::new("<temp>", scope, self.num_definitions, Span::default());
        self.num_definitions += 1;
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
                    if self.allow_free {
                        Some(self.define_free(obj))
                    } else {
                        Some(obj)
                    }
                } else {
                    None
                }
            }
        }
    }

    pub fn define_free(&mut self, original: Symbol) -> Symbol {
        self.free_symbols.push(original.clone());
        let symbol = Symbol::new(
            original.name.clone(),
            SymbolScope::Free,
            self.free_symbols.len() - 1,
            original.span,
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
