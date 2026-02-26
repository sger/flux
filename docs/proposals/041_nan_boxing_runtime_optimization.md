# Proposal 041: NaN-boxing Runtime Optimization

**Status:** Proposed  
**Date:** 2026-02-23  
**Depends on:** None

---


**Status:** Proposed  
**Priority:** Medium  
**Created:** 2026-02-23  
**Related:** Proposal 031 (Cranelift JIT Backend), Proposal 034 (Builtin PrimOps), Proposal 038 (Deterministic Effect Replay)

## Summary

Evaluate NaN-boxing as a future runtime representation optimization for Flux `Value` storage.

This is an optimization track only. It does not change Flux language semantics. It should be pursued only if benchmarks show numeric-heavy workloads are materially limited by current value representation overhead.

## Problem

Flux currently prioritizes correctness and maintainability in runtime value representation. For numeric-heavy programs, representation-level overhead (tag checks, indirection, allocation patterns) may become a bottleneck.

NaN-boxing can reduce representation overhead by encoding immediate values and tags in 64-bit words, potentially improving VM dispatch and memory locality.

## Goals

- Quantify whether NaN-boxing improves end-to-end performance on representative workloads.
- Preserve exact language semantics and diagnostics.
- Keep VM/JIT behavioral parity.
- Make rollout reversible behind a feature flag during evaluation.

## Non-Goals

- Changing primop/Base routing architecture.
- Changing numeric semantics (integer math behavior, float behavior, error behavior).
- Immediate default-on rollout.

## Design (Phased)

### Phase 1: Benchmark + Profiling Baseline

- Freeze a benchmark suite covering:
  - numeric-heavy workloads
  - mixed workloads
  - non-numeric workloads
- Capture baseline metrics:
  - wall-clock time
  - allocations / GC pressure
  - instruction counts where available

### Phase 2: Experimental Runtime Representation

- Implement NaN-boxing behind a non-default compile feature (for example `nan-boxing-exp`).
- Keep old representation path intact for A/B testing.
- Ensure serialization/cache boundaries remain explicit and safe.

### Phase 3: Correctness + Parity Hardening

- VM/JIT parity tests must pass unchanged.
- Edge-case numeric tests (NaN, infinities, signed zero) must be explicit.
- GC integration and tracing invariants must be validated under stress.

### Phase 4: Adoption Decision

Promote only if all adoption gates pass (see below). Otherwise, keep as optional/experimental or remove.

## Risks

- Runtime complexity increase (tagging/decoding paths).
- GC/tracing correctness risk when mixing tagged immediates and heap pointers.
- Portability concerns across targets and compilers.
- Debuggability regression due to denser representation.

## Evaluation Gates

All gates must pass:

1. Correctness:
   - no semantic regressions in `cargo test --all --all-features`
   - no new VM/JIT parity mismatches
2. Performance:
   - meaningful end-to-end gains on numeric benchmarks (target threshold to be decided before rollout)
   - no unacceptable regressions on mixed/non-numeric suites
3. Operability:
   - clear diagnostics and debugging strategy
   - maintainable code paths and documentation

## Testing Plan

- Existing full test suite under both value representations.
- Dedicated numeric conformance tests:
  - NaN propagation
  - infinities
  - signed zero behavior
  - int/float boundary behavior
- Benchmark CI job (non-blocking during evaluation, blocking before adoption).

## Rollout

1. Land feature-gated prototype.
2. Publish benchmark report and risk assessment.
3. Decide default policy:
   - default off (experimental),
   - default on (if gates pass),
   - or abandoned (if complexity outweighs benefit).

## Open Questions

- What numeric benchmark threshold justifies complexity?
- Should NaN-boxing be VM-only first, then aligned with JIT?
- Do we keep dual representation long-term or remove one path after decision?
