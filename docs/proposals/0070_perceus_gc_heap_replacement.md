- Feature Name: Replace GcHandle with Perceus-managed Persistent Structures
- Start Date: 2026-03-01
- Status: Not Implemented
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0070: Replace GcHandle with Perceus-managed Persistent Structures

## Summary
[summary]: #summary

Eliminate `Value::Gc(GcHandle)` and the global `GcHeap` by replacing cons lists and HAMT
maps with `Rc`-based persistent data structures that participate in the Perceus uniqueness
analysis (proposal 0068). This removes the last global mutable state from the runtime,
makes all `Value` variants actor-sendable (after `SendableValue` conversion), and
eliminates the need for a separate mark-and-sweep GC cycle.

## Motivation
[motivation]: #motivation

`Value::Gc(GcHandle)` is Flux's primary obstacle to a clean actor model and a clean
memory model:

1. **Cannot cross actor boundaries** (proposal 0067): `GcHandle` is a `u32` index into
   a global `GcHeap` that lives on one thread. Sending it to another actor is UB.
2. **Global mutable state**: `GcHeap` is a shared `Vec<Option<HeapEntry>>` modified by
   every cons or HAMT allocation. It is incompatible with per-actor GC.
3. **Separate GC cycle**: the mark-and-sweep over `GcHeap` is a distinct mechanism from
   `Rc` drop. Two memory management systems in one runtime is unnecessary complexity.
4. **Display bug**: `Value::Gc` shows `<gc@N>` instead of the value's content because
   display requires heap access (proposal 0045 workaround).

Replacing `GcHandle` with `Rc<ConsCell>` and `Rc<HamtNode>` unifies all memory
management under `Rc`, participates in Perceus, and eliminates the `GcHeap` entirely.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Before: `Value::Gc(GcHandle(u32))`

```flux
let xs = list(1, 2, 3)
-- xs is Value::Gc(GcHandle(42))
-- The cons cell lives in GcHeap.entries[42]
-- GC runs: heap scanned, marked, swept
-- Cons display: <gc@42> (broken without heap reference)
```

### After: `Value::ConsList(Rc<ConsList>)` and `Value::HamtMap(Rc<HamtNode>)`

```flux
let xs = list(1, 2, 3)
-- xs is Value::ConsList(Rc<ConsList { head: 1, tail: ConsList { head: 2, ... } }>)
-- Memory managed by Rc drop (no GC needed)
-- Perceus: if xs is uniquely owned, list ops can reuse the cons cells
-- Display: [1, 2, 3] (direct access via Rc)
-- Actor sendable: deep copy through SendableValue works (same as Array)
```

### No change to Flux surface syntax

```flux
-- These work identically before and after:
let xs = list(1, 2, 3)
let ys = [1 | [2 | [3 | []]]]
let h  = hd(xs)
let t  = tl(xs)
let m  = put({}, "key", 42)
let v  = get(m, "key")
```

### GC telemetry changes

The `--gc-telemetry` flag and related metrics become meaningless after this change (no
GC cycles occur for cons/HAMT data). The flag is retained but reports zero collections.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### New Value variants

```rust
// src/runtime/value.rs

pub enum Value {
    // ... existing variants (Integer, Float, Boolean, String, None, EmptyList,
    //     Some, Left, Right, Array, Tuple, Function, Closure, Adt, etc.) ...

    // NEW: replaces Value::Gc for cons lists
    ConsList(Rc<ConsList>),

    // NEW: replaces Value::Gc for HAMT maps
    HamtMap(Rc<HamtNode>),

    // REMOVED: Value::Gc(GcHandle) — deleted after migration
    // Gc(GcHandle),
}

/// Persistent cons cell (replaces HeapObject::Cons in GcHeap)
pub struct ConsList {
    pub head: Value,
    pub tail: Value,   // Either ConsList or Value::EmptyList
}

/// HAMT trie node (replaces HeapObject::HamtNode in GcHeap)
/// Identical structure to current HamtNode but stored in Rc, not GcHeap slot.
pub struct HamtNode {
    pub bitmap: u32,
    pub children: Vec<HamtEntry>,
}

pub enum HamtEntry {
    Value(HashKey, Value),
    Node(Rc<HamtNode>),
    Collision(Rc<HamtCollision>),
}

pub struct HamtCollision {
    pub hash: u64,
    pub entries: Vec<(HashKey, Value)>,
}
```

