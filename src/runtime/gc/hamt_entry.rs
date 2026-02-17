use crate::runtime::{gc::gc_handle::GcHandle, hash_key::HashKey, value::Value};

/// Entry in a HAMT node's compressed child array.
#[derive(Debug, Clone)]
pub enum HamtEntry {
    Leaf(HashKey, Value),
    Node(GcHandle),
    Collision(GcHandle),
}
