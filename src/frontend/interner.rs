use std::{
    collections::HashMap,
    hash::{BuildHasher, Hash, Hasher, RandomState},
};

use crate::frontend::{entry::Entry, symbol::Symbol};

/// A string interner that stores unique strings and returns symbols for efficient comparison.
///
/// The interner uses a hash-bucketing strategy to handle collisions and stores all strings
/// in a single contiguous buffer for cache efficiency. Symbols are represented as `u32` indices,
/// making them cheap to copy and compare.
///
/// # Example
///
/// ```
/// use flux::frontend::interner::Interner;
///
/// let mut interner = Interner::new();
/// let sym1 = interner.intern("hello");
/// let sym2 = interner.intern("hello");
/// let sym3 = interner.intern("world");
///
/// assert_eq!(sym1, sym2); // Same string gets same symbol
/// assert_ne!(sym1, sym3); // Different strings get different symbols
/// assert_eq!(interner.resolve(sym1), "hello");
/// ```
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
    /// Creates a new empty interner.
    pub fn new() -> Self {
        Self {
            hasher: RandomState::new(),
            buckets: HashMap::default(),
            entries: Vec::new(),
            storage: String::new(),
        }
    }

    /// Creates a new interner with pre-allocated capacity.
    ///
    /// # Arguments
    ///
    /// * `symbol_capacity` - Number of unique symbols to pre-allocate space for
    /// * `storage_bytes` - Number of bytes to pre-allocate in the string storage buffer
    pub fn with_capacity(symbol_capacity: usize, storage_bytes: usize) -> Self {
        Self {
            hasher: RandomState::new(),
            buckets: HashMap::with_capacity(symbol_capacity),
            entries: Vec::with_capacity(symbol_capacity),
            storage: String::with_capacity(storage_bytes),
        }
    }

    /// Reserves capacity for additional symbols and storage.
    ///
    /// # Arguments
    ///
    /// * `symbols` - Additional number of symbols to reserve space for
    /// * `storage_bytes` - Additional bytes to reserve in the string storage
    pub fn reserve(&mut self, symbols: usize, storage_bytes: usize) {
        self.buckets.reserve(symbols);
        self.entries.reserve(symbols);
        self.storage.reserve(storage_bytes);
    }

    /// Clears all interned strings while preserving allocated capacity.
    ///
    /// This is useful for reusing the interner across multiple compilation units
    /// in long-running processes (e.g., LSP servers, REPLs).
    pub fn clear(&mut self) {
        self.buckets.clear();
        self.entries.clear();
        self.storage.clear();
    }

    /// Interns a string and returns its symbol.
    ///
    /// If the string has been interned before, returns the existing symbol.
    /// Otherwise, stores the string and returns a new symbol.
    ///
    /// # Panics
    ///
    /// Panics if the number of unique symbols exceeds `u32::MAX`.
    pub fn intern(&mut self, s: &str) -> Symbol {
        let hash = self.hash_str(s);
        if let Some(candidates) = self.buckets.get(&hash) {
            for candidate in candidates {
                if self.resolve(*candidate) == s {
                    return *candidate;
                }
            }
        }

        let index = self.entries.len();
        assert!(
            index <= u32::MAX as usize,
            "symbol table overflow: cannot intern more than {} unique strings",
            u32::MAX
        );
        let sym = Symbol::new(index as u32);

        let start = self.storage.len();
        self.storage.push_str(s);

        let end = self.storage.len();

        self.entries.push(Entry::new(start, end));
        self.buckets.entry(hash).or_default().push(sym);
        sym
    }

    /// Resolves a symbol to its string value.
    ///
    /// # Panics
    ///
    /// Panics if the symbol is invalid (not created by this interner).
    /// This is a programming error that should never occur in correct code.
    #[inline]
    pub fn resolve(&self, sym: Symbol) -> &str {
        self.try_resolve(sym)
            .unwrap_or_else(|| panic!("invalid symbol: {:?}", sym))
    }

    /// Attempts to resolve a symbol to its string value.
    ///
    /// Returns `None` if the symbol is invalid (not created by this interner).
    /// Prefer using `resolve()` when you're certain the symbol is valid.
    pub fn try_resolve(&self, sym: Symbol) -> Option<&str> {
        let entry = self.entries.get(sym.as_u32() as usize)?;
        self.storage.get(entry.start..entry.end)
    }

    fn hash_str(&self, s: &str) -> u64 {
        let mut h = self.hasher.build_hasher();

        s.hash(&mut h);
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn try_resolve_returns_none_for_invalid_symbol() {
        let interner = Interner::new();
        let invalid_symbol = Symbol::new(999);
        assert_eq!(interner.try_resolve(invalid_symbol), None);
    }

    #[test]
    #[should_panic(expected = "invalid symbol")]
    fn resolve_panics_on_invalid_symbol() {
        let interner = Interner::new();
        let invalid_symbol = Symbol::new(999);
        let _ = interner.resolve(invalid_symbol);
    }

    #[test]
    fn handles_unicode_identifiers() {
        let mut interner = Interner::new();
        let sym1 = interner.intern("α");
        let sym2 = interner.intern("β");
        let sym3 = interner.intern("你好");
        let sym4 = interner.intern("α"); // duplicate

        assert_eq!(sym1, sym4);
        assert_ne!(sym1, sym2);
        assert_eq!(interner.resolve(sym1), "α");
        assert_eq!(interner.resolve(sym2), "β");
        assert_eq!(interner.resolve(sym3), "你好");
    }

    #[test]
    fn handles_empty_string() {
        let mut interner = Interner::new();
        let sym1 = interner.intern("");
        let sym2 = interner.intern("");

        assert_eq!(sym1, sym2);
        assert_eq!(interner.resolve(sym1), "");
    }

    #[test]
    fn clear_removes_all_entries() {
        let mut interner = Interner::new();
        let sym1 = interner.intern("hello");
        let sym2 = interner.intern("world");

        assert_eq!(interner.resolve(sym1), "hello");
        assert_eq!(interner.resolve(sym2), "world");

        interner.clear();

        // After clear, old symbols are invalid
        assert_eq!(interner.try_resolve(sym1), None);
        assert_eq!(interner.try_resolve(sym2), None);

        // New symbols can be created
        let sym3 = interner.intern("new");
        assert_eq!(interner.resolve(sym3), "new");
    }

    #[test]
    fn with_capacity_preallocates() {
        let interner = Interner::with_capacity(100, 1000);
        // Just verify it doesn't panic and works correctly
        drop(interner);
    }

    #[test]
    fn reserve_expands_capacity() {
        let mut interner = Interner::new();
        interner.reserve(50, 500);

        // Add some strings to verify it still works
        let sym = interner.intern("test");
        assert_eq!(interner.resolve(sym), "test");
    }

    #[test]
    fn handles_hash_collisions_correctly() {
        // This test verifies that even if multiple strings hash to the same bucket,
        // they are correctly distinguished by their actual content
        let mut interner = Interner::new();

        // Create many strings to increase chance of collisions
        let strings: Vec<String> = (0..100)
            .map(|i| format!("identifier_{}", i))
            .collect();

        let symbols: Vec<Symbol> = strings
            .iter()
            .map(|s| interner.intern(s))
            .collect();

        // Verify all symbols are unique
        for i in 0..symbols.len() {
            for j in (i + 1)..symbols.len() {
                assert_ne!(symbols[i], symbols[j]);
            }
        }

        // Verify all resolve correctly
        for (i, sym) in symbols.iter().enumerate() {
            assert_eq!(interner.resolve(*sym), strings[i]);
        }

        // Verify interning same strings returns same symbols
        for (i, s) in strings.iter().enumerate() {
            let sym = interner.intern(s);
            assert_eq!(sym, symbols[i]);
        }
    }

    #[test]
    fn very_long_string() {
        let mut interner = Interner::new();
        let long_string = "a".repeat(10000);
        let sym = interner.intern(&long_string);
        assert_eq!(interner.resolve(sym), long_string);
    }

    #[test]
    fn strings_with_special_characters() {
        let mut interner = Interner::new();
        let strings = vec![
            "hello\nworld",
            "tab\there",
            "quote\"inside",
            "slash\\back",
            "null\0byte",
        ];

        for s in &strings {
            let sym = interner.intern(s);
            assert_eq!(interner.resolve(sym), *s);
        }
    }
}
