- Feature Name: Tracing GC for VM Values
- Start Date: 2026-03-10
- Status: Draft
- Proposal PR:
- Flux Issue:

# Proposal 0096: Tracing GC for VM Values

## Summary

Extend the existing `GcHeap` mark-and-sweep collector to manage `Closure` and `Adt`
runtime values, replacing `Rc<Closure>` and `Rc<AdtValue>` with `GcHandle` pointers.
This eliminates reference-count overhead on the two hottest allocation paths in the VM
and aligns the interpreter's value representation with how OCaml's runtime avoids
per-operation refcount traffic.

## Motivation

The VM's `Value` enum uses `Rc` for every heap-allocated type:

```rust
Value::Closure(Rc<Closure>)
Value::Adt(Rc<AdtValue>)
Value::Array(Rc<Vec<Value>>)
Value::String(Rc<str>)
// ...
```

Every `clone()` on one of these variants increments a reference count. This happens
on operations that are fundamental to functional programming:

- **Function call / return** — clones `Rc<Closure>` to push into the new frame
- **Pattern match with field extraction** — clones `Rc<AdtValue>` to read fields
- **`OpGetLocal` / `OpGetUpvalue`** — clones whatever value sits in the slot

In OCaml's VM, none of these operations touch a reference count. Values are either
tagged integers (one word, no heap) or raw pointers into a GC-managed heap. The
tracing collector discovers liveness in a separate phase; the hot path is just
pointer reads and writes.

Flux already has the foundation: `GcHeap` is a non-moving mark-and-sweep collector
used for HAMT nodes and cons cells (Proposal 0017). Extending it to cover closures
and ADTs applies the same design to the two highest-frequency allocation types.

### Why these two types first

- **`Closure`** is cloned on every function call, every upvalue capture, and every
  `OpCurrentClosure`. In recursive programs this is the single most frequent `Rc`
  operation.
- **`Adt`** is cloned on every pattern match that extracts fields — the core operation
  of functional data processing.

`Array`, `String`, and `Tuple` are left as `Rc` for now. They are less frequently
cloned in the hot dispatch path and their migration can follow independently.

## Guide-level explanation

From a contributor's perspective, the change affects two layers:

### 1. `GcHeap` becomes the owner of closures and ADTs

Instead of:

```rust
let closure = Rc::new(Closure { ... });
Value::Closure(closure)
```

The VM allocates into the GC heap:

```rust
let handle = vm.gc_alloc(HeapObject::Closure(closure_data));
Value::Closure(handle)
```

`handle` is a `GcHandle(u32)` — a cheap copyable index. Cloning a
`Value::Closure(handle)` copies a `u32`. No atomic operation, no heap write.

### 2. The mark phase learns to trace into closures and ADTs

The existing mark phase already traverses `HeapObject::ConsList` and
`HeapObject::Hamt`. It must be extended to follow `Value` fields inside the
new `HeapObject::Closure` and `HeapObject::Adt` variants, recursively
marking any `GcHandle`s they contain.

### 3. Root enumeration is unchanged

`GcHeap::collect()` already receives `stack`, `globals`, `frames`, and
`constants` as roots. The root-scanning pass that currently looks for
`Value::Gc(h)` is extended to also look for `Value::Closure(h)` and
`Value::Adt(h)` — the same traversal, two new arms.

### What does NOT change

- The language surface — no syntax or semantics changes.
- `Array`, `String`, `Tuple` — remain `Rc` in this proposal.
- The JIT backend — already uses raw `i64` / arena pointers; unaffected.
- The GC algorithm — still non-moving mark-and-sweep. No compaction.
- The `GcHandle` type — still a `u32` index into `Vec<Option<HeapEntry>>`.

## Reference-level explanation

### New `HeapObject` variants

```rust
pub enum HeapObject {
    // existing
    ConsList { head: Value, tail: Value },
    Hamt(HamtNode),

    // new
    Closure {
        function: Rc<CompiledFunction>,   // bytecode is immutable; Rc is fine
        upvalues: Vec<Value>,
    },
    Adt {
        constructor: Rc<str>,             // interned string; Rc is fine
        fields: AdtFields,
    },
}
```

`CompiledFunction` and constructor strings are immutable and never form cycles
with Values, so `Rc` inside `HeapObject` is sound — the GC only needs to trace
the `Value`-typed fields (`upvalues`, `AdtFields`).

### Updated `Value` variants

```rust
// before
Value::Closure(Rc<Closure>)
Value::Adt(Rc<AdtValue>)

// after
Value::Closure(GcHandle)
Value::Adt(GcHandle)
```

