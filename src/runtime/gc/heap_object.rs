use std::rc::Rc;

use crate::runtime::{
    gc::hamt_entry::HamtEntry,
    hash_key::HashKey,
    value::{AdtFields, Value},
};

/// Objects that live on the GC-managed heap.
#[derive(Debug, Clone)]
pub enum HeapObject {
    /// Cons cell for persistent linked lists.
    Cons { head: Value, tail: Value },
    /// GC-backed ADT payload used by VM and JIT non-nullary constructors.
    Adt {
        constructor: Rc<String>,
        fields: AdtFields,
    },
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

#[cfg(feature = "gc-telemetry")]
impl HeapObject {
    /// Estimates the shallow byte size of this object including inline Vec capacity.
    ///
    /// Counts `size_of::<Self>()` plus heap-allocated Vec backing storage.
    pub fn shallow_size_bytes(&self) -> usize {
        let base = std::mem::size_of::<Self>();
        match self {
            HeapObject::Cons { .. } => base,
            HeapObject::Adt { fields, .. } => match fields {
                AdtFields::One(_) | AdtFields::Two(..) | AdtFields::Three(..) => base,
                AdtFields::Many(values) => base + values.capacity() * std::mem::size_of::<Value>(),
            },
            HeapObject::HamtNode { children, .. } => {
                base + children.capacity() * std::mem::size_of::<HamtEntry>()
            }
            HeapObject::HamtCollision { entries, .. } => {
                base + entries.capacity() * std::mem::size_of::<(HashKey, Value)>()
            }
        }
    }
}
