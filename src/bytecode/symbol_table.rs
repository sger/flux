use std::collections::HashMap;

use crate::bytecode::{binding::Binding, symbol_scope::SymbolScope};
use crate::diagnostics::position::Span;
use crate::syntax::symbol::Symbol;

#[derive(Debug, Clone)]
pub struct SymbolTable {
    pub outer: Option<Box<SymbolTable>>,
    store: HashMap<Symbol, Binding>,
    pub num_definitions: usize,
    pub free_symbols: Vec<Binding>,
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

    pub fn define(&mut self, name: Symbol, span: Span) -> Binding {
        let scope = if self.outer.is_none() {
            SymbolScope::Global
        } else {
            SymbolScope::Local
        };

        if let Some(_existing) = self.store.get(&name) {
            // Variable already defined in this scope - this would be caught during compilation
            // The compiler will handle the error message
        }

        let symbol = Binding::new(name, scope, self.num_definitions, span);
        self.store.insert(name, symbol.clone());
        self.num_definitions += 1;
        symbol
    }

    pub fn exists_in_current_scope(&self, name: Symbol) -> bool {
        self.store.contains_key(&name)
    }

    pub fn mark_assigned(&mut self, name: Symbol) -> Result<(), String> {
        if let Some(symbol) = self.store.get_mut(&name) {
            symbol.mark_assigned();
            Ok(())
        } else {
            Err(format!("Variable {} not found", name))
        }
    }

    pub fn define_builtin(&mut self, index: usize, name: Symbol) -> Binding {
        let symbol = Binding::new(name, SymbolScope::Builtin, index, Span::default());
        self.store.insert(name, symbol.clone());
        symbol
    }

    pub fn define_function_name(&mut self, name: Symbol, span: Span) -> Binding {
        let symbol = Binding::new(name, SymbolScope::Function, 0, span);
        self.store.insert(name, symbol.clone());
        symbol
    }

    pub fn define_temp(&mut self) -> Binding {
        let scope = if self.outer.is_none() {
            SymbolScope::Global
        } else {
            SymbolScope::Local
        };
        let symbol = Binding::new(
            Symbol::new(u32::MAX),
            scope,
            self.num_definitions,
            Span::default(),
        );
        self.num_definitions += 1;
        symbol
    }

    pub fn resolve(&mut self, name: Symbol) -> Option<Binding> {
        match self.store.get(&name) {
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

    /// Get all symbol names visible from this scope
    ///
    /// Returns all symbols from the current scope and outer scopes,
    /// filtering out temporary symbols (those starting with '<').
    /// Used for generating "did you mean?" suggestions.
    pub fn all_symbol_names(&self) -> Vec<Symbol> {
        let mut names = Vec::new();

        // Add symbols from current scope
        for name in self.store.keys() {
            // Filter out temporary symbols
            if name.as_u32() != u32::MAX {
                names.push(*name);
            }
        }

        // Add symbols from outer scopes
        if let Some(outer) = &self.outer {
            names.extend(outer.all_symbol_names());
        }

        names
    }

    /// Returns all Global-scoped bindings as (Symbol, global_index) pairs.
    /// Used by the test runner to discover `test_*` functions after compilation.
    pub fn global_definitions(&self) -> Vec<(Symbol, usize)> {
        self.store
            .iter()
            .filter(|(_, b)| b.symbol_scope == crate::bytecode::symbol_scope::SymbolScope::Global)
            .map(|(sym, b)| (*sym, b.index))
            .collect()
    }

    pub fn define_free(&mut self, original: Binding) -> Binding {
        self.free_symbols.push(original.clone());
        let symbol = Binding::new(
            original.name,
            SymbolScope::Free,
            self.free_symbols.len() - 1,
            original.span,
        );
        self.store.insert(symbol.name, symbol.clone());
        symbol
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}
