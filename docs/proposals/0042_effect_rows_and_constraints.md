- Feature Name: Effect Rows and Constraint Solving for `with e`
- Start Date: 2026-02-25
- Proposal PR: pending (feature/type-system merge PR)
- Flux Issue: pending (type-system merge-readiness tracker, March 1, 2026)

# Proposal 0042: Effect Rows and Constraint Solving for `with e`

## Summary
[summary]: #summary

Define practical row-constraint semantics for `with e` so effect polymorphism is predictable, normalized, and diagnosable.

## Motivation
[motivation]: #motivation

Proposal 0032 introduced effect-aware typing. This proposal formalizes row-style constraints so higher-order effect propagation and handler discharge are checked consistently.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 2. Goals

1. Make `with e` constraints explicit and deterministic.
2. Support row extension/normalization for currently supported forms.
3. Improve diagnostics for missing/extra/incompatible effect obligations.
4. Preserve existing Flux surface syntax.

### 3. Non-Goals (for this proposal)

1. Full research-grade row-polymorphism redesign.
2. Runtime representation changes for effects.
3. Capability/security effect system.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Consolidated technical points

- Surface model stays `with ...`, with row-aware solver behavior internally.
- Solver handles equality/subset/subtraction paths used by typed checks.
- Canonicalization treats concrete effect ordering as non-semantic.
- Diagnostics must stay deterministic (code/title/primary-label intent).

### Detailed specification (migrated legacy content)

This proposal remains the canonical row-constraint policy for current `with e` behavior, together with 0049 completeness hardening.

### Historical notes

- Legacy text normalized to template while preserving semantics.

## Drawbacks
[drawbacks]: #drawbacks

- Tighter effect checking can initially increase compile-time diagnostics for gradual code.
- Row diagnostics need disciplined snapshot governance to avoid churn.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- Keep Flux syntax ergonomic while implementing principled solver behavior.
- Prioritize deterministic diagnostics over aggressively exposing internal row terms.

## Prior art
[prior-art]: #prior-art

- Koka / Eff row-polymorphism traditions informed constraints and discharge ideas.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

### 12. Resolution Log (March 1, 2026)

1. **Expose explicit row-tail syntax in user code**
   - Outcome: **Rejected for v0.0.4**.
   - Decision: keep `with ...` surface syntax only.
2. **Add absence constraints in v1**
   - Outcome: **Deferred (linked follow-up proposal)**.
   - Follow-up: `docs/proposals/0049_effect_rows_completeness.md`.
3. **How much row detail appears by default in diagnostics**
   - Outcome: **Accepted now**.
   - Decision: default diagnostics stay concise; internal row detail appears only when materially actionable.
4. **Require explicit effect annotations for strict public higher-order APIs**
   - Outcome: **Deferred (linked follow-up proposal)**.
   - Follow-up: `docs/proposals/0049_effect_rows_completeness.md` and strict-boundary policy updates.

## Future possibilities
[future-possibilities]: #future-possibilities

- Expand absence constraints and advanced rows only with dedicated proposal + fixture coverage.
