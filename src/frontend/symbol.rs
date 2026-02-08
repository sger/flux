/// A unique identifier for an interned string.
///
/// Symbols are created by the `Interner` and should not be constructed manually.
/// They are cheap to copy and compare, making them ideal for compiler data structures.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Symbol(u32);

impl Symbol {
    /// Creates a new symbol from a raw index.
    ///
    /// This is intended for internal use by the `Interner` only.
    /// Creating symbols with arbitrary indices can lead to panics when resolving.
    #[inline]
    pub(crate) fn new(index: u32) -> Self {
        Self(index)
    }

    /// Returns the raw index of this symbol.
    ///
    /// This is primarily useful for debugging or serialization.
    #[inline]
    pub fn as_u32(self) -> u32 {
        self.0
    }
}
