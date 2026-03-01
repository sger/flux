- Feature Name: Pure Functional Language Vision
- Start Date: 2026-02-12
- Proposal PR: 
- Flux Issue: 

# Proposal 0025: Pure Functional Language Vision

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Pure Functional Language Vision in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Syntax Decisions

| Choice | Flux | Rationale |
|--------|------|-----------|
| Blocks | `{ }` | Familiar to JS/Rust/C developers |
| Lambdas | `\x -> expr` | Short, unambiguous, visually evokes lambda |
| Match arms | `pattern => expr` | Rust-like (currently uses `->`, consider migrating to `=>`) |
| Type annotations | `x: Int` | Rust/TS style, not `x :: Int` (Haskell) |
| Generics | `<T>` | Rust/TS style, not `[T]` or `'a` |
| Pipe | `\|>` | F#/Elm/Elixir standard |
| Function keyword | `fn` | Short, clear (not `fn`, `func`, `def`, or `function`) |
| Comments | `//` and `/* */` | C-family standard |
| String interpolation | `"Hello, #{name}"` | Ruby-style, already implemented |
| No semicolons | Expression-based | Modern language trend, reduces noise |

### Syntax Decisions

### Syntax Decisions

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Vision:** Flux is a **pure functional programming language** with syntax that feels familiar to developers coming from JavaScript, TypeScript, and Rust. The goal is to make...
- **Vision:** Flux is a **pure functional programming language** with syntax that feels familiar to developers coming from JavaScript, TypeScript, and Rust. The goal is to make pure FP approa...
- **Guiding Principles:** 1. **Pure by default** — Functions have no side effects unless explicitly marked. Referential transparency is the norm. 2. **Expressions everywhere** — Everything returns a valu...
- **What "Pure" Means for Flux:** | Aspect | Pure FP Rule | Flux Approach | |--------|-------------|---------------| | Side effects | Tracked in type system | `with IO` annotation on effectful functions; pure fu...
- **Keep It Familiar:** Flux syntax should feel like a blend of **Rust** (types, match, traits) and **JavaScript/TypeScript** (braces, arrows, object literals) with FP ergonomics from **Elm** and **F#*...
- **Phase 1: Type Foundation:** The type system is the backbone of a pure FP language. Without it, purity cannot be enforced at compile time.

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

### Syntax Decisions

| Choice | Flux | Rationale |
|--------|------|-----------|
| Blocks | `{ }` | Familiar to JS/Rust/C developers |
| Lambdas | `\x -> expr` | Short, unambiguous, visually evokes lambda |
| Match arms | `pattern => expr` | Rust-like (currently uses `->`, consider migrating to `=>`) |
| Type annotations | `x: Int` | Rust/TS style, not `x :: Int` (Haskell) |
| Generics | `<T>` | Rust/TS style, not `[T]` or `'a` |
| Pipe | `\|>` | F#/Elm/Elixir standard |
| Function keyword | `fn` | Short, clear (not `fn`, `func`, `def`, or `function`) |
| Comments | `//` and `/* */` | C-family standard |
| String interpolation | `"Hello, #{name}"` | Ruby-style, already implemented |
| No semicolons | Expression-based | Modern language trend, reduces noise |

### Syntax Decisions

### Syntax Decisions

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

### Open Questions

1. **Match arm syntax**: Keep `->` or migrate to `=>`? The `=>` is more Rust-like, but `->` is already established in the codebase.

2. **Semicolons**: Currently expression-based but some semicolons exist. Fully remove them?

3. **Effect system complexity**: Start with just `IO` and `Fail`, or design the full algebraic effect system upfront?

4. **Currying performance**: Auto-currying has overhead. Accept it, or use a hybrid approach (curry only when partially applied)?

5. **Backward compatibility**: How to migrate existing `.flx` programs as the language evolves? Versioned syntax (`edition` like Rust)?

6. **Standard library scope**: Minimal (like Go) or batteries-included (like Python)?

### Open Questions

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
