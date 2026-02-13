use crate::runtime::{gc::hamt_entry::HamtEntry, hash_key::HashKey, value::Value};

/// Objects that live on the GC-managed heap.
#[derive(Debug, Clone)]
pub enum HeapObject {
    /// Cons cell for persistent linked lists.
    Cons { head: Value, tail: Value },
    /// Internal node of a Hash Array Mapped Trie (HAMT).
    HamtNode {
        bitmap: u32,
        children: Vec<HamtEntry>,
    },
    /// Collision node for HAMT entries that share the same hash prefix.
    HamtCollision {
        hash: u64,
        entries: Vec<(HashKey, Value)>,
    },
}
