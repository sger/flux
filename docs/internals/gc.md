# Garbage Collector

> Source: `src/runtime/gc/`

Flux uses a **stop-the-world mark-and-sweep GC** for persistent collections. The GC heap is separate from Rc-backed values — only cons lists and HAMT maps live on it.

## Why a GC at All

`Rc` cannot collect cycles, but the language's **no-cycle invariant** (values must form DAGs) means `Rc` is safe for most values. The GC exists specifically for persistent data structures that use structural sharing — cons cells and HAMT nodes — where sharing makes it impractical to track individual reference counts.

## Heap Object Types

```rust
// src/runtime/gc/heap_object.rs
enum HeapObject {
    Cons { head: Value, tail: Value },         // immutable linked list cell
    HamtNode { ... },                          // internal HAMT branch node
    HamtCollision { key: Value, value: Value },// HAMT leaf collision bucket
}
```

- **Cons cells** — O(1) prepend, O(n) traversal. Empty list is `Value::None`.
- **HAMT nodes** — Hash Array Mapped Trie, 5 bits per level, structural sharing on insert/delete.

## GcHandle

All heap-allocated objects are accessed via `GcHandle` — a typed index into the GC's entry vector. Handles are stored in `Value::Gc(GcHandle)`.

```rust
// Value::Gc wraps a heap-managed object
let list = Value::Gc(gc_heap.alloc(HeapObject::Cons { head, tail }));
```

`Display` for `Value::Gc` prints `<gc@N>` (no heap access). Use `list_ops::format_value(value, ctx)` for proper display.

## Allocation

```rust
fn alloc(&mut self, obj: HeapObject) -> GcHandle
```

- Appends to the entries vector, or reuses a freed slot from the free list.
- Increments `allocation_count`.
- Caller checks `should_collect()` before or after allocation.

## Collection Trigger

```rust
fn should_collect(&self) -> bool {
    self.allocation_count >= self.gc_threshold
}
```

Default threshold: **10,000 allocations**. Override with `--gc-threshold <n>`. Disable GC with `--no-gc`.

### Adaptive Threshold

After each collection:
- If **< 25%** of objects were freed (heap is dense) → threshold **doubles**.
- If **> 75%** of objects were freed (heap is sparse) → threshold **halves**.

This prevents thrashing on allocation-heavy workloads and avoids unnecessary collections on sparse ones.

## Mark Phase

Uses an iterative worklist (avoids stack overflow on deep lists):

1. Seed worklist from **root set**: VM stack values, globals, call frame locals, closure captures.
2. For each `WorkItem` popped from the worklist:
   - If it's a `Value` — check if it contains a `GcHandle`; if so, push the handle.
   - If it's a `GcHandle` — mark it reachable, then push its children (head/tail for Cons, subtrees for HAMT).
3. All unmarked handles after the walk are unreachable.

## Sweep Phase

Linear scan over all entries:
- **Marked** → clear mark bit, keep.
- **Unmarked** → drop the `HeapObject`, add slot index to the free list.

## Telemetry

Build with `--features gc-telemetry` and run with `--gc-telemetry` to print GC stats after execution:

```
GC: 3 collections, 12,847 allocs, 9,203 freed, peak 4,201 live
```

Stats tracked in `src/runtime/gc/telemetry.rs`.

## Files

| File | Purpose |
|------|---------|
| `gc_heap.rs` | `GcHeap` struct, `alloc`, `collect`, `should_collect`, adaptive threshold |
| `gc_handle.rs` | `GcHandle` — typed index into the heap |
| `heap_object.rs` | `HeapObject` enum: Cons, HamtNode, HamtCollision |
| `heap_entry.rs` | Heap entry wrapper (object + mark bit) |
| `hamt.rs` | HAMT insert, lookup, delete with structural sharing |
| `hamt_entry.rs` | HAMT entry types |
| `telemetry.rs` | Collection statistics (feature-gated) |
