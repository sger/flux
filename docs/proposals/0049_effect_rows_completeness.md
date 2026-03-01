- Feature Name: Effect Rows Completeness and Principal Solving
- Start Date: 2026-02-26
- Proposal PR: pending (feature/type-system merge PR)
- Flux Issue: pending (type-system merge-readiness tracker, March 1, 2026)

# Proposal 0049: Effect Rows Completeness and Principal Solving

## Summary
[summary]: #summary

Complete Flux effect-row solving so outcomes are principal, deterministic, and aligned between HM inference and compiler validation.

## Motivation
[motivation]: #motivation

`0042` established practical row constraints; this proposal hardens completeness and ownership boundaries for merge-quality semantics.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Define principal row-solving behavior for supported row forms.
2. Specify deterministic normalization/equivalence rules.
3. Lock HM/compiler ownership boundaries.
4. Guarantee deterministic diagnostics for row failures.
5. Preserve VM/JIT parity for compile-time row diagnostics.

### 4. Non-Goals

1. Full row-polymorphism redesign.
2. New user-facing effect syntax.
3. Runtime effect representation changes.
4. Capability/security effect extensions.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Consolidated technical points

- Solver completeness matrix should classify covered behavior (`complete | partial | unsupported`).
- HM responsibilities: preserve resolved effect components in inferred function types.
- Compiler responsibilities: solve row constraints at call/boundary validation points.
- Integration rule: HM and compiler must not silently widen obligations.
- Deterministic diagnostics contract: stable code/title/primary-label intent and ordering.

### Detailed specification (migrated legacy content)

This proposal is the row-completeness hardening companion to `0042` and governs principal solving expectations.

### Historical notes

- Legacy content normalized into canonical template structure.

## Drawbacks
[drawbacks]: #drawbacks

- Tightening row solving may surface new compile-time failures in gradual paths.
- Additional targeted fixture/snapshot maintenance is required.

### 13. Risks and Mitigations

1. Risk: false-positive tightening in gradual code.
   - Mitigation: strict-first rollout and exception policy.
2. Risk: diagnostic churn.
   - Mitigation: snapshot gate and deterministic ordering.
3. Risk: HM/compiler disagreement.
   - Mitigation: shared compatibility checks and regression fixtures.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### 5. Semantic Decisions (Locked)

1. Row semantics: set-like concrete atoms plus row variables.
2. Concrete effect ordering is non-semantic (canonicalize before comparison).
3. Duplicate atoms are idempotent.
4. Missing-atom subtraction is a deterministic constraint failure.
5. Unresolved row vars in strict-relevant typed paths fail at compile time.
6. Prefer existing diagnostics families (`E419`-`E422`, `E400`) unless a new class is required.

## Prior art
[prior-art]: #prior-art

- Builds on the same row-polymorphism prior art as 0042.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions remain for this proposal's current merge scope.

## Future possibilities
[future-possibilities]: #future-possibilities

- Any post-MVP row-expansion work must preserve diagnostics stability and fixture coverage.
