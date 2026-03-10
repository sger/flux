- Feature Name: Stack Overflow Fix for Higher-Order Base Functions
- Start Date: 2026-02-11
- Status: Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0021: Stack Overflow Fix for Higher-Order Base Functions

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Stack Overflow Fix for Higher-Order Base Functions in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

~~The current runtime causes `stack overflow` for arrays near/above ~2k elements. This is a **blocking limitation** for production use with large datasets.~~

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

This proposal should be read as a user-facing and contributor-facing guide for the feature.

- The feature goals, usage model, and expected behavior are preserved from the legacy text.
- Examples and migration expectations follow existing Flux conventions.
- Diagnostics and policy boundaries remain aligned with current proposal contracts.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **__preamble__:** **Implemented:** 2026-02-11 - **Root Cause:** The primary failure mode is the VM value stack hard limit (`STACK_SIZE = 2048`), not host-thread call stack exh...
- **Detailed specification (migrated legacy content):** **Implemented:** 2026-02-11
- **Root Cause:** The primary failure mode is the VM value stack hard limit (`STACK_SIZE = 2048`), not host-thread call stack exhaustion: ```rust // src/runtime/vm/mod.rs const STACK_SIZE: usize...
- **Option 1: Iterative Callback Execution (Secondary Optimization):** **Approach:** Execute callbacks without creating nested frames by using a dedicated execution mode. This can improve performance, but it is not the primary fix for the current o...
- **Option 2: Trampolining with Continuation Passing:** **Approach:** Convert recursive callback invocations into an iterative trampoline loop.
- **Option 3: Increase Stack Size (Not Recommended):** **Pros:** - ✅ Trivial to implement (5 lines) - ✅ No VM changes needed

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Alternative: Accept Current Limitation

If fixing the stack overflow is not immediately feasible, we should:

1. **Document the limitation clearly** in user-facing docs
2. **Add runtime check** to fail fast with clear error:
   ```rust
   if self.sp >= STACK_SIZE {
       return Err("stack overflow: VM stack limit reached; reduce literal size or configure a larger VM stack".to_string());
   }
   ```
3. **Provide workaround examples** in documentation:
   ```flux
   // Instead of: map(huge_array, fn)
   // Use manual iteration:
   let result = [];
   for item in huge_array {
       push(result, fn(item));
   }
   ```

This is **not recommended** as it significantly limits the usefulness of these base functions.

### Alternative: Accept Current Limitation

### Alternative: Accept Current Limitation

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Alternative: Accept Current Limitation

If fixing the stack overflow is not immediately feasible, we should:

1. **Document the limitation clearly** in user-facing docs
2. **Add runtime check** to fail fast with clear error:
   ```rust
   if self.sp >= STACK_SIZE {
       return Err("stack overflow: VM stack limit reached; reduce literal size or configure a larger VM stack".to_string());
   }
   ```
3. **Provide workaround examples** in documentation:
   ```flux
   // Instead of: map(huge_array, fn)
   // Use manual iteration:
   let result = [];
   for item in huge_array {
       push(result, fn(item));
   }
   ```

This is **not recommended** as it significantly limits the usefulness of these base functions.

### Alternative: Accept Current Limitation

### Alternative: Accept Current Limitation

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- Proposal 0020: map/filter/fold Base Functions
- [src/runtime/vm/mod.rs](../src/runtime/vm/mod.rs) - VM stack limit and push overflow behavior
- [benches/map_filter_fold_bench.rs](../benches/map_filter_fold_bench.rs) - Performance benchmarks

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
