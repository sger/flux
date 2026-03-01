- Feature Name: Tail-Call Accumulator Optimization
- Start Date: 2026-02-08
- Proposal PR: 
- Flux Issue: 

# Proposal 0016: Tail-Call Accumulator Optimization

## Summary
[summary]: #summary

Introduce a two-phase optimization that (1) eliminates stack overflow for self-recursive tail calls by reusing frames, and (2) eliminates O(n^2) array copying in accumulator patterns by allowing the VM to mutate arrays in-place when the compiler can prove the source binding is dead.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Goals

1. Prevent stack overflow for deep self-recursion (Phase 1).
2. Reduce O(n^2) array accumulation to O(n) (Phase 2).
3. Preserve immutable semantics — the optimization must be invisible to the user.
4. Keep changes backward-compatible — existing bytecode without tail calls runs unchanged.

### Non-Goals

1. Mutual tail-call optimization (two or more functions calling each other in tail position).
2. General move semantics or linear types at the language level.
3. Garbage collection (this proposal is complementary to Proposal 0010, not a replacement).

### Goals

### Non-Goals

### Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **__preamble__:** **Implementation Order:** 019 → 016 (this) → 017 → 018 - **Problem 1: Stack Overflow:** Every `OpCall` pushes a new `Frame` onto the frame stack. A self-recu...
- **Detailed specification (migrated legacy content):** **Implementation Order:** 019 → 016 (this) → 017 → 018
- **Problem 1: Stack Overflow:** Every `OpCall` pushes a new `Frame` onto the frame stack. A self-recursive function that recurses 2000+ times overflows the 2048-slot stack: ```flux fn countdown(n) { if n == 0...
- **Problem 2: O(n^2) Array Accumulation:** `base_push` always clones the entire array: ```rust let mut new_arr = arr.clone(); // O(n) clone new_arr.push(args[1].clone()); Ok(Object::Array(new_arr)) ```
- **New Opcode: `OpTailCall`:** Add `OpTailCall = 44` with the same 1-byte operand as `OpCall` (argument count).
- **Compiler: Tail Position Detection:** Add an `in_tail_position: bool` flag to the `Compiler`. Set it to `true` before compiling the last expression of a function body. Propagate it through `if`/`else` branches and `...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Non-Goals

1. Mutual tail-call optimization (two or more functions calling each other in tail position).
2. General move semantics or linear types at the language level.
3. Garbage collection (this proposal is complementary to Proposal 0010, not a replacement).

### Non-Goals

### Risks

| Risk | Mitigation |
|------|------------|
| Incorrect tail-position detection | Start with self-calls only; extensive test coverage |
| Frame reuse corrupts arguments | Copy all new args before overwriting old locals |
| Liveness marks a live variable as dead | Conservative: only parameters, check `free_symbols` |
| `base_push` ownership change breaks callers | Signature `fn(Vec<Object>)` already transfers ownership |
| Bytecode cache incompatibility | Bump cache version number |

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

1. **`--no-tco` flag?** Useful for debugging. Compiler emits `OpCall` even in tail position.
2. **Extend to `rest(arr)`?** Same ownership principle — `drain(1..)` instead of `to_vec()`. Future extension.
3. **Liveness beyond parameters?** Let-bound locals used exactly once in tail-call args could also be consumed. Requires use-count tracking in symbol table. Future enhancement.
4. **`CompiledFunction.has_tail_calls` flag?** Skip tail-call dispatch logic for functions that don't use it. Minor optimization; defer unless profiling shows overhead.
5. **Interaction with Proposal 0010 (GC)?** Complementary. `OpConsumeLocal` drops references earlier, making objects eligible for collection sooner.

### Open Questions

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
