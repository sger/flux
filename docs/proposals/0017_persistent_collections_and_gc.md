- Feature Name: Persistent Collections and Garbage Collection
- Start Date: 2026-02-08
- Proposal PR: 
- Flux Issue: 

# Proposal 0017: Persistent Collections and Garbage Collection

## Summary
[summary]: #summary

Introduce GC-managed persistent data structures — a cons-cell linked List and a Hash Array Mapped Trie (HAMT) Map — to Flux. Persistent structures share subtrees across versions, eliminating the O(n) full-clone cost on every mutating operation. A stop-the-world mark-and-sweep garbage collector manages the lifetime of shared heap nodes.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Goals

1. Provide an O(1) prepend/head/tail persistent List with `[head | tail]` syntax and pattern matching.
2. Provide an O(log32 n) persistent Map backed by a HAMT that replaces the current `Hash(HashMap)`.
3. Implement a stop-the-world mark-and-sweep GC to manage heap-allocated shared nodes.
4. Preserve full backward compatibility for Array syntax and base functions.

### Non-Goals

1. Concurrent, incremental, or generational GC.
2. Moving or compacting GC.
3. Removing the Array type.
4. Lazy sequences or streams.
5. Transient (mutable builder) variants of persistent collections.

### 2.3 New Syntax

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

**`[a, b, c]` remains Array.** Lists are built via `[h | t]` or the `list(...)` base.

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

### Equality Semantics

`Object::Gc(h)` equality requires deep structural comparison:
- Two lists are equal if they have the same elements in the same order.
- Two maps are equal if they have the same key-value pairs regardless of insertion order.

`Object::PartialEq` must be updated to dereference `GcHandle` values and compare `HeapObject` contents. This requires access to the `GcHeap`, which means equality checks in the VM dispatch must pass the heap reference.

### Goals

### Non-Goals

### 2.3 New Syntax

**Cons expression:**

**Cons pattern:**

### New Syntax

### Unchanged Syntax

### Equality Semantics

### Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **__preamble__:** **Implementation Order:** 019 → 016 → 017 (this) → 018 - **Problem 1: O(n) Clone on Every Collection Operation:** Every `push`, `concat`, `merge`, `rest`, `r...
- **Detailed specification (migrated legacy content):** **Implementation Order:** 019 → 016 → 017 (this) → 018
- **Problem 1: O(n) Clone on Every Collection Operation:** Every `push`, `concat`, `merge`, `rest`, `reverse`, and `sort` clones the entire backing `Vec` or `HashMap`: ```rust // array_ops.rs — base_push let mut new_arr = arr.clone(); /...
- **Problem 2: No Idiomatic Recursive List Processing:** `rest(arr)` clones the entire tail — O(n) per call: ```rust // array_ops.rs — base_rest Ok(Object::Array(arr[1..].to_vec())) // O(n) copy ```
- **Problem 3: Shared Nodes Require GC:** With persistent data structures, multiple collection versions share internal nodes forming a DAG. When closures capture collections, reachability can become arbitrarily complex....
- **1.1 GC Heap:** New module `src/runtime/gc/`: ```rust /// Opaque handle into the GC heap. #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub struct GcHandle(u32);

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Non-Goals

1. Concurrent, incremental, or generational GC.
2. Moving or compacting GC.
3. Removing the Array type.
4. Lazy sequences or streams.
5. Transient (mutable builder) variants of persistent collections.

### Non-Goals

### Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| GC pause latency on large heaps | Visible pauses | Adaptive threshold; future incremental GC |
| Incorrect root tracing | Use-after-free | Exhaustive root enumeration; stress tests |
| HAMT hash collisions | Worst-case O(n) | Collision nodes + good hash function |
| `Object::Gc` indirection overhead | Slower primitive ops | Primitives stay non-Gc |
| Removing `Object::Hash` breaks code | Compile errors | Mechanical migration |
| `Bar` token conflicts | Limits future syntax | Well-established in ML/Erlang/Elixir |
| Opcode numbering conflicts with Proposal 0016 | Miscompilation | Coordinate; implement 016 first |
| List `len` is O(n) | Performance trap | Document clearly; Array for random access |
| Equality needs GcHeap access | API complexity | Pass heap ref in VM dispatch |

### Non-Goals

### Risks

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

### Open Questions

1. **Should `list(1, 2, 3)` be a base or compile-time sugar?** Recommendation: compiler-recognized base that emits `OpList` directly (like `OpArray`).

2. **Should empty list be `[]` or `None`?** Using `None` is simpler but conflates "no value" with "empty list." A dedicated `Object::Nil` is cleaner but adds a variant. Recommendation: `None` for now, matching Elixir's spirit.

3. **Should `Object::Closure` migrate to GC heap?** Would simplify the model but is a larger change. Recommendation: defer. Closures stay `Rc<Closure>`.

4. **HAMT branching factor: 32 or 16?** 32 gives shallower trees (Clojure/Elixir standard). 16 uses less memory per node. Recommendation: 32.

5. **Should `put(map, k, v)` also work on Arrays?** Generic "associative update" is useful but broadens scope. Recommendation: defer; map-only for now.

6. **Should GC be optional via compile-time feature flag?** Recommendation: always included; disable at runtime via `--no-gc`.

7. **Phase ordering?** Phase 1 (GC) must come first. Phase 2 (List) ships before Phase 3 (Map) since it's simpler and validates the GC integration.

8. **How does `==` work across GcHandle values?** Deep structural comparison. Requires threading `&GcHeap` through equality checks. The `OpEqual` handler constructs the comparison with heap access.

9. **Should `Tuple` be added?** Elixir has fixed-size tuples. Flux's Array fills this role. Defer unless a clear need emerges.

10. **Interaction with Proposal 0016 (TCE)?** Complementary. `OpConsumeLocal` still benefits List/Map operations. Coordinate opcode numbering.

### Open Questions

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
