- Feature Name: Unit Test Framework for Flux
- Start Date: 2026-02-19
- Status: Not Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0035: Unit Test Framework for Flux

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Unit Test Framework for Flux in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

Flux has no mechanism for users to write unit tests in Flux code. Testing currently means
either writing Rust integration tests against the compiler pipeline (which tests the
compiler, not user code), or running programs manually and inspecting `print` output.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Design Principles

1. **No new syntax.** Tests are ordinary Flux functions. No decorators, attributes, or
   special declarations.
2. **Discovery by convention.** Functions named `test_*` are automatically collected by
   the test runner.
3. **Isolation at the Rust level.** The test runner calls each test via `invoke_value`,
   which returns `Result<Value, String>`. A runtime error (from an assertion or otherwise)
   becomes an `Err(String)` that the runner catches — other tests continue.
4. **Assertions are base functions.** They return `Err(String)` on failure, which propagates
   through the VM's normal error path and is caught by the runner.
5. **Pure functional test bodies.** Tests are zero-argument functions. Setup via `let`
   bindings or shared helper functions.

### Design Principles

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Comparison: Functional Language Test Frameworks:** | Language | Framework | Mechanism | |---|---|---| | Haskell | HUnit / Hspec | Assertions throw exceptions; runner catches...
- **Comparison: Functional Language Test Frameworks:** | Language | Framework | Mechanism | |---|---|---| | Haskell | HUnit / Hspec | Assertions throw exceptions; runner catches per-test | | Elm | elm-test | Tests return `Expectatio...
- **Writing Tests:** Tests are zero-argument functions whose name starts with `test_`: ```flux fn test_add_basic() { assert_eq(1 + 1, 2) }
- **Importing Modules Under Test:** fn test_collection_reverse() { assert_eq(C.reverse([1, 2, 3]), [3, 2, 1]) } ```
- **Test Output:** PASS test_add_basic (0ms) PASS test_string_length (0ms) FAIL test_multiply assert_eq failed expected: 12 actual: 8 PASS test_filter_empty (0ms) FAIL test_divide_by_zero runtime...
- **Assertion Base Functions:** Four new base functions added to `runtime/base functions/`:

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- `src/runtime/vm/function_call.rs` — `invoke_value` (the isolation mechanism)
- `src/runtime/base functions/mod.rs` — BASE_FUNCTIONS array and registration pattern
- `src/runtime/base functions/io_ops.rs` — `time(fn)` as a pattern for calling user functions
  from base functions
- `src/main.rs` — existing CLI flag handling
- Elm Test: `package.elm-lang.org/packages/elm-explorations/test`
- HUnit: `hackage.haskell.org/package/HUnit`

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
