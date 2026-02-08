use std::{
    collections::HashMap,
    hash::{BuildHasher, Hash, Hasher, RandomState},
};

use crate::frontend::{entry::Entry, symbol::Symbol};

#[derive(Debug, Clone)]
pub struct Interner {
    hasher: RandomState,
    buckets: HashMap<u64, Vec<Symbol>>,
    entries: Vec<Entry>,
    storage: String,
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

impl Interner {
    pub fn new() -> Self {
        Self {
            hasher: RandomState::new(),
            buckets: HashMap::default(),
            entries: Vec::new(),
            storage: String::new(),
        }
    }

    pub fn with_capacity(symbol_capacity: usize, storage_bytes: usize) -> Self {
        Self {
            hasher: RandomState::new(),
            buckets: HashMap::with_capacity(symbol_capacity),
            entries: Vec::with_capacity(symbol_capacity),
            storage: String::with_capacity(storage_bytes),
        }
    }

    pub fn reserve(&mut self, symbols: usize, storage_bytes: usize) {
        self.buckets.reserve(symbols);
        self.entries.reserve(symbols);
        self.storage.reserve(storage_bytes);
    }

    pub fn intern(&mut self, s: &str) -> Symbol {
        let hash = self.hash_str(s);
        if let Some(candidates) = self.buckets.get(&hash) {
            for candidate in candidates {
                if self.resolve(*candidate) == s {
                    return *candidate;
                }
            }
        }

        let sym = Symbol(self.entries.len() as u32);

        let start = self.storage.len();
        self.storage.push_str(s);

        let end = self.storage.len();

        self.entries.push(Entry { start, end });
        self.buckets.entry(hash).or_default().push(sym);
        sym
    }

    pub fn resolve(&self, sym: Symbol) -> &str {
        let Some(entry) = self.entries.get(sym.0 as usize) else {
            return "";
        };

        self.storage.get(entry.start..entry.end).unwrap_or("")
    }

    fn hash_str(&self, s: &str) -> u64 {
        let mut h = self.hasher.build_hasher();

        s.hash(&mut h);
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::frontend::interner::Interner;

    #[test]
    fn interning_reuses_symbol_for_same_identifier() {
        let mut interner = Interner::new();
        let a = interner.intern("alpha");
        let b = interner.intern("alpha");
        let c = interner.intern("beta");

        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(interner.resolve(a), "alpha");
        assert_eq!(interner.resolve(c), "beta");
    }
}
