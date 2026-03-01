- Feature Name: Any Fallback Reduction and Typed-Path Soundness
- Start Date: 2026-02-26
- Proposal PR: 
- Flux Issue: 

# Proposal 0051: Any Fallback Reduction and Typed-Path Soundness

## Summary
[summary]: #summary

Reduce accidental unsoundness by replacing silent `Any` degradation with concrete type constraints or explicit unresolved diagnostics in high-value typed paths.

## Motivation
[motivation]: #motivation

`Any` is a deliberate gradual escape hatch, but current behavior still contains fallback sites that are effectively accidental: `Any` is a deliberate gradual escape hatch, but current behavior still contains fallback sites that are effectively accidental:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Inventory and classify all `Any` fallback hotspots.
2. Define clear allow/disallow policy for fallback.
3. Tighten typed/HM-known contexts first.
4. Keep intentional gradual behavior explicit and documented.
5. Improve deterministic diagnostics where fallback is blocked.

### 4. Non-Goals

1. Eliminate `Any` from Flux entirely.
2. Force fully static typing for all programs.
3. Introduce new syntax for gradual boundaries.
4. Redesign runtime boundary checking model.

### 3. Goals

### 4. Non-Goals

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **5.1 Allowed fallback:** 1. source value is explicitly dynamic/unknown, 2. type information is truly unavailable after HM + contract resolution, 3. context is explicitly grad...
- **5.1 Allowed fallback:** Fallback to `Any` remains allowed when: 1. source value is explicitly dynamic/unknown, 2. type information is truly unavailable after HM + contract resolution, 3. context is exp...
- **5.2 Disallowed fallback:** Fallback is disallowed when: 1. HM has concrete type evidence at the expression site, 2. function/module contracts provide concrete expectations, 3. strict boundary validation r...
- **5.3 When disallowed fallback is hit:** 1. emit concrete mismatch diagnostics (`E300`) when types conflict, 2. emit unresolved-boundary diagnostics (`E425`) only when type is genuinely unresolved after evidence paths...
- **6. Hotspot Inventory Matrix (Required):** Each hotspot row must include: current behavior, target behavior, severity, and rollout stage.
- **Stage 1 (strict + typed known contexts):** 1. Remove disallowed fallback in strict and typed-contract-sensitive paths. 2. Keep explicit diagnostics stable.

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. Eliminate `Any` from Flux entirely.
2. Force fully static typing for all programs.
3. Introduce new syntax for gradual boundaries.
4. Redesign runtime boundary checking model.

### 4. Non-Goals

### 12. Risks and Mitigations

1. Risk: breaking existing gradual code unexpectedly.
   - Mitigation: staged rollout and explicit allowed-fallback table.
2. Risk: increased diagnostic volume.
   - Mitigation: keep mismatch/unresolved split clear and deterministic.
3. Risk: implementation drift across HM and compiler validators.
   - Mitigation: hotspot matrix ownership and shared regression tests.

### 4. Non-Goals

### 12. Risks and Mitigations

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

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
