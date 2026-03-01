- Feature Name: NaN-boxing Runtime Optimization
- Start Date: 2026-02-23
- Proposal PR: 
- Flux Issue: 

# Proposal 0041: NaN-boxing Runtime Optimization

## Summary
[summary]: #summary

Evaluate NaN-boxing as a future runtime representation optimization for Flux `Value` storage.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Goals

- Quantify whether NaN-boxing improves end-to-end performance on representative workloads.
- Preserve exact language semantics and diagnostics.
- Keep VM/JIT behavioral parity.
- Make rollout reversible behind a feature flag during evaluation.

### Non-Goals

- Changing primop/Base routing architecture.
- Changing numeric semantics (integer math behavior, float behavior, error behavior).
- Immediate default-on rollout.

### Goals

### Non-Goals

### Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Problem:** Flux currently prioritizes correctness and maintainability in runtime value representation. For numeric-heavy programs, representation-level overhead (tag checks,...
- **Problem:** Flux currently prioritizes correctness and maintainability in runtime value representation. For numeric-heavy programs, representation-level overhead (tag checks, indirection, a...
- **Phase 1: Benchmark + Profiling Baseline:** - Freeze a benchmark suite covering: - numeric-heavy workloads - mixed workloads - non-numeric workloads - Capture baseline metrics: - wall-clock time - allocations / GC pressur...
- **Phase 2: Experimental Runtime Representation:** - Implement NaN-boxing behind a non-default compile feature (for example `nan-boxing-exp`). - Keep old representation path intact for A/B testing. - Ensure serialization/cache b...
- **Phase 3: Correctness + Parity Hardening:** - VM/JIT parity tests must pass unchanged. - Edge-case numeric tests (NaN, infinities, signed zero) must be explicit. - GC integration and tracing invariants must be validated u...
- **Evaluation Gates:** All gates must pass: 1. Correctness: - no semantic regressions in `cargo test --all --all-features` - no new VM/JIT parity mismatches 2. Performance: - meaningful end-to-end gai...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Non-Goals

- Changing primop/Base routing architecture.
- Changing numeric semantics (integer math behavior, float behavior, error behavior).
- Immediate default-on rollout.

### Non-Goals

### Risks

- Runtime complexity increase (tagging/decoding paths).
- GC/tracing correctness risk when mixing tagged immediates and heap pointers.
- Portability concerns across targets and compilers.
- Debuggability regression due to denser representation.

### Non-Goals

### Risks

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Phase 4: Adoption Decision

Promote only if all adoption gates pass (see below). Otherwise, keep as optional/experimental or remove.

### Phase 4: Adoption Decision

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

### Open Questions

- What numeric benchmark threshold justifies complexity?
- Should NaN-boxing be VM-only first, then aligned with JIT?
- Do we keep dual representation long-term or remove one path after decision?

### Open Questions

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
