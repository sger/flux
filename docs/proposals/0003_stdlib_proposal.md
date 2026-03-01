- Feature Name: 003_stdlib_proposal
- Start Date: 2026-02-26
- Proposal PR: 
- Flux Issue: 

# Proposal 0003: 003_stdlib_proposal

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for 003_stdlib_proposal in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

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

- **Consolidated technical points:** - **__preamble__:** This document outlines the plan for implementing a comprehensive standard library for Flux called **Flow**, including prerequisites, new language features, a...
- **Flux Flow Library Proposal:** This document outlines the plan for implementing a comprehensive standard library for Flux called **Flow**, including prerequisites, new language features, and module designs.
- **Table of Contents:** 1. [Current State](#current-state) 2. [Phase 0: New Object Types](#phase-0-new-object-types) 3. [Phase 1: Compiler Prerequisites](#phase-1-compiler-prerequisites) 4. [Phase 2: N...
- **Object Types (8 total):** | Type | Rust Representation | Hashable | Description | |------|---------------------|----------|-------------| | `Integer` | `i64` | Yes | 64-bit signed integer | | `Float` | `...
- **Base Functions (7 total):** | Function | Signature | Description | |----------|-----------|-------------| | `print` | `(...args) -> None` | Print to stdout | | `len` | `String \| Array -> Int` | Get length...
- **Operators (7 total):** | Operator | Token | Opcode | Notes | |----------|-------|--------|-------| | `+` | `Plus` | `OpAdd` | Addition, string concat | | `-` | `Minus` | `OpSub` | Subtraction | | `*`...

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

### References

- Haskell Prelude: https://hackage.haskell.org/package/base/docs/Prelude.html
- Elm Core: https://package.elm-lang.org/packages/elm/core/latest/
- Rust std: https://doc.rust-lang.org/std/
- F# Core: https://fsharp.github.io/fsharp-core-docs/

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

### Open Questions

1. **Module loading:** How should stdlib modules be automatically available?
   - Implicit import?
   - Prelude-style auto-import?
   - Explicit import required?

2. **Naming conventions:**
   - `Flow.List` vs `List` vs `flow.list`?
   - `is_empty` vs `isEmpty` vs `empty?`?

3. **Error handling:**
   - Add `Either` type now or later?
   - How to handle errors in base functions (return None vs Left vs panic)?

4. **Performance:**
   - When is TCO needed?
   - Which functions must be base functions vs pure Flux?

### Open Questions

## Future possibilities
[future-possibilities]: #future-possibilities

### Milestone 1: Core Operators (Week 1-2)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Add `<=` operator | Critical | Small | None |
| Add `>=` operator | Critical | Small | None |
| Add `%` operator | High | Small | None |
| Add `&&` operator (short-circuit) | Critical | Medium | None |
| Add `\|\|` operator (short-circuit) | Critical | Medium | None |

### Milestone 2: Essential Base Functions (Week 3-4)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Add `concat(arr1, arr2)` | Critical | Small | None |
| Add `reverse(arr)` | High | Small | None |
| Add `slice(arr, start, end)` | High | Small | None |
| Add type checking base functions | High | Medium | None |
| Add `keys(h)`, `values(h)` | Medium | Small | None |

### Milestone 3: String Base Functions (Week 5-6)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Add `split(s, delim)` | High | Small | None |
| Add `join(arr, delim)` | High | Small | None |
| Add `trim(s)` | Medium | Small | None |
| Add `upper(s)`, `lower(s)` | Medium | Small | None |
| Add `substring(s, start, end)` | Medium | Small | None |
| Add `chars(s)` | Medium | Small | None |

### Milestone 4: Math Base Functions (Week 7)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Add `abs(n)` | High | Small | None |
| Add `min(a, b)`, `max(a, b)` | High | Small | None |
| Add `floor`, `ceil`, `round` | Medium | Small | None |
| Add `sqrt`, `pow` | Medium | Small | None |

### Milestone 5: Core Flow Modules (Week 8-10)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Implement `Flow.List` | Critical | Large | M1, M2 |
| Implement `Flow.Option` | High | Medium | None |
| Implement `Flow.Either` | High | Medium | Either type |
| Implement `Flow.Math` | Medium | Medium | M1, M4 |
| Implement `Flow.Func` | Medium | Small | None |
| Implement `Flow.String` | Medium | Medium | M3 |

### Milestone 6: Advanced Features (Week 11+)

| Task | Priority | Effort | Dependencies |
|------|----------|--------|--------------|
| Add `Either` type (Left/Right) | High | Large | None |
| Add `Tuple` type | Medium | Large | None |
| Implement TCO | Medium | Large | None |
| Pattern matching for arrays | Low | Large | None |
| Implement `Flow.Dict` | Medium | Medium | Either type |

### Milestone 1: Core Operators (Week 1-2)

### Milestone 2: Essential Base Functions (Week 3-4)

### Milestone 3: String Base Functions (Week 5-6)

### Milestone 4: Math Base Functions (Week 7)

### Milestone 5: Core Flow Modules (Week 8-10)

### Milestone 6: Advanced Features (Week 11+)
