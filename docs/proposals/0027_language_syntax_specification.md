- Feature Name: Flux Language Syntax Specification
- Start Date: 2026-02-12
- Status: Partially Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0027: Flux Language Syntax Specification

## Summary
[summary]: #summary

This document defines the complete syntax for Flux as a pure functional language. It covers current syntax (what exists today), confirmed additions, and the target syntax for the full language. Every construct includes grammar rules and examples.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Breaking Changes from Current Syntax

| Current | New | Reason |
|---------|-----|--------|
| `->` in match arms | `=>` | Distinguish from lambda `->` and type arrows `->` |
| `Some`/`None` as keywords | `Some`/`None` as ADT constructors | Generalized via `type Option<T> = Some(T) \| None` |
| `Left`/`Right` as keywords | `Ok`/`Err` convention (or user-defined) | Replace with proper Result type |
| Semicolons (optional) | No semicolons | Expression-based, newline-separated |
| `Assign` statement | Removed | Pure FP — no reassignment (except inside actors) |

### Breaking Changes from Current Syntax

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **1.1 Keywords:** **Current (13):** ``` let fn if else return true false match module import as Some None Left Right ``` - **1.2 Operators:** **Current:** ``` + - * / % Arithm...
- **1.1 Keywords:** **Current (13):** ``` let fn if else return true false match module import as Some None Left Right ```
- **1.2 Operators:** **Current:** ``` + - * / % Arithmetic == != < > <= >= Comparison && || ! Logical |> Pipe -> Arrow (lambdas, match arms) = Binding . Member access ```
- **1.3 Precedence Table (Low to High):** | Level | Operators | Associativity | |-------|-----------|---------------| | 1 | `\|>` | Left | | 2 | `>>` `<<` | Left | | 3 | `\|\|` | Left | | 4 | `&&` | Left | | 5 | `==` `!...
- **1.4 Comments:** ```flux // Single-line comment
- **1.5 Literals:** // Floats 3.14 -0.5 1.0e10 // scientific notation (new) 1_000.50

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

- [Rust Reference](https://doc.rust-lang.org/reference/) — Pattern matching, traits, match syntax
- [Elm Guide](https://guide.elm-lang.org/) — Pure FP syntax, no side effects
- [Gleam Language Tour](https://gleam.run/book/tour/) — Friendly FP syntax, Result types
- [Koka Documentation](https://koka-lang.github.io/koka/doc/index.html) — Effect system syntax
- [F# Language Reference](https://learn.microsoft.com/en-us/dotnet/fsharp/) — Pipe operator, computation expressions

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