### Migration plan

The migration is done in five atomic steps to keep tests passing throughout:

**Step 1**: Add `ConsList` and `HamtNode` structs alongside the existing `GcHeap` code.
No behavior change; just the new types.

**Step 2**: Add `Value::ConsList` and `Value::HamtMap` variants. The existing `Value::Gc`
is still present. All code still routes through `GcHandle`.

**Step 3**: Redirect `list()`, `hd()`, `tl()`, and related operations to use `ConsList`
for new allocations. Existing `Value::Gc` still works via the old path. Add pattern
matching for both in all display and operation code.

**Step 4**: Redirect HAMT operations (`put()`, `get()`, `has_key()`, `keys()`, etc.) to
use `HamtMap`.

**Step 5**: Remove `Value::Gc`, `GcHandle`, `GcHeap`, and the mark-and-sweep collector.
Delete `src/runtime/gc/`. All tests must pass.

### Step 3 in detail: cons list implementation

```rust
// src/runtime/base/list_ops.rs

/// Build a cons cell: [head | tail]
pub fn make_cons(head: Value, tail: Value) -> Value {
    Value::ConsList(Rc::new(ConsList { head, tail }))
}

/// base_hd: returns the head of a cons list
pub fn base_hd(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    match args.into_iter().next() {
        Some(Value::ConsList(cell)) => Ok(cell.head.clone()),
        Some(Value::EmptyList) => Err("hd: empty list".to_string()),
        other => Err(format!("hd: expected a list, got {:?}", other)),
    }
}

/// base_tl: returns the tail of a cons list
pub fn base_tl(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    match args.into_iter().next() {
        Some(Value::ConsList(cell)) => Ok(cell.tail.clone()),
        Some(Value::EmptyList) => Err("tl: empty list".to_string()),
        other => Err(format!("tl: expected a list, got {:?}", other)),
    }
}

/// base_list: construct a list from arguments
/// list(1, 2, 3) → ConsList(1, ConsList(2, ConsList(3, EmptyList)))
pub fn base_list(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let mut result = Value::EmptyList;
    for item in args.into_iter().rev() {
        result = make_cons(item, result);
    }
    Ok(result)
}

/// base_to_list: convert array to cons list
pub fn base_to_list(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    match args.into_iter().next() {
        Some(Value::Array(arr)) => {
            let mut result = Value::EmptyList;
            for item in arr.iter().rev() {
                result = make_cons(item.clone(), result);
            }
            Ok(result)
        }
        other => Err(format!("to_list: expected Array, got {:?}", other)),
    }
}

/// Display a cons list as [1, 2, 3]
pub fn format_cons_list(cell: &Rc<ConsList>) -> String {
    let mut items = Vec::new();
    let mut cur: &Value = &Value::ConsList(Rc::clone(cell));
    loop {
        match cur {
            Value::ConsList(c) => {
                items.push(format!("{}", c.head));
                cur = &c.tail;
            }
            Value::EmptyList => break,
            other => {
                items.push(format!("| {}", other));
                break;
            }
        }
    }
    format!("[{}]", items.join(", "))
}
```

### Step 4 in detail: HAMT map implementation

