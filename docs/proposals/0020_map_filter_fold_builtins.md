- Feature Name: `map` / `filter` / `fold` Base Functions
- Start Date: 2026-02-11
- Proposal PR: 
- Flux Issue: 

# Proposal 0020: `map` / `filter` / `fold` Base Functions

## Summary
[summary]: #summary

Add three higher-order base functions with eager array semantics: Add three higher-order base functions with eager array semantics:

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Goals

1. Provide first-class functional collection transforms for arrays.
2. Keep semantics explicit and predictable.
3. Ship with clear error behavior and performance expectations.
4. Keep API future-proof for Proposal 0017 migration.

### Non-Goals

1. Lazy iterator/stream semantics.
2. Implicit `reduce` variant without `init`.
3. Pattern-matching-based collection transforms.

### Goals

### Non-Goals

### Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **`map(arr, fn)`:** - Applies `fn(element)` to each element in order. - Returns a new array with transformed values. - Empty input returns `[]`. - **Callback arity**: `fn` mus...
- **`map(arr, fn)`:** - Applies `fn(element)` to each element in order. - Returns a new array with transformed values. - Empty input returns `[]`. - **Callback arity**: `fn` must accept exactly 1 arg...
- **`filter(arr, pred)`:** - Applies `pred(element)` to each element in order. - Keeps elements where predicate result is truthy. - Returns a new array. - Empty input returns `[]`. - **Callback arity**: `...
- **`fold(arr, init, fn)`:** - Left fold only (`foldl` semantics). - Applies `fn(acc, element)` in index order. - Returns the final accumulator. - `fold([], init, fn) == init`. - `init` is required (no ambi...
- **Error Contract:** - Type errors if: - first arg is not an array - function arg is not callable (`Closure` or `Base`) - Arity errors if callback receives wrong number of args: - `map` and `filter`...
- **Performance Expectations:** Baseline acceptance targets for initial release: 1. No asymptotic regressions versus hand-written loops. 2. `map/filter/fold` over 1k, 2k, 5k, and 10k arrays execute within 1.5x...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Non-Goals

1. Lazy iterator/stream semantics.
2. Implicit `reduce` variant without `init`.
3. Pattern-matching-based collection transforms.

### Non-Goals

### Non-Goals

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

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

### Compatibility and Future-Proofing

- Current semantics are eager over arrays.
- Base contract must not expose internals tied to `Vec` mutability.
- Future Proposal 0017 migration may extend support to persistent list/map structures without changing user-facing call shape.

### Compatibility and Future-Proofing
