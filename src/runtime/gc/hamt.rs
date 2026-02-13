use std::hash::{DefaultHasher, Hash, Hasher};

use crate::runtime::{
    gc::{GcHandle, GcHeap, HamtEntry, HeapObject},
    hash_key::HashKey,
    value::Value,
};

/// Bits consumed per HAMT level.
const BITS_PER_LEVEL: u32 = 5;
/// Maximum depth (64-bit hash / 5 bits per level = 12.8, round up).
const MAX_DEPTH: u32 = 13;

fn hash_key(key: &HashKey) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// Extract the 5-bit slot index at a given depth from the hash.
fn slot_at_depth(hash: u64, depth: u32) -> u32 {
    ((hash >> (depth * BITS_PER_LEVEL)) & 0x1F) as u32
}

/// Count the number of set bits below a given position in the bitmap.
fn compressed_index(bitmap: u32, slot: u32) -> usize {
    (bitmap & ((1 << slot) - 1)).count_ones() as usize
}

/// Creates an empty HAMT root node on the heap.
pub fn hamt_empty(heap: &mut GcHeap) -> GcHandle {
    heap.alloc(HeapObject::HamtNode {
        bitmap: 0,
        children: Vec::new(),
    })
}

/// Looks up a key in a HAMT.
pub fn hamt_lookup(heap: &GcHeap, root: GcHandle, key: &HashKey) -> Option<Value> {
    let hash = hash_key(key);
    let mut handle = root;
    let mut depth = 0u32;

    loop {
        match heap.get(handle) {
            HeapObject::HamtNode { bitmap, children } => {
                let slot = slot_at_depth(hash, depth);
                let bit = 1u32 << slot;

                if bitmap & bit == 0 {
                    return None; // Key not present
                }

                let idx = compressed_index(*bitmap, slot);

                match &children[idx] {
                    HamtEntry::Leaf(k, v) => {
                        if k == key {
                            return Some(v.clone());
                        }
                        return None;
                    }
                    HamtEntry::Node(child) => {
                        handle = *child;
                        depth += 1;
                    }
                    HamtEntry::Collision(col) => {
                        handle = *col;
                        // Will be matched as HamtCollision on next iteration
                        depth += 1;
                    }
                }
            }
            HeapObject::HamtCollision { entries, .. } => {
                for (k, v) in entries {
                    if k == key {
                        return Some(v.clone());
                    }
                }
                return None;
            }
            _ => return None,
        }
    }
}

/// Inserts a key-value pair into a HAMT, returning a new root handle.
/// The original HAMT is not modified.
pub fn hamt_insert(heap: &mut GcHeap, root: GcHandle, key: HashKey, value: Value) -> GcHandle {
    let hash = hash_key(&key);
    hamt_insert_at(heap, root, key, value, hash, 0)
}

