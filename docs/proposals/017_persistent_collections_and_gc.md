# Proposal 017: Persistent Collections and Garbage Collection

**Status:** Proposed
**Priority:** High (Runtime)
**Created:** 2026-02-08
**Related:** Proposal 010 (GC), Proposal 016 (Tail-Call Optimization), Proposal 019 (Zero-Copy Value Passing)
**Implementation Order:** 019 → 016 → 017 (this) → 018

---

## Overview

Introduce GC-managed persistent data structures — a cons-cell linked List and a Hash Array Mapped Trie (HAMT) Map — to Flux. Persistent structures share subtrees across versions, eliminating the O(n) full-clone cost on every mutating operation. A stop-the-world mark-and-sweep garbage collector manages the lifetime of shared heap nodes.

The design follows Elixir's collection model: List for sequential/recursive processing with O(1) prepend and head/tail; Map for associative lookup with O(log32 n) insert/lookup/delete and structural sharing. Both coexist with the existing Array type.

---

## Goals

1. Provide an O(1) prepend/head/tail persistent List with `[head | tail]` syntax and pattern matching.
2. Provide an O(log32 n) persistent Map backed by a HAMT that replaces the current `Hash(HashMap)`.
3. Implement a stop-the-world mark-and-sweep GC to manage heap-allocated shared nodes.
4. Preserve full backward compatibility for Array syntax and builtins.

### Non-Goals

1. Concurrent, incremental, or generational GC.
2. Moving or compacting GC.
3. Removing the Array type.
4. Lazy sequences or streams.
5. Transient (mutable builder) variants of persistent collections.

---

## Problem Statement

### Problem 1: O(n) Clone on Every Collection Operation

Every `push`, `concat`, `merge`, `rest`, `reverse`, and `sort` clones the entire backing `Vec` or `HashMap`:

```rust
// array_ops.rs — builtin_push
let mut new_arr = arr.clone();  // O(n) clone
new_arr.push(args[1].clone());

// hash_ops.rs — builtin_merge
let mut result = h1.clone();    // O(n) clone
for (k, v) in h2.iter() {
    result.insert(k.clone(), v.clone());
}
```

An accumulator loop calling `push` n times incurs O(n^2) total work.

### Problem 2: No Idiomatic Recursive List Processing

`rest(arr)` clones the entire tail — O(n) per call:

```rust
// array_ops.rs — builtin_rest
Ok(Object::Array(arr[1..].to_vec()))  // O(n) copy
```

A linked list makes `hd` and `tl` O(1) by following a pointer. Pattern matching on `[head | tail]` becomes a zero-copy destructure.

### Problem 3: Shared Nodes Require GC

With persistent data structures, multiple collection versions share internal nodes forming a DAG. When closures capture collections, reachability can become arbitrarily complex. `Rc` handles DAGs but not potential cycles through closure capture chains. A mark-and-sweep GC cleanly manages all heap-allocated nodes.

---

## Proposed Design

### Phase 1: GC Infrastructure

#### 1.1 GC Heap

New module `src/runtime/gc/`:

```rust
/// Opaque handle into the GC heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GcHandle(u32);

/// What lives on the GC heap.
pub enum HeapObject {
    Cons { head: Object, tail: Object },
    HamtNode { bitmap: u32, children: Vec<HamtEntry> },
    HamtCollision { hash: u64, entries: Vec<(HashKey, Object)> },
}

struct HeapEntry {
    object: HeapObject,
    marked: bool,
}

pub struct GcHeap {
    entries: Vec<Option<HeapEntry>>,
    free_list: Vec<u32>,
    allocation_count: usize,
    gc_threshold: usize,
}
```

#### 1.2 GcHeap API

```rust
impl GcHeap {
    pub fn alloc(&mut self, object: HeapObject, roots: &RootSet) -> GcHandle;
    pub fn get(&self, handle: GcHandle) -> &HeapObject;
    pub fn collect(&mut self, roots: &RootSet);
    pub fn live_count(&self) -> usize;
}
```

`alloc()` triggers `collect()` when `allocation_count >= gc_threshold`.

#### 1.3 Root Set

```rust
pub struct RootSet<'a> {
    pub stack: &'a [Object],
    pub sp: usize,
    pub globals: &'a [Object],
    pub frames: &'a [Frame],
    pub constants: &'a [Object],
}
```

#### 1.4 Mark-and-Sweep

