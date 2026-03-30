//! Rc-based HAMT (Hash Array Mapped Trie) for persistent maps.
//!
//! This is the Aether Phase 3 replacement for `gc::hamt`, using `Rc` instead of
//! GcHeap allocation. All operations are heap-free (no GcHandle/GcHeap needed).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use crate::runtime::{hash_key::HashKey, value::Value};

/// Bits consumed per HAMT level.
const BITS_PER_LEVEL: u32 = 5;
/// Maximum depth (64-bit hash / 5 bits per level = 12.8, round up).
const MAX_DEPTH: u32 = 13;

#[derive(Debug, Clone, PartialEq)]
pub struct HamtNode {
    pub bitmap: u32,
    pub children: Vec<HamtEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HamtCollision {
    pub hash: u64,
    pub entries: Vec<(HashKey, Value)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HamtEntry {
    Leaf(HashKey, Value),
    Node(Rc<HamtNode>),
    Collision(Rc<HamtCollision>),
}

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

/// Creates an empty HAMT root node.
pub fn hamt_empty() -> Rc<HamtNode> {
    Rc::new(HamtNode {
        bitmap: 0,
        children: Vec::new(),
    })
}

/// Looks up a key in a HAMT.
pub fn hamt_lookup(root: &HamtNode, key: &HashKey) -> Option<Value> {
    let hash = hash_key(key);
    let mut current: &HamtNode = root;
    let mut depth = 0u32;

    loop {
        let slot = slot_at_depth(hash, depth);
        let bit = 1u32 << slot;

        if current.bitmap & bit == 0 {
            return None;
        }

        let idx = compressed_index(current.bitmap, slot);

        match &current.children[idx] {
            HamtEntry::Leaf(k, v) => {
                if k == key {
                    return Some(v.clone());
                }
                return None;
            }
            HamtEntry::Node(child) => {
                current = child;
                depth += 1;
            }
            HamtEntry::Collision(col) => {
                for (k, v) in &col.entries {
                    if k == key {
                        return Some(v.clone());
                    }
                }
                return None;
            }
        }
    }
}

/// Inserts a key-value pair into a HAMT, returning a new root.
/// The original HAMT is not modified.
pub fn hamt_insert(root: &Rc<HamtNode>, key: HashKey, value: Value) -> Rc<HamtNode> {
    let hash = hash_key(&key);
    hamt_insert_at(root, key, value, hash, 0)
}

fn hamt_insert_at(
    node: &Rc<HamtNode>,
    key: HashKey,
    value: Value,
    hash: u64,
    depth: u32,
) -> Rc<HamtNode> {
    let slot = slot_at_depth(hash, depth);
    let bit = 1u32 << slot;
    let idx = compressed_index(node.bitmap, slot);

    if node.bitmap & bit == 0 {
        // Slot is empty -- insert a new leaf
        let mut new_children = node.children.clone();
        new_children.insert(idx, HamtEntry::Leaf(key, value));
        Rc::new(HamtNode {
            bitmap: node.bitmap | bit,
            children: new_children,
        })
    } else {
        // Slot is occupied
        let mut new_children = node.children.clone();

        match &new_children[idx] {
            HamtEntry::Leaf(existing_key, existing_value) => {
                if *existing_key == key {
                    // Update existing key
                    new_children[idx] = HamtEntry::Leaf(key, value);
                    Rc::new(HamtNode {
                        bitmap: node.bitmap,
                        children: new_children,
                    })
                } else {
                    // Hash collision at this level -- push down
                    let existing_hash = hash_key(existing_key);
                    let ek = existing_key.clone();
                    let ev = existing_value.clone();

                    if depth + 1 >= MAX_DEPTH {
                        // At max depth, create a collision node
                        let col = Rc::new(HamtCollision {
                            hash,
                            entries: vec![(ek, ev), (key, value)],
                        });
                        new_children[idx] = HamtEntry::Collision(col);
                        Rc::new(HamtNode {
                            bitmap: node.bitmap,
                            children: new_children,
                        })
                    } else {
                        // Create a sub-node and insert both entries
                        let empty = Rc::new(HamtNode {
                            bitmap: 0,
                            children: Vec::new(),
                        });
                        let sub = hamt_insert_at(&empty, ek, ev, existing_hash, depth + 1);
                        let sub = hamt_insert_at(&sub, key, value, hash, depth + 1);
                        new_children[idx] = HamtEntry::Node(sub);
                        Rc::new(HamtNode {
                            bitmap: node.bitmap,
                            children: new_children,
                        })
                    }
                }
            }
            HamtEntry::Node(child) => {
                let new_child = hamt_insert_at(child, key, value, hash, depth + 1);
                new_children[idx] = HamtEntry::Node(new_child);
                Rc::new(HamtNode {
                    bitmap: node.bitmap,
                    children: new_children,
                })
            }
            HamtEntry::Collision(col) => {
                let mut entries = col.entries.clone();
                if let Some(pos) = entries.iter().position(|(k, _)| *k == key) {
                    entries[pos] = (key, value);
                } else {
                    entries.push((key, value));
                }
                let new_col = Rc::new(HamtCollision {
                    hash: col.hash,
                    entries,
                });
                new_children[idx] = HamtEntry::Collision(new_col);
                Rc::new(HamtNode {
                    bitmap: node.bitmap,
                    children: new_children,
                })
            }
        }
    }
}

/// Deletes a key from a HAMT, returning a new root.
pub fn hamt_delete(root: &Rc<HamtNode>, key: &HashKey) -> Rc<HamtNode> {
    let hash = hash_key(key);
    hamt_delete_at(root, key, hash, 0)
}

fn hamt_delete_at(node: &Rc<HamtNode>, key: &HashKey, hash: u64, depth: u32) -> Rc<HamtNode> {
    let slot = slot_at_depth(hash, depth);
    let bit = 1u32 << slot;

    if node.bitmap & bit == 0 {
        return Rc::clone(node); // Key not present
    }

    let idx = compressed_index(node.bitmap, slot);
    let mut new_children = node.children.clone();

    match &new_children[idx] {
        HamtEntry::Leaf(k, _) => {
            if k != key {
                return Rc::clone(node); // Different key
            }
            // Remove this leaf
            new_children.remove(idx);
            let new_bitmap = node.bitmap & !bit;
            Rc::new(HamtNode {
                bitmap: new_bitmap,
                children: new_children,
            })
        }
        HamtEntry::Node(child) => {
            let new_child = hamt_delete_at(child, key, hash, depth + 1);
            if new_child.bitmap == 0 && new_child.children.is_empty() {
                // Child is empty, remove the slot
                new_children.remove(idx);
                let new_bitmap = node.bitmap & !bit;
                Rc::new(HamtNode {
                    bitmap: new_bitmap,
                    children: new_children,
                })
            } else if new_child.children.len() == 1 {
                // Child has single entry, pull it up
                let entry = new_child.children[0].clone();
                new_children[idx] = entry;
                Rc::new(HamtNode {
                    bitmap: node.bitmap,
                    children: new_children,
                })
            } else {
                new_children[idx] = HamtEntry::Node(new_child);
                Rc::new(HamtNode {
                    bitmap: node.bitmap,
                    children: new_children,
                })
            }
        }
        HamtEntry::Collision(col) => {
            let mut entries = col.entries.clone();
            if let Some(pos) = entries.iter().position(|(k, _)| k == key) {
                entries.remove(pos);
                if entries.len() == 1 {
                    // Convert back to leaf
                    let (k, v) = entries.remove(0);
                    new_children[idx] = HamtEntry::Leaf(k, v);
                    Rc::new(HamtNode {
                        bitmap: node.bitmap,
                        children: new_children,
                    })
                } else {
                    let new_col = Rc::new(HamtCollision {
                        hash: col.hash,
                        entries,
                    });
                    new_children[idx] = HamtEntry::Collision(new_col);
                    Rc::new(HamtNode {
                        bitmap: node.bitmap,
                        children: new_children,
                    })
                }
            } else {
                Rc::clone(node) // Key not in collision
            }
        }
    }
}

fn hamt_iter_collect(node: &HamtNode, result: &mut Vec<(HashKey, Value)>) {
    for entry in &node.children {
        match entry {
            HamtEntry::Leaf(k, v) => result.push((k.clone(), v.clone())),
            HamtEntry::Node(child) => hamt_iter_collect(child, result),
            HamtEntry::Collision(col) => {
                for (k, v) in &col.entries {
                    result.push((k.clone(), v.clone()));
                }
            }
        }
    }
}

/// Collects all key-value pairs from a HAMT into a Vec.
pub fn hamt_iter(root: &HamtNode) -> Vec<(HashKey, Value)> {
    let mut result = Vec::new();
    hamt_iter_collect(root, &mut result);
    result
}

fn hamt_count(node: &HamtNode) -> usize {
    let mut count = 0;
    for entry in &node.children {
        match entry {
            HamtEntry::Leaf(_, _) => count += 1,
            HamtEntry::Node(child) => count += hamt_count(child),
            HamtEntry::Collision(col) => count += col.entries.len(),
        }
    }
    count
}

/// Returns the number of key-value pairs in a HAMT.
pub fn hamt_len(root: &HamtNode) -> usize {
    hamt_count(root)
}

/// Deep equality comparison of two HAMT trees.
/// Two maps are equal if they have the same key-value pairs.
pub fn hamt_equal(a: &HamtNode, b: &HamtNode) -> bool {
    if std::ptr::eq(a, b) {
        return true;
    }

    let pairs_a = hamt_iter(a);
    let pairs_b = hamt_iter(b);

    if pairs_a.len() != pairs_b.len() {
        return false;
    }

    for (ka, va) in &pairs_a {
        match hamt_lookup(b, ka) {
            Some(vb) if vb == *va => {}
            _ => return false,
        }
    }
    true
}

/// Format a HAMT as a string like `{"a": 1, "b": 2}`.
pub fn format_hamt(root: &HamtNode) -> String {
    let mut pairs = hamt_iter(root);
    pairs.sort_by(|(a, _), (b, _)| a.to_string().cmp(&b.to_string()));
    let items: Vec<String> = pairs.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
    format!("{{{}}}", items.join(", "))
}