```rust
// src/runtime/base/hash_ops.rs

/// base_put: insert a key-value pair into a HAMT map
pub fn base_put(_ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let mut it = args.into_iter();
    let map_val = it.next().ok_or("put: missing map")?;
    let key_val = it.next().ok_or("put: missing key")?;
    let val_val = it.next().ok_or("put: missing value")?;

    let key = HashKey::from_value(&key_val)?;
    let hash = compute_hash(&key);

    let new_root = match map_val {
        Value::HamtMap(root) => hamt_insert(root, key, val_val, hash, 0),
        // Empty map (represented as Value::None or a specific empty HamtMap)
        Value::None => {
            hamt_singleton(key, val_val, hash)
        }
        other => return Err(format!("put: expected a map, got {:?}", other)),
    };

    Ok(Value::HamtMap(Rc::new(new_root)))
}

/// Persistent HAMT insert with structural sharing.
/// Returns a new root node; the old tree is unchanged (Rc-shared).
fn hamt_insert(
    node: Rc<HamtNode>,
    key: HashKey,
    value: Value,
    hash: u64,
    depth: usize,
) -> HamtNode {
    let bit = (hash >> (depth * 5)) & 0x1F;
    let mask = 1u32 << bit;

    if node.bitmap & mask == 0 {
        // Slot is empty: insert here
        let pos = (node.bitmap & (mask - 1)).count_ones() as usize;
        let mut new_children = node.children.clone();  // Rc::clone for shared children
        new_children.insert(pos, HamtEntry::Value(key, value));
        HamtNode { bitmap: node.bitmap | mask, children: new_children }
    } else {
        // Slot is occupied: recurse
        let pos = (node.bitmap & (mask - 1)).count_ones() as usize;
        let mut new_children = node.children.clone();
        new_children[pos] = match &node.children[pos] {
            HamtEntry::Value(existing_key, existing_val) => {
                if *existing_key == key {
                    // Update in place (new value for same key)
                    HamtEntry::Value(key, value)
                } else {
                    // Collision: create sub-node
                    let sub = hamt_two_entry(
                        existing_key.clone(), existing_val.clone(),
                        key, value,
                        hash, compute_hash(existing_key),
                        depth + 1,
                    );
                    HamtEntry::Node(Rc::new(sub))
                }
            }
            HamtEntry::Node(child) => {
                let new_child = hamt_insert(Rc::clone(child), key, value, hash, depth + 1);
                HamtEntry::Node(Rc::new(new_child))
            }
            HamtEntry::Collision(coll) => {
                let mut new_entries = (*coll.entries).clone();
                if let Some(pos) = new_entries.iter().position(|(k, _)| k == &key) {
                    new_entries[pos].1 = value;
                } else {
                    new_entries.push((key, value));
                }
                HamtEntry::Collision(Rc::new(HamtCollision {
                    hash: coll.hash,
                    entries: new_entries,
                }))
            }
        };
        HamtNode { bitmap: node.bitmap, children: new_children }
    }
}
```

### Step 5: deleting GcHeap

Files to delete:
- `src/runtime/gc/gc_heap.rs`
- `src/runtime/gc/gc_handle.rs`
- `src/runtime/gc/heap_object.rs`
- `src/runtime/gc/hamt.rs`
- `src/runtime/gc/mod.rs`

References to remove from:
- `src/runtime/vm/mod.rs`: `self.heap: GcHeap`, `heap.collect(...)`, `heap.alloc(...)`
- `src/runtime/vm/dispatch.rs`: `OpGcAlloc`, any GcHeap allocation calls
- `src/main.rs`: `--gc-threshold`, `--no-gc` flags (can be kept as no-ops with deprecation warning)
- `Cargo.toml`: remove `gc-telemetry` feature gate if desired

### SendableValue extension

After this proposal, `ConsList` and `HamtMap` can be encoded in `SendableValue`:

