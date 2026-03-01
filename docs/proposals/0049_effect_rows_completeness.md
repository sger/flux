- Feature Name: Effect Rows Completeness and Principal Solving
- Start Date: 2026-02-26
- Proposal PR: 
- Flux Issue: 

# Proposal 0049: Effect Rows Completeness and Principal Solving

## Summary
[summary]: #summary

Complete Flux effect-row semantics so row-variable solving is principal, deterministic, and consistent across HM-aware typing and compiler validation paths.

## Motivation
[motivation]: #motivation

`042` landed practical row constraints and useful diagnostics, but row reasoning is still partial in several high-value cases: `042` landed practical row constraints and useful diagnostics, but row reasoning is still partial in several high-value cases:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Define principal row-solving behavior for currently supported row forms.
2. Specify deterministic row normalization and equivalence.
3. Make HM/compiler ownership boundaries explicit and stable.
4. Guarantee deterministic diagnostics for unresolved/ambiguous row outcomes.
5. Preserve VM/JIT parity for compile-time row failures.

### 4. Non-Goals

1. Full research-grade row-polymorphism redesign.
2. New user-facing effect syntax.
3. Runtime effect representation changes.
4. Capability/security effect system extensions.

### 3. Goals

### 4. Non-Goals

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **6. Solver Completeness Matrix:** Each row below must be classified and covered by tests as `complete | partial | unsupported`. - **7.1 HM responsibilities:** 1. Carry functi...
- **6. Solver Completeness Matrix:** Each row below must be classified and covered by tests as `complete | partial | unsupported`.
- **7.1 HM responsibilities:** 1. Carry function effect sets consistently through inferred function types. 2. Preserve resolved effect components on inferred function types. 3. Avoid introducing ad hoc row fa...
- **7.2 Compiler responsibilities:** 1. Collect and solve row constraints from function contracts and call arguments. 2. Apply subset/equality/subtraction rules at call and ambient-boundary validation points. 3. Em...
- **7.3 Integration rule:** HM-derived function types and compiler row constraints must agree on function-effect compatibility; neither side may silently widen obligations.
- **8. Deterministic Diagnostics Contract:** For row-related failures, diagnostics must be deterministic in: 1. code, 2. title, 3. primary label intent text, 4. ordering (first material violation wins).

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. Full research-grade row-polymorphism redesign.
2. New user-facing effect syntax.
3. Runtime effect representation changes.
4. Capability/security effect system extensions.

### 4. Non-Goals

### 13. Risks and Mitigations

1. Risk: false-positive tightening in gradual code.
   - Mitigation: strict-first rollout and explicit exception table.
2. Risk: diagnostic churn.
   - Mitigation: snapshot gate and deterministic ordering rule.
3. Risk: HM/compiler disagreement on function effects.
   - Mitigation: shared compatibility checks and targeted regression tests.

### 4. Non-Goals

### 13. Risks and Mitigations

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### 5. Semantic Decisions (Locked)

1. Row semantics use set-like concrete atoms plus row variables.
2. Concrete-effect ordering is non-semantic; canonicalization is required before comparison.
3. Duplicate atoms are idempotent.
4. Subtraction removes concrete atoms only when present; missing-atom subtraction is a deterministic constraint violation.
5. Unresolved row variables in strict-relevant typed contexts are compile-time failures; in non-strict gradual contexts they may remain recoverable where explicitly allowed by policy.
6. Existing diagnostics classes are preferred (`E419`-`E422`, `E400`), with new codes only for materially distinct failure classes.

### 5. Semantic Decisions (Locked)

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

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
