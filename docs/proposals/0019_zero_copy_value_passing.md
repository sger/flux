- Feature Name: Zero-Copy Value Passing via Reference Counting
- Start Date: 2026-02-08
- Status: Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0019: Zero-Copy Value Passing via Reference Counting

## Summary
[summary]: #summary

Align Flux's runtime value model with Elixir/BEAM semantics: **immutable values passed by reference, never by deep clone**. Today, every stack read, local access, and function argument pass clones the entire `Object` — including deep copies of `Vec<Object>` and `HashMap<HashKey, Object>`. This proposal replaces deep cloning with cheap reference sharing via `Rc<T>`, while keeping Flux's existing strict evaluation and stack-based VM.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Goals

1. Eliminate O(n) deep clones when passing strings, arrays, and hashes to functions.
2. Make per-value argument transfer O(1) for all runtime types (ref-count increment or bitwise copy) and remove per-call argument `Vec` allocation by passing base arguments as borrowed slices (`&[Value]`).
3. Preserve immutable value semantics — no observable behavior change.
4. Lay groundwork for Proposal 0017 (persistent collections) by establishing shared-reference infrastructure.
5. Keep the stack-based VM architecture; no GC required for this proposal.

### Non-Goals

1. Persistent data structures (deferred to Proposal 0017).
2. Garbage collection (deferred to Proposal 0017).
3. Move semantics or borrow checking at the language level.
4. Lazy evaluation.
5. Concurrent/thread-safe sharing (`Arc` not needed — single-threaded VM).

### Goals

### Non-Goals

### Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **__preamble__:** **Implementation Order:** 019 (this) → 016 → 017 → 018 - **Implementation Checklist:** Track execution with small, verifiable tasks. Each task has a clear do...
- **Detailed specification (migrated legacy content):** **Implementation Order:** 019 (this) → 016 → 017 → 018
- **Implementation Checklist:** Track execution with small, verifiable tasks. Each task has a clear done condition.
- **019.1 Baseline and Safety Net:** - Add microbenchmarks for clone-heavy runtime paths: - local/global/free access (`OpGet*`) - base argument passing - closure capture - Add regression tests for value semantics (...
- **019.2 Introduce `Value` Type (No Behavior Change) [DONE]:** - Add `src/runtime/value.rs`. - Implement `Value` enum and core helpers (`type_name`, `is_truthy`, `to_hash_key`, `Display`). - Add temporary migration alias in `src/runtime/obj...
- **019.3 Rc-Wrap Heap Variants [DONE]:** - Convert heap-owned variants: - `String` -> `Rc<str>` - `Array` -> `Rc<Vec<Value>>` - `Hash` -> `Rc<HashMap<HashKey, Value>>` - `Some/Left/Right/ReturnValue` -> `Rc<Value>` - K...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Non-Goals

1. Persistent data structures (deferred to Proposal 0017).
2. Garbage collection (deferred to Proposal 0017).
3. Move semantics or borrow checking at the language level.
4. Lazy evaluation.
5. Concurrent/thread-safe sharing (`Arc` not needed — single-threaded VM).

### Non-Goals

### Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| `Rc` overhead for small strings | Slight memory increase | Most strings are interned symbols; only runtime strings use `Rc<str>` |
| `PartialEq` through `Rc` | Compares by value, not identity (correct but potentially slow for deep structures) | `Rc::ptr_eq` for fast identity check where appropriate |
| Breakage in pattern matching | `Rc` patterns differ from `Box` | Mechanical migration; compiler catches all mismatches |
| Serialization changes | Bytecode cache incompatibility | Bump cache version; old caches auto-invalidate |
| Base API churn | Every base needs updating | Systematic, file-by-file migration with tests |
| `Rc` reference cycle potential | Memory leak | Enforce No-Cycle Invariant; add regression tests around closure capture and nested containers |
| `Rc<str>` vs `Rc<String>` choice | API ergonomics | `Rc<str>` is more efficient; conversion via `Rc::from(s.as_str())` |

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

1. **Naming: `Value` or keep `Object`?** Elixir uses "term", Haskell uses "value". Recommendation: introduce as `Value`, then decide whether to keep both names or consolidate.

2. **Should `pop()` use `mem::replace` or `clone()`?** `mem::replace` is a true move (no ref-count), but leaves `Value::None` in dead slots. Recommendation: `mem::replace` — it's faster and the dead slot is never read.

3. **Do we keep a temporary adapter from old `Vec<Value>` base signatures to new `&[Value]` signatures during migration?** Recommendation: yes, short-lived adapter for phased rollout, then remove.

4. **Should `Rc<str>` be used or `Rc<String>`?** `Rc<str>` avoids double indirection but is slightly less ergonomic. Recommendation: `Rc<str>` for efficiency.

5. **Interaction with string interning (`Symbol`)?** Interned symbols are already deduplicated. `Rc<str>` is for runtime strings (concatenation results, user input, etc.). No conflict.

6. **Should this proposal also optimize `OpEqual` to use `Rc::ptr_eq` as a fast path?** If two values share the same `Rc` pointer, they're guaranteed equal. Recommendation: yes, add as part of Phase 2 — it's a one-line optimization.

### Open Questions

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
