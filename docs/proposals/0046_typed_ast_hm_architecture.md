- Feature Name: Typed AST + TypeEnv Architecture (Post-0.0.4 HM Evolution)
- Start Date: 2026-02-26
- Status: Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0046: Typed AST + TypeEnv Architecture (Post-0.0.4 HM Evolution)

## Summary
[summary]: #summary

Move Flux HM typing from the current dual artifact model:
- `TypeEnv` (`name -> scheme`)
- `ExprTypeMap` (internal expression-id -> `InferType`)

## Motivation
[motivation]: #motivation

Current 0.0.4 HM architecture is intentionally pragmatic and low-risk, but has structural limits: Current 0.0.4 HM architecture is intentionally pragmatic and low-risk, but has structural limits:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Make expression typing first-class and structural (typed node), not indirect map lookup.
2. Preserve HM behavior and diagnostics policy already stabilized in 0.0.4.
3. Remove pointer-identity dependence for typed validation paths.
4. Provide a clean foundation for later HM improvements (module/generic/member edge cases).
5. Align with 044 phase-pipeline modularization so typed data flows explicitly between passes.

### 4. Non-Goals

1. New language syntax or runtime features.
2. Higher-rank polymorphism.
3. Trait/typeclass system.
4. Effect-system semantic redesign.

### 3. Goals

### 4. Non-Goals

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **5.1 Typed Program Artifacts:** struct TypedExpr { ty: InferType, span: Span, } ``` - **5.2 Canonical data flow:** Pipeline shape: 1. Parse/transform (desugar/fold/rename) to...
- **5.1 Typed Program Artifacts:** Introduce a typed-pass output: ```rust struct TypedProgram { program: Program, type_env: TypeEnv, typed_exprs: TypedExprIndex, }
- **5.2 Canonical data flow:** Pipeline shape: 1. Parse/transform (desugar/fold/rename) to final compile AST. 2. HM inference on final AST. 3. Produce `TypedProgram` (`TypeEnv + typed expressions`). 4. Valida...
- **5.3 Callsite contract:** Typed validators must accept typed node info from `TypedExprIndex`: - let initializer checks - return-tail checks - condition/guard checks - operator/index/member checks - contr...
- **Phase A: Typed artifact introduction (behavior-preserving):** 1. Add `TypedProgram` and typed-expression index types. 2. Keep current HM map internals as adapter while exposing typed artifact API. 3. Keep all existing diagnostics and parit...
- **Phase B: Validation callsite cutover:** 1. Replace direct map/pointer lookups in compiler validation code with typed artifact access. 2. Ensure unresolved handling flows through existing strict policy. 3. Remove fallb...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. New language syntax or runtime features.
2. Higher-rank polymorphism.
3. Trait/typeclass system.
4. Effect-system semantic redesign.

### 4. Non-Goals

### 10. Rollout and Risk Control

1. Land typed artifact API first with adapter compatibility.
2. Migrate callsites incrementally behind stable tests.
3. Cut pointer identity only after parity suite is green.
4. Snapshot updates allowed only for intentional policy changes.

Risks:
- large AST plumbing diff
- subtle diagnostics ordering drift
- transform/id consistency bugs

Mitigations:
- phase-by-phase gating
- parity suite lock
- invariant tests for typed artifact completeness.

### 4. Non-Goals

### 10. Rollout and Risk Control

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