```
MARK:
  for each root in root_set:
      mark_object(root)

  fn mark_object(obj):
      if obj is Gc(handle) and not heap[handle].marked:
          heap[handle].marked = true
          match heap[handle]:
              Cons { head, tail } → mark(head), mark(tail)
              HamtNode { children } → mark each child recursively
              HamtCollision { entries } → mark each value
      if obj is Some/Left/Right(inner): mark(inner)
      if obj is Array(elems): mark each element
      if obj is Closure(c): mark each free variable

SWEEP:
  for each entry in heap:
      if marked: clear mark
      else: free entry, add to free_list
```

#### 1.5 Trigger Strategy

- Default threshold: 10,000 allocations.
- Adaptive: doubles if <25% collected, halves if >75% collected.
- `--gc-threshold N` overrides default.
- `--no-gc` disables automatic collection.

#### 1.6 Object Enum Changes

```rust
pub enum Object {
    Integer(i64), Float(f64), Boolean(bool), String(String),
    None,
    Some(Box<Object>), Left(Box<Object>), Right(Box<Object>),
    ReturnValue(Box<Object>),
    Function(Rc<CompiledFunction>),
    Closure(Rc<Closure>),
    Builtin(BuiltinFunction),
    Array(Vec<Object>),
    Gc(GcHandle),               // NEW — gateway to all heap-managed types
}
```

`Object::Hash(HashMap)` is removed in Phase 3. `Gc(GcHandle)` replaces it.

#### 1.7 VM Integration

```rust
pub struct VM {
    // ... existing fields ...
    gc_heap: GcHeap,   // NEW
}
```

---

### Phase 2: Persistent List (Cons Cells)

#### 2.1 Representation

```
Object::None                           — empty list []
Object::Gc(h) → Cons { head, tail }   — [head | tail]
```

Lists share tails naturally:

```
let a = list(1, 2, 3);     // [1 | [2 | [3 | None]]]
let b = [0 | a];            // [0 | ·] → shares a's cons cells
```

#### 2.2 Performance

| Operation | Time | Space |
|-----------|------|-------|
| `[x \| xs]` (cons) | O(1) | 1 alloc |
| `hd(list)` | O(1) | — |
| `tl(list)` | O(1) | — |
| `len(list)` | O(n) | — |
| `list[i]` | O(i) | — |
| `reverse(list)` | O(n) | n allocs |

#### 2.3 New Syntax

**Cons expression:**

```flux
let xs = [1 | [2 | [3 | None]]];
let ys = [0 | xs];                  // O(1) prepend, shares xs
```

**Cons pattern:**

```flux
match my_list {
    [x | rest] -> print(x),
    None -> print("empty"),
}
```

**`[a, b, c]` remains Array.** Lists are built via `[h | t]` or the `list(...)` builtin.

#### 2.4 New Token

Add `Bar` token (`|`) to `token_type.rs`. The lexer produces `Bar` for standalone `|` (not followed by `>`). `|>` continues to produce `Pipe`.

#### 2.5 New AST Nodes

```rust
// expression.rs
Expression::Cons { head: Box<Expression>, tail: Box<Expression>, span }

// pattern enum
Pattern::Cons { head: Box<Pattern>, tail: Box<Pattern>, span }
Pattern::EmptyList { span }
```

#### 2.6 New Opcodes

```rust
OpCons  = 46,   // pop head + tail, alloc Cons, push Gc(handle)
OpHead  = 47,   // pop list, push head
OpTail  = 48,   // pop list, push tail
OpList  = 49,   // build list from N stack elements (nested cons)
```

(Numbering assumes Proposal 016 takes slots 44-45 for OpTailCall/OpConsumeLocal.)

**OpCons handler:**
```rust
let tail = self.pop()?;
let head = self.pop()?;
let handle = self.gc_heap.alloc(HeapObject::Cons { head, tail }, &self.root_set());
self.push(Object::Gc(handle))?;
```

**OpList handler:**
```rust
// Build right-to-left: [a, b, c] → [a | [b | [c | None]]]
let mut list = Object::None;
for i in (0..n).rev() {
    let elem = self.stack[self.sp - n + i].clone();
    let h = self.gc_heap.alloc(HeapObject::Cons { head: elem, tail: list }, &self.root_set());
    list = Object::Gc(h);
}
self.sp -= n;
self.push(list)?;
```

#### 2.7 New Builtins

| Builtin | Signature | Complexity |
|---------|-----------|------------|
| `hd(list)` | List → Object | O(1) |
| `tl(list)` | List → List | O(1) |
| `list(...)` | varargs → List | O(n) |
| `is_list(x)` | Object → Bool | O(1) |
| `to_list(arr)` | Array → List | O(n) |
| `to_array(list)` | List → Array | O(n) |