`Value::AdtUnit(Rc<str>)` is unchanged — zero-field constructors carry no
tracing cost and the `Rc<str>` is shared, interned, and never a GC root issue.

### Mark phase extension

In `gc_heap.rs`, the mark traversal currently processes `Value::Gc(h)`.
Extend it to handle the two new variants:

```rust
fn mark_value(&mut self, value: &Value) {
    match value {
        Value::Gc(h) | Value::Closure(h) | Value::Adt(h) => {
            self.mark_handle(*h);
        }
        Value::Array(rc) => {
            for v in rc.iter() { self.mark_value(v); }
        }
        // primitives: nothing to mark
        _ => {}
    }
}

fn mark_handle(&mut self, handle: GcHandle) {
    let entry = match self.entries.get_mut(handle.0 as usize) {
        Some(Some(e)) if !e.marked => e,
        _ => return,
    };
    entry.marked = true;
    match &entry.object.clone() {   // clone to release borrow
        HeapObject::Closure { upvalues, .. } => {
            for v in upvalues { self.mark_value(v); }
        }
        HeapObject::Adt { fields, .. } => {
            for v in fields.iter() { self.mark_value(v); }
        }
        HeapObject::ConsList { head, tail } => {
            self.mark_value(head);
            self.mark_value(tail);
        }
        HeapObject::Hamt(node) => self.mark_hamt(node),
    }
}
```

### Root scan extension

```rust
fn scan_roots(&mut self, stack: &[Value], sp: usize, globals: &[Value], ...) {
    for v in &stack[..sp] { self.mark_value(v); }
    for v in globals { self.mark_value(v); }
    // frames: scan upvalues of active closures
    for frame in &frames[..=frame_index] {
        // frame.closure is now a GcHandle embedded in Value::Closure
        // but we also need the handle itself
        self.mark_handle(frame.closure_handle);
    }
}
```

`Frame` stores the current `GcHandle` instead of `Rc<Closure>`. The frame's
`closure_handle` is treated as an explicit root, ensuring the active closure
is never swept mid-execution.

### Frame and function_call changes

`Frame` currently holds `Rc<Closure>`. It becomes:

```rust
pub struct Frame {
    pub closure_handle: GcHandle,   // replaces Rc<Closure>
    pub base_pointer: usize,
    pub ip: usize,
}
```

When the VM needs to read upvalues or the function body from a frame, it
dereferences through `gc_heap`:

```rust
let closure = self.gc_heap.get_closure(frame.closure_handle);
let instructions = &closure.function.instructions;
```

`get_closure` is a non-panicking index into `entries` — safe because the
frame's handle is always a GC root and will not be swept.

### Safety argument

The `GcHeap` is non-moving. A `GcHandle` is a stable index; once allocated,
the slot is never relocated. The only validity concern is use-after-free (sweep
removes a live object). This is prevented by exhaustive root enumeration:
any `GcHandle` reachable from stack / globals / active frames is marked before
the sweep phase runs.

The borrow checker cannot enforce this statically (we hold `&mut GcHeap` and
also hold handles into it). The pattern used by the existing HAMT code — `clone`
the `HeapObject` out of the entry before recursing — is sufficient to satisfy
the borrow checker without unsafe code in the mark phase.

### Implementation order

1. Add `HeapObject::Closure` and `HeapObject::Adt` variants.
2. Extend `mark_value` / `mark_handle` to trace them.
3. Change `Value::Closure` and `Value::Adt` to hold `GcHandle`.
4. Update `Frame` to store `closure_handle: GcHandle`.
5. Update `function_call.rs` — push/pop frames using handles.
6. Update `dispatch.rs` — all `OpGetUpvalue`, `OpCurrentClosure`,
   `OpCall`, `OpTailCall` to dereference via `gc_heap`.
7. Update root scan in `gc_heap.rs::collect()`.
8. Delete `Closure` struct's `Rc` wrapper (if no longer needed elsewhere).
9. Run full test suite; verify with `gc-telemetry` feature.

Start with step 1–3 in isolation (extend the GC, keep `Value` unchanged) to
verify the mark phase is correct before changing the value representation.

## Drawbacks

- **Borrow checker friction** — every site that reads a closure must go through
  `&self.gc_heap`. When `&mut self` is held for the stack, pattern-matching on
  a closure requires borrowing two fields simultaneously, which Rust disallows
  without splitting the struct. This is solvable but requires care at each call
  site.

- **GC pause includes closures** — previously closures were freed deterministically
  via `Rc` drop. Now they live until the next collection. Programs that create
  many short-lived closures may see higher peak memory between GC cycles.

