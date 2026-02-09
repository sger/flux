/// Represents the byte range of an interned string in the storage buffer.
///
/// This is an internal data structure used by the `Interner` to track
/// where each unique string is stored in the contiguous string buffer.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub(super) start: usize,
    pub(super) end: usize,
}

impl Entry {
    /// Creates a new entry with the given start and end positions.
    #[inline]
    pub(super) fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Returns the starting byte position of this entry.
    #[inline]
    pub fn start(&self) -> usize {
        self.start
    }

    /// Returns the ending byte position of this entry.
    #[inline]
    pub fn end(&self) -> usize {
        self.end
    }

    /// Returns the length in bytes of this entry.
    #[inline]
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Returns `true` if this entry is empty (zero length).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}