#### 2.8 Existing Builtins Gain List Support

| Builtin | Current | With List |
|---------|---------|-----------|
| `len(x)` | Array, String | + List: O(n) traversal |
| `first(x)` | Array O(1) | + List: O(1) (synonym for hd) |
| `rest(x)` | Array O(n) clone | + List: O(1) (synonym for tl) |
| `contains(x, e)` | Array O(n) | + List: O(n) traversal |
| `reverse(x)` | Array O(n) | + List: O(n), new spine |
| `type_of(x)` | returns type string | returns `"List"` for cons |

#### 2.9 Parser Changes

In `parse_array()`: after parsing the first expression inside `[`, check for `Bar`. If found, parse as `Expression::Cons { head, tail }`. Otherwise, continue with comma-separated array.

In `parse_pattern()`: `[head | tail]` produces `Pattern::Cons`. `[]` produces `Pattern::EmptyList`.

---

### Phase 3: Persistent Map (HAMT)

#### 3.1 Hash Array Mapped Trie

A HAMT with branching factor 32. Each internal node:

```rust
pub enum HamtEntry {
    Leaf(HashKey, Object),
    Node(GcHandle),          // subtree
    Collision(GcHandle),     // entries with same hash prefix
}

// On GC heap:
HeapObject::HamtNode { bitmap: u32, children: Vec<HamtEntry> }
HeapObject::HamtCollision { hash: u64, entries: Vec<(HashKey, Object)> }
```

The 32-bit bitmap tracks which of 32 branches are populated. The `children` Vec is compressed — only populated entries are stored. Child index = popcount(bitmap & (1 << slot) - 1).

#### 3.2 HAMT Operations

**Lookup (`map[key]`):**
1. Hash the key (64-bit).
2. At depth d, extract 5 bits from hash at position `d * 5`.
3. Check bitmap. If bit unset → key not found.
4. If bit set → follow compressed child. Leaf → compare key. Node → recurse. Collision → linear scan.

**Insert (`put(map, key, value)`):**
1. Follow lookup path.
2. At insertion point, create new node differing only at modified child.
3. All other children shared via GcHandle (structural sharing).
4. Return new root handle; old root unchanged.

**Delete (`delete(map, key)`):**
1. Follow lookup path.
2. Create new node without the target child.
3. Collapse single-child nodes upward.

#### 3.3 Performance

| Operation | Time | Notes |
|-----------|------|-------|
| `map[key]` | O(log32 n) | ~4 levels for 1M entries |
| `put(map, k, v)` | O(log32 n) | creates ~4 new nodes, shares rest |
| `delete(map, k)` | O(log32 n) | creates ~4 new nodes, shares rest |
| `merge(m1, m2)` | O(m2 × log32 m1) | insert each m2 entry into m1 |
| `keys(map)` | O(n) | traverses all leaves |
| `has_key(map, k)` | O(log32 n) | |
| `len(map)` | O(n) or O(1) if cached | |

For typical workloads (<1M entries), trie depth ≤ 4. Each operation touches/creates ≤ 4 nodes. Everything else is shared.

#### 3.4 Map Replaces Hash

`Object::Hash(HashMap)` is removed. The `{"a": 1}` syntax creates a HAMT:

```flux
let m = {"name": "Alice", "age": 30};  // creates HAMT Map
let m2 = put(m, "email", "a@b.com");   // new Map, shares nodes with m
```

`OpHash` (opcode 28) is repurposed to build a HAMT from stack pairs.

#### 3.5 Hash Builtin Migration

| Builtin | Before | After |
|---------|--------|-------|
| `keys(h)` | HashMap → Array | HAMT leaf traversal → Array |
| `values(h)` | HashMap → Array | HAMT leaf traversal → Array |
| `has_key(h, k)` | HashMap O(1) | HAMT O(log32 n) |
| `merge(h1, h2)` | HashMap clone O(n) | HAMT insert chain O(m2 × log32 m1) |
| `is_hash(x)` | checks Hash variant | alias for `is_map(x)`, deprecated |

#### 3.6 New Map Builtins

| Builtin | Signature | Complexity |
|---------|-----------|------------|
| `put(map, key, value)` | Map → Map | O(log32 n) |
| `delete(map, key)` | Map → Map | O(log32 n) |
| `get(map, key)` | Map → Option | O(log32 n) |
| `is_map(x)` | Object → Bool | O(1) |

