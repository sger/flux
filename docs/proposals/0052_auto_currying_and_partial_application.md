- Feature Name: Auto-Currying and Placeholder Partial Application
- Start Date: 2026-02-26
- Status: Not Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0052: Auto-Currying and Placeholder Partial Application

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Auto-Currying and Placeholder Partial Application in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

Flux currently requires manual lambdas for partial application: Flux currently requires manual lambdas for partial application:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 6.3 No runtime effect redesign

This proposal does not change effect runtime model; it only extends callable application semantics while reusing current compile-time effect enforcement.

### Phase A: Syntax + AST

1. parse placeholder in call args,
2. represent placeholder as dedicated AST form,
3. reject placeholder outside call args.

### 6.3 No runtime effect redesign

### Phase A: Syntax + AST

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **1. Status/Header Metadata:** This proposal is execution-grade and implementation-oriented. It supersedes the high-level currying vision text in `0025_pure_fp_language_vision...
- **1. Status/Header Metadata:** This proposal is execution-grade and implementation-oriented. It supersedes the high-level currying vision text in `0025_pure_fp_language_vision.md` for delivery decisions, whil...
- **3.1 Callable kinds in scope:** The rules in this proposal apply uniformly to: 1. user-defined named functions, 2. lambdas/function expressions, 3. module-qualified functions, 4. builtins/base functions, 5. pr...
- **3.2 Arity model:** Each callable has fixed declared arity `N` at compile time where known.
- **3.3 Call normalization:** For `f(a1, ..., am)` where `m <= N`: 1. Create an argument template of length `N`. 2. Fill the first `m` slots with provided call arguments (including placeholders). 3. Fill rem...
- **3.4 Fill order:** When invoking a partial callable with new args, holes are filled strictly left-to-right by template slot index.

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 13. Risks and Mitigations

1. Risk: ambiguity between wildcard and placeholder `_`.
   - Mitigation: distinct parser contexts + AST forms.
2. Risk: arity behavior drift for builtins/primops.
   - Mitigation: single callable normalization path and dedicated lowering tests.
3. Risk: HM regressions for generic partial calls.
   - Mitigation: targeted generic fixtures and strict inference tests.
4. Risk: parity drift in diagnostics.
   - Mitigation: add focused parity snapshots for currying failure cases.

### 13. Risks and Mitigations

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
