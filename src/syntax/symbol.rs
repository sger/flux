use std::fmt;

use serde::{Deserialize, Serialize};

/// A unique identifier for an interned string.
///
/// Symbols are created by the `Interner` and should not be constructed manually.
/// They are cheap to copy and compare, making them ideal for compiler data structures.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub struct Symbol(u32);

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "${}", self.0)
    }
}

impl Symbol {
    /// Creates a new symbol from a raw index.
    ///
    /// This is intended for internal use by the `Interner` only.
    /// Creating symbols with arbitrary indices can lead to panics when resolving.
    #[inline]
    pub(crate) fn new(index: u32) -> Self {
        Self(index)
    }

    /// A sentinel symbol that does not correspond to any interned string.
    ///
    /// Used as a "no value" marker by code that wants a `Copy`, `Eq`-able
    /// placeholder without paying for `Option<Symbol>`. Resolving the sentinel
    /// against an `Interner` will fail (`try_resolve` returns `None`,
    /// `resolve` panics) — call sites that may hold the sentinel must check
    /// for it explicitly before resolving.
    ///
    /// The interner cannot produce this value because it would have to first
    /// allocate `u32::MAX` distinct strings, which is rejected by the overflow
    /// guard in `Interner::intern`.
    pub const SENTINEL: Symbol = Symbol(u32::MAX);

    /// Returns true if this is the [`SENTINEL`](Self::SENTINEL) symbol.
    #[inline]
    pub const fn is_sentinel(self) -> bool {
        self.0 == u32::MAX
    }

    /// Returns the raw index of this symbol.
    ///
    /// This is primarily useful for debugging or serialization.
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}
