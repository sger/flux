- Feature Name: Flux Grammar Improvements
- Start Date: 2026-02-26
- Proposal PR: 
- Flux Issue: 

# Proposal 0037: Flux Grammar Improvements

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Flux Grammar Improvements in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

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

- **Consolidated technical points:** - **__preamble__:** This document analyzes the current Flux grammar and proposes targeted improvements for consistency, expressiveness, and maintainability. - **Table of Content...
- **Detailed specification (migrated legacy content):** This document analyzes the current Flux grammar and proposes targeted improvements for consistency, expressiveness, and maintainability.
- **Table of Contents:** 1. [Current Grammar Analysis](#current-grammar-analysis) 2. [Identified Issues](#identified-issues) 3. [Proposed Improvements](#proposed-improvements) 4. [Formal Grammar Specifi...
- **Token Types (Current):** ``` Literals: Int, Float, String, Ident Operators: + - * / ! < > == != = Delimiters: ( ) { } [ ] , ; : . -> Keywords: let fn if else return true false module import as Some None...
- **Expression Types (Current):** | Expression | Example | Notes | |------------|---------|-------| | Identifier | `foo`, `Math.sqrt` | Qualified names via MemberAccess | | Integer | `42` | i64 | | Float | `3.14...
- **Statement Types (Current):** | Statement | Example | |-----------|---------| | Let | `let x = 1;` | | Assign | `x = 2;` | | Return | `return x;` | | Function | `fn foo(x) { ... }` | | Module | `module M { ....

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

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