#### 3.7 Index Operations Update

`execute_index_expression` in `index_ops.rs` gains:

```rust
(Object::Gc(handle), _) => {
    match self.gc_heap.get(handle) {
        HeapObject::Cons { .. } => {
            // List index: O(i) traversal
        }
        HeapObject::HamtNode { .. } => {
            // Map lookup: O(log32 n)
        }
    }
}
```

---

## Language Surface Summary

### New Syntax

| Syntax | Meaning | Example |
|--------|---------|---------|
| `[h \| t]` expression | Cons (prepend) | `[1 \| xs]` |
| `[h \| t]` pattern | Destructure list | `match xs { [h \| t] -> h }` |
| `[]` pattern | Empty list pattern | `match xs { [] -> "empty" }` |

### Unchanged Syntax

| Syntax | Meaning |
|--------|---------|
| `[a, b, c]` | Array literal (stays Array) |
| `{"k": v}` | Map literal (now HAMT instead of HashMap) |
| `x[i]` | Index (works on Array, List, Map) |

### New Builtins (10)

`hd`, `tl`, `list`, `is_list`, `to_list`, `to_array`, `put`, `delete`, `get`, `is_map`

### Modified Builtins

`len`, `first`, `rest`, `contains`, `reverse`, `keys`, `values`, `has_key`, `merge`, `type_of`

---

## Closure Interaction with GC

Closures capture free variables via eager clone. With GC collections, the `free` vector contains `Object::Gc(handle)` values — copying a handle is a u32 copy (cheap). The GC mark phase walks closure free variables:

```
mark_closure(closure):
    for obj in closure.free:
        mark_object(obj)
```

Closures remain `Rc<Closure>` in this proposal. The mark phase handles `Object::Closure(rc)` by dereferencing and walking `rc.free`. Migration to `HeapObject::Closure` is deferred.

---

## Equality Semantics

`Object::Gc(h)` equality requires deep structural comparison:
- Two lists are equal if they have the same elements in the same order.
- Two maps are equal if they have the same key-value pairs regardless of insertion order.

`Object::PartialEq` must be updated to dereference `GcHandle` values and compare `HeapObject` contents. This requires access to the `GcHeap`, which means equality checks in the VM dispatch must pass the heap reference.

---

## Acceptance Criteria

### Phase 1: GC

1. `GcHeap` allocates and collects correctly.
2. 100K allocations with limited roots: GC frees >99% unreachable objects.
3. No use-after-free on live handles after collection.
4. Threshold triggers automatically; `--no-gc` disables.
5. All existing tests pass.

### Phase 2: List

1. `[1 | [2 | [3 | None]]]` creates a 3-element list.
2. `hd` and `tl` return head/tail in O(1).
3. `match xs { [h | t] -> h, None -> 0 }` works.
4. `list(1, 2, 3)` creates nested cons.
5. Tail sharing: `let b = [0 | a]` — `tl(b)` shares cons cells with `a`.
6. `len`, `contains`, `reverse`, `first`, `rest` work on lists.
7. `to_list`/`to_array` round-trip correctly.

### Phase 3: Map

1. `{"a": 1}` creates HAMT.
2. `put(m, "b", 2)` returns new map; original unchanged.
3. `m["a"]` returns `Some(1)`.
4. `delete(m, "a")` returns map without key.
5. Structural sharing: `put` shares all nodes except the modified path.
6. 10K sequential `put` operations complete in O(n log n), not O(n^2).
7. All existing hash literal tests pass.

---

## Implementation Checklist

### Phase 1: GC Infrastructure

1. Create `src/runtime/gc/` module (`mod.rs`, `heap.rs`, `handle.rs`)
2. Implement `GcHeap`, `GcHandle`, `HeapEntry`, `HeapObject`
3. Implement `alloc()`, `get()`, `collect()` with mark/sweep
4. Implement `RootSet` construction from VM state
5. Add `gc_heap: GcHeap` to `VM` struct
6. Add `Object::Gc(GcHandle)` variant
7. Update `Object::type_name()`, `Display`, `PartialEq`, `Clone`
8. Update mark phase to walk `Some`, `Left`, `Right`, `Array`, `Closure`
9. Add adaptive threshold logic
10. Add `--gc-threshold` and `--no-gc` CLI flags
11. Extend leak detector with GC stats
12. Unit tests: allocation, mark, sweep, threshold
13. Integration test: 100K allocations stress test