```rust
// src/runtime/actor/sendable.rs — additions after 0070

pub enum SendableValue {
    // ... existing variants ...

    /// Sendable cons list: deeply copied
    ConsList(Arc<Vec<SendableValue>>),  // flattened representation

    /// Sendable HAMT map: deeply copied as Vec of key-value pairs
    HamtMap(Arc<Vec<(SendableValue, SendableValue)>>),
}

impl SendableValue {
    pub fn from_value(v: &Value) -> Result<Self, SendError> {
        match v {
            // ... existing arms ...

            Value::ConsList(cell) => {
                let mut items = Vec::new();
                let mut cur: &Value = v;
                loop {
                    match cur {
                        Value::ConsList(c) => {
                            items.push(Self::from_value(&c.head)?);
                            cur = &c.tail;
                        }
                        Value::EmptyList => break,
                        other => {
                            // Improper list: encode tail as last element
                            items.push(Self::from_value(other)?);
                            break;
                        }
                    }
                }
                Ok(Self::ConsList(Arc::new(items)))
            }

            // Value::Gc(_) arm is removed — no longer exists
        }
    }
}
```

This also resolves proposal 0067: `E1005` is no longer needed for cons lists and HAMT maps
because they are now sendable. The error only remains for any remaining non-sendable types.

### Test validation

```bash
# After each migration step, run full test suite
cargo test

# Verify cons list display is correct
cargo run -- --no-cache examples/basics/lists.flx

# Verify HAMT map display is correct
cargo run -- --no-cache examples/basics/maps.flx

# Verify actor send of cons list works (after 0070 completes 0067's fix)
cargo run -- --no-cache --root lib/ examples/actors/send_list.flx

# Verify no GC heap allocation (--gc-telemetry should show 0 collections)
cargo run -- --no-cache --gc-telemetry examples/basics/lists.flx
```

## Drawbacks
[drawbacks]: #drawbacks

- This is a significant refactoring across the entire runtime. Risk of regressions.
  Mitigated by the five-step incremental migration.
- `Rc<ConsList>` chains have worse cache locality than a contiguous array. For small
  lists, this may be slower than the current `GcHeap` slab. Mitigated by Perceus reuse
  on uniquely-owned lists.
- The `--gc-threshold`, `--no-gc`, and `--gc-telemetry` flags become no-ops. Users who
  rely on GC tuning for performance must be notified.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why not keep GcHeap and make it per-actor?** Making `GcHeap` per-actor requires copying
cons lists and HAMT maps on actor spawn (to move them to the new heap) and on `send`.
The copy cost is the same as the `Rc<ConsList>` approach, but the GcHeap adds a
separate GC mechanism. The `Rc` approach is simpler and eliminates the GC.

**Why not use a bump allocator for cons cells?** A bump allocator requires a GC to
reclaim. The whole point of this proposal is to eliminate the GC. `Rc` with Perceus is
the correct replacement.

**Why Rc not Arc?** Actors run one Flux VM each. `Rc` is sufficient within one actor.
`SendableValue` handles the cross-actor boundary with deep copy.

## Prior art
[prior-art]: #prior-art

- **Koka's persistent data structures**: Koka uses Perceus-managed linked data structures
  without a separate GC. The `ConsList` design here mirrors Koka's approach.
- **OCaml's GC**: OCaml uses a generational GC for lists. Flux's choice of `Rc` + Perceus
  is simpler for a single-threaded per-actor model.
- **Proposal 0017** — earlier Flux proposal for persistent collections (superseded by this).
- **Proposal 0045** — earlier GC proposal (superseded by this).

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should the empty map be represented as `Value::None` (reusing the existing variant) or
   a dedicated `Value::EmptyMap`? Decision: a sentinel `Rc<HamtNode>` with `bitmap=0`
   and `children=[]`. Avoids overloading `None`.
2. After removing `GcHeap`, should the `--gc-threshold` and `--no-gc` flags be removed
   or retained as no-ops with a deprecation warning? Decision: retain as no-ops with a
   single deprecation line at startup.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Perceus for cons cells** (proposal 0069 extension): when a cons cell is uniquely
  owned at a pattern match site, the cell can be reused in-place for the new head value.
  This makes `map` over cons lists as fast as in-place mutation.
- **Typed persistent collections**: `List<a>` and `Map<k, v>` as typed versions of the
  current untyped cons list and HAMT, enabled by the type system work in 0032/0042.
- **Finger trees**: for efficient `O(log n)` append and split on sequences, as an
  alternative to cons lists for random-access-heavy workloads.