fn hamt_insert_at(
    heap: &mut GcHeap,
    node: GcHandle,
    key: HashKey,
    value: Value,
    hash: u64,
    depth: u32,
) -> GcHandle {
    // Clone the node data since we need to read and then allocate
    let node_data = heap.get(node).clone();

    match node_data {
        HeapObject::HamtNode { bitmap, children } => {
            let slot = slot_at_depth(hash, depth);
            let bit = 1u32 << slot;
            let idx = compressed_index(bitmap, slot);

            if bitmap & bit == 0 {
                // Slot is empty — insert a new leaf
                let mut new_children = children;
                new_children.insert(idx, HamtEntry::Leaf(key, value));
                heap.alloc(HeapObject::HamtNode {
                    bitmap: bitmap | bit,
                    children: new_children,
                })
            } else {
                // Slot is occupied — need to handle collision or recurse
                let mut new_children = children;

                match &new_children[idx] {
                    HamtEntry::Leaf(existing_key, existing_value) => {
                        if *existing_key == key {
                            // Update existing key
                            new_children[idx] = HamtEntry::Leaf(key, value);
                            heap.alloc(HeapObject::HamtNode {
                                bitmap,
                                children: new_children,
                            })
                        } else {
                            // Hash collision at this level — push down
                            let existing_hash = hash_key(existing_key);
                            let ek = existing_key.clone();
                            let ev = existing_value.clone();

                            if depth + 1 >= MAX_DEPTH {
                                // At max depth, create a collision node
                                let col = heap.alloc(HeapObject::HamtCollision {
                                    hash,
                                    entries: vec![(ek, ev), (key, value)],
                                });

                                new_children[idx] = HamtEntry::Collision(col);
                                heap.alloc(HeapObject::HamtNode {
                                    bitmap,
                                    children: new_children,
                                })
                            } else {
                                // Create a sub-node and insert both entries
                                let empty = heap.alloc(HeapObject::HamtNode {
                                    bitmap: 0,
                                    children: Vec::new(),
                                });

                                let sub =
                                    hamt_insert_at(heap, empty, ek, ev, existing_hash, depth + 1);
                                let sub = hamt_insert_at(heap, sub, key, value, hash, depth + 1);

                                new_children[idx] = HamtEntry::Node(sub);
                                heap.alloc(HeapObject::HamtNode {
                                    bitmap,
                                    children: new_children,
                                })
                            }
                        }
                    }
                    HamtEntry::Node(child) => {
                        let child = *child;
                        let new_child = hamt_insert_at(heap, child, key, value, hash, depth + 1);
                        new_children[idx] = HamtEntry::Node(new_child);
                        heap.alloc(HeapObject::HamtNode {
                            bitmap,
                            children: new_children,
                        })
                    }
                    HamtEntry::Collision(col) => {
                        let col = *col;
                        let col_data = heap.get(col).clone();
                        match col_data {
                            HeapObject::HamtCollision {
                                hash: col_hash,
                                mut entries,
                            } => {
                                // Update existing or add new entry
                                if let Some(pos) = entries.iter().position(|(k, _)| *k == key) {
                                    entries[pos] = (key, value);
                                } else {
                                    entries.push((key, value));
                                }

                                let new_col = heap.alloc(HeapObject::HamtCollision {
                                    hash: col_hash,
                                    entries,
                                });
                                new_children[idx] = HamtEntry::Collision(new_col);
                                heap.alloc(HeapObject::HamtNode {
                                    bitmap,
                                    children: new_children,
                                })
                            }
                            _ => unreachable!("Collision entry points to non-collision node"),
                        }
                    }
                }
            }
        }
        HeapObject::HamtCollision {
            hash: col_hash,
            mut entries,
        } => {
            // Inserting into a collision node
            if let Some(pos) = entries.iter().position(|(k, _)| *k == key) {
                entries[pos] = (key, value);
            } else {
                entries.push((key, value));
            }
            heap.alloc(HeapObject::HamtCollision {
                hash: col_hash,
                entries,
            })
        }
        _ => {
            // Not a HAMT node — shouldn't happen, but create a new leaf
            let mut root = hamt_empty(heap);
            root = hamt_insert_at(heap, node, key, value, hash, depth);
            root
        }
    }
}

/// Deletes a key from a HAMT, returning a new root handle.
pub fn hamt_delete(heap: &mut GcHeap, root: GcHandle, key: &HashKey) -> GcHandle {
    let hash = hash_key(key);
    hamt_delete_at(heap, root, key, hash, 0)
}

pub fn hamt_delete_at(
    heap: &mut GcHeap,
    node: GcHandle,
    key: &HashKey,
    hash: u64,
    depth: u32,
) -> GcHandle {
    let node_data = heap.get(node).clone();

    match node_data {
        HeapObject::HamtNode { bitmap, children } => {
            let slot = slot_at_depth(hash, depth);
            let bit = 1u32 << slot;

            if bitmap & bit == 0 {
                return node; // Key not present, no change
            }

            let idx = compressed_index(bitmap, slot);
            let mut new_children = children;

            match &new_children[idx] {
                HamtEntry::Leaf(k, _) => {
                    if k != key {
                        return node; // Different key, no change
                    }

                    // Remove this leaf
                    new_children.remove(idx);
                    let new_bitmap = bitmap & !bit;
                    heap.alloc(HeapObject::HamtNode {
                        bitmap: new_bitmap,
                        children: new_children,
                    })
                }
                HamtEntry::Node(child) => {
                    let child = *child;
                    let new_child = hamt_delete_at(heap, child, key, hash, depth + 1);
                    // Check if child became empty
                    match heap.get(new_child) {
                        HeapObject::HamtNode {
                            bitmap: cb,
                            children: cc,
                        } => {
                            if *cb == 0 && cc.is_empty() {
                                // Child is empty, remove the slot
                                new_children.remove(idx);
                                let new_newbitmap = bitmap & !bit;
                                heap.alloc(HeapObject::HamtNode {
                                    bitmap: new_newbitmap,
                                    children: new_children,
                                })
                            } else if cc.len() == 1 {
                                // Child has single entry, pull it up
                                let entry = cc[0].clone();
                                new_children[idx] = entry;
                                heap.alloc(HeapObject::HamtNode {
                                    bitmap,
                                    children: new_children,
                                })
                            } else {
                                new_children[idx] = HamtEntry::Node(new_child);
                                heap.alloc(HeapObject::HamtNode {
                                    bitmap,
                                    children: new_children,
                                })
                            }
                        }
                        _ => {
                            new_children[idx] = HamtEntry::Node(new_child);
                            heap.alloc(HeapObject::HamtNode {
                                bitmap,
                                children: new_children,
                            })
                        }
                    }
                }
                HamtEntry::Collision(col) => {
                    let col = *col;
                    let col_data = heap.get(col).clone();

                    match col_data {
                        HeapObject::HamtCollision {
                            hash: col_hash,
                            mut entries,
                        } => {
                            if let Some(pos) = entries.iter().position(|(k, _)| k == key) {
                                entries.remove(pos);
                                if entries.len() == 1 {
                                    // Convert back to leaf
                                    let (k, v) = entries.remove(0);
                                    new_children[idx] = HamtEntry::Leaf(k, v);
                                    heap.alloc(HeapObject::HamtNode {
                                        bitmap,
                                        children: new_children,
                                    })
                                } else {
                                    let new_col = heap.alloc(HeapObject::HamtCollision {
                                        hash: col_hash,
                                        entries,
                                    });
                                    new_children[idx] = HamtEntry::Collision(new_col);
                                    heap.alloc(HeapObject::HamtNode {
                                        bitmap,
                                        children: new_children,
                                    })
                                }
                            } else {
                                node // Key not in collision, no change
                            }
                        }
                        _ => node,
                    }
                }
            }
        }
        HeapObject::HamtCollision {
            hash: col_hash,
            mut entries,
        } => {
            if let Some(pos) = entries.iter().position(|(k, _)| k == key) {
                entries.remove(pos);
                if entries.len() == 1 {
                    let (k, v) = entries.remove(0);
                    // Create a node with just this leaf
                    let slot = slot_at_depth(col_hash, depth);
                    let bitmap = 1u32 << slot;
                    heap.alloc(HeapObject::HamtNode {
                        bitmap,
                        children: vec![HamtEntry::Leaf(k, v)],
                    })
                } else {
                    heap.alloc(HeapObject::HamtCollision {
                        hash: col_hash,
                        entries,
                    })
                }
            } else {
                node
            }
        }
        _ => node,
    }
}