### Phase 2: Persistent List

1. Add `Bar` token (`|`) to `token_type.rs`
2. Update lexer: standalone `|` → `Bar`, `|>` → `Pipe`
3. Add `Expression::Cons` to `expression.rs`
4. Add `Pattern::Cons` and `Pattern::EmptyList`
5. Update `parse_array()` to detect `[head | tail]`
6. Update `parse_pattern()` for cons and empty-list patterns
7. Add `HeapObject::Cons` variant
8. Add opcodes: `OpCons`, `OpHead`, `OpTail`, `OpList`
9. Implement dispatch handlers
10. Compile `Expression::Cons` → `OpCons`
11. Compile `list(...)` → `OpList`
12. Compile `Pattern::Cons` to head/tail destructure in match
13. Add builtins: `hd`, `tl`, `list`, `is_list`, `to_list`, `to_array`
14. Update `len`, `first`, `rest`, `contains`, `reverse` for lists
15. Update `execute_index_expression` for list indexing
16. Update `Display`, `type_of` for lists
17. Bump bytecode cache version
18. Tests: parser, compiler, VM, integration, snapshots

### Phase 3: Persistent Map (HAMT)

1. Add `HeapObject::HamtNode`, `HeapObject::HamtCollision`, `HamtEntry`
2. Implement HAMT `lookup()`, `insert()`, `delete()`, `iter()`
3. Repurpose `OpHash` to build HAMT from stack pairs
4. Remove `Object::Hash(HashMap)` variant
5. Update `execute_index_expression` for HAMT lookup
6. Migrate `keys`, `values`, `has_key`, `merge` to HAMT
7. Add builtins: `put`, `delete`, `get`, `is_map`
8. Add `is_hash` alias for `is_map`
9. Update `Display`, `type_of`, `PartialEq` for maps
10. Implement consistent 64-bit hash for `HashKey`
11. Update bytecode cache serialization
12. Migrate hash tests → map tests
13. HAMT tests: structural sharing, collisions, large scale
14. Performance benchmark: 10K sequential inserts

---

## Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| GC pause latency on large heaps | Visible pauses | Adaptive threshold; future incremental GC |
| Incorrect root tracing | Use-after-free | Exhaustive root enumeration; stress tests |
| HAMT hash collisions | Worst-case O(n) | Collision nodes + good hash function |
| `Object::Gc` indirection overhead | Slower primitive ops | Primitives stay non-Gc |
| Removing `Object::Hash` breaks code | Compile errors | Mechanical migration |
| `Bar` token conflicts | Limits future syntax | Well-established in ML/Erlang/Elixir |
| Opcode numbering conflicts with Proposal 016 | Miscompilation | Coordinate; implement 016 first |
| List `len` is O(n) | Performance trap | Document clearly; Array for random access |
| Equality needs GcHeap access | API complexity | Pass heap ref in VM dispatch |

---

## Open Questions

1. **Should `list(1, 2, 3)` be a builtin or compile-time sugar?** Recommendation: compiler-recognized builtin that emits `OpList` directly (like `OpArray`).

2. **Should empty list be `[]` or `None`?** Using `None` is simpler but conflates "no value" with "empty list." A dedicated `Object::Nil` is cleaner but adds a variant. Recommendation: `None` for now, matching Elixir's spirit.

3. **Should `Object::Closure` migrate to GC heap?** Would simplify the model but is a larger change. Recommendation: defer. Closures stay `Rc<Closure>`.

4. **HAMT branching factor: 32 or 16?** 32 gives shallower trees (Clojure/Elixir standard). 16 uses less memory per node. Recommendation: 32.

5. **Should `put(map, k, v)` also work on Arrays?** Generic "associative update" is useful but broadens scope. Recommendation: defer; map-only for now.

6. **Should GC be optional via compile-time feature flag?** Recommendation: always included; disable at runtime via `--no-gc`.

7. **Phase ordering?** Phase 1 (GC) must come first. Phase 2 (List) ships before Phase 3 (Map) since it's simpler and validates the GC integration.

8. **How does `==` work across GcHandle values?** Deep structural comparison. Requires threading `&GcHeap` through equality checks. The `OpEqual` handler constructs the comparison with heap access.

9. **Should `Tuple` be added?** Elixir has fixed-size tuples. Flux's Array fills this role. Defer unless a clear need emerges.

10. **Interaction with Proposal 016 (TCE)?** Complementary. `OpConsumeLocal` still benefits List/Map operations. Coordinate opcode numbering.