- **`Continuation` complexity** — delimited continuations capture frames by
  clone. When frames hold `GcHandle` instead of `Rc<Closure>`, continuations
  must ensure captured handles are treated as roots for the duration of the
  suspended computation. This needs careful handling.

## Rationale and alternatives

### Why extend `GcHeap` rather than switch to `Arc`?

`Arc` removes the single-threaded restriction but adds atomic operations —
strictly worse than `Rc` on single-threaded workloads and does not eliminate
the per-clone cost. `GcHandle` copy is non-atomic.

### Why not NaN-boxing (Proposal 0041)?

NaN-boxing compresses the `Value` enum to 8 bytes and gives unboxed integers
and floats. It does not eliminate `Rc` clones for heap types — it just makes
the discriminant check cheaper. The two approaches are complementary:
NaN-boxing + GC-managed closures would be the ideal long-term combination.

### Why not a generational collector?

The existing stop-the-world collector is simple and already correct. Generational
GC improves throughput for short-lived allocations but adds significant
implementation complexity (write barriers, remembered sets). Defer until
benchmarks show the stop-the-world pause is a real problem.

### Impact of not doing this

The VM continues to pay `Rc` refcount overhead on every function call and
pattern match. For tight recursive FP workloads this is the primary bottleneck
after instruction dispatch. The JIT already avoids this cost; the VM falls
further behind the JIT on allocation-heavy programs.

## Prior art

- **OCaml runtime** (`ocaml/runtime/interp.c`) — values are tagged words or
  raw GC pointers. No refcount on any value operation. The minor/major heap
  generational collector reclaims unreachable values without touching the hot path.

- **Lua 5.x / LuaJIT** — `GCObject` union with a mark byte in the header.
  All heap objects (strings, tables, closures, upvalues) are GC-managed.
  Closures are traced through their upvalue array.

- **BEAM (Erlang VM)** — all heap terms are word-sized. Process heaps are
  collected independently. No shared mutable state means no cross-process
  tracing is needed.

- **Proposal 0017** — introduced `GcHeap` and `GcHandle` for HAMT/cons cells.
  This proposal extends that infrastructure to the two most-cloned value types.

- **Proposal 0045** — earlier GC proposal that identified closures as a
  candidate for GC management (Question 3 in Unresolved Questions). This
  proposal resolves that question affirmatively.

- **Proposal 0041** — NaN-boxing. Complementary; addresses `Value` enum size,
  not clone overhead.

## Unresolved questions

1. **Continuation roots** — how does `Value::Continuation(Rc<RefCell<Continuation>>)`
   interact with GC-managed closures? Continuations capture frames; those frames
   now hold `GcHandle`s that must be kept live. Should `Continuation` itself
   become GC-managed, or should it retain the captured handles as explicit roots
   in the root scan?

2. **`Rc<CompiledFunction>` inside `HeapObject::Closure`** — `CompiledFunction`
   holds the bytecode `Vec<u8>` and debug info. It is shared across all closures
   created from the same function definition. Should it move to an interning table
   (like string interning) or stay as `Rc`?

3. **`JitClosure`** — `Value::JitClosure(Rc<JitClosure>)` follows the same
   pattern. Should it be migrated in the same pass or deferred?

4. **Threshold tuning** — with closures now GC-managed, the allocation count
   threshold (`DEFAULT_GC_THRESHOLD = 10_000`) may need recalibration. Closures
   are created far more frequently than HAMT nodes. Should the threshold be
   type-aware?

5. **`gc-telemetry` coverage** — the telemetry feature should be extended to
   report closure and ADT allocation/collection counts separately from
   HAMT/cons-cell counts, to measure the actual impact.

## Future possibilities

- **Complete value unification** — after `Closure` and `Adt`, migrate `Array`,
  `Tuple`, and `String` into `GcHeap`. At that point `Rc` disappears from
  `Value` entirely and the VM matches OCaml's value model structurally.

- **Generational GC** — once all values are GC-managed, a minor/major heap
  split becomes feasible. Most closures in recursive FP programs are short-lived
  and would be collected cheaply in the minor heap without touching the major heap.

- **Write barrier for future mutation** — if Flux ever adds mutable cells or
  actors sharing heap values, a write barrier (card table or snapshot-at-beginning)
  would be needed. The non-moving design makes adding one tractable.

- **NaN-boxing + GC handles** — combine Proposal 0041 (8-byte Value) with
  GC-managed heap types. `GcHandle(u32)` fits comfortably in the payload bits
  of a NaN-boxed word, giving both compact stack slots and zero-cost clone.