fn hamt_iter_collect(heap: &GcHeap, handle: GcHandle, result: &mut Vec<(HashKey, Value)>) {
    match heap.get(handle) {
        HeapObject::HamtNode { children, .. } => {
            for entry in children {
                match entry {
                    HamtEntry::Leaf(k, v) => result.push((k.clone(), v.clone())),
                    HamtEntry::Node(child) => hamt_iter_collect(heap, *child, result),
                    HamtEntry::Collision(col) => hamt_iter_collect(heap, *col, result),
                }
            }
        }
        HeapObject::HamtCollision { entries, .. } => {
            for (k, v) in entries {
                result.push((k.clone(), v.clone()));
            }
        }
        _ => {}
    }
}

pub fn hamt_iter(heap: &GcHeap, root: GcHandle) -> Vec<(HashKey, Value)> {
    let mut result = Vec::new();
    hamt_iter_collect(heap, root, &mut result);
    result
}

fn hamt_count(heap: &GcHeap, handle: GcHandle) -> usize {
    match heap.get(handle) {
        HeapObject::HamtNode { children, .. } => {
            let mut count = 0;
            for entry in children {
                match entry {
                    HamtEntry::Leaf(_, _) => count += 1,
                    HamtEntry::Node(child) => count += hamt_count(heap, *child),
                    HamtEntry::Collision(col) => count += hamt_count(heap, *col),
                }
            }
            count
        }
        HeapObject::HamtCollision { entries, .. } => entries.len(),
        _ => 0,
    }
}

pub fn hamt_len(heap: &GcHeap, root: GcHandle) -> usize {
    hamt_count(heap, root)
}

pub fn is_hamt(heap: &GcHeap, handle: GcHandle) -> bool {
    matches!(
        heap.get(handle),
        HeapObject::HamtNode { .. } | HeapObject::HamtCollision { .. }
    )
}

/// Deep equality comparison of two HAMT trees.
/// Two maps are equal if they have the same key-value pairs.
pub fn hamt_equal(heap: &GcHeap, a: GcHandle, b: GcHandle) -> bool {
    if a == b {
        return true;
    }

    let pairs_a = hamt_iter(heap, a);
    let pairs_b = hamt_iter(heap, b);

    if pairs_a.len() != pairs_b.len() {
        return false;
    }

    // Check every key-value pair from a exists in b
    for (ka, va) in &pairs_a {
        match hamt_lookup(heap, b, ka) {
            Some(vb) if vb == *va => {}
            _ => return false,
        }
    }
    true
}

/// Format a HAMT as a string like `{"a": 1, "b": 2}`.
pub fn format_hamt(heap: &GcHeap, root: GcHandle) -> String {
    let pairs = hamt_iter(heap, root);
    let items: Vec<String> = pairs.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
    format!("{{{}}}", items.join(", "))
}
