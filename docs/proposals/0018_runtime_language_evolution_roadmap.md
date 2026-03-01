- Feature Name: Runtime and Language Evolution Roadmap
- Start Date: 2026-02-08
- Proposal PR: 
- Flux Issue: 

# Proposal 0018: Runtime and Language Evolution Roadmap

## Summary
[summary]: #summary

| # | Item | Status | Effort | Impact |
|---|------|--------|--------|--------|
| 1.1 | Tail-Call Elimination | Not started | 1 week | Unbounded recursion |
| 1.2 | Liveness Analysis | Not started | 3-4 days | Foundation for 1.3 |
| 1.3 | Accumulator Array Reuse | Not started | 1 week | O(n^2) → O(n) |
| 1.4 | Constant Folding | Not started | 3-4 days | 10-20% bytecode reduction |
| 1.5 | Constant Pool Dedupe | Not started | half day | 20-40% pool reduction |
| 1.6 | Symbol Interning Pipeline | 50% (lexer done) | 1-2 weeks | 15-30% faster symbol resolution |
| 1.7 | Arena Allocation | Not started | 2-3 weeks | Faster parse + better cache |
| 1.8 | VM Dispatch Table | Partial (LLVM helps) | 3-4 days | Predictable dispatch, enables threading |

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

- **Consolidated technical points:** - **__preamble__:** **Implementation Order:** 019 → 016 → 017 → 018 (this) - **What Flux Has Today:** | Area | Summary | |------|---------| | **Expressions** | 20 variants (lite...
- **Detailed specification (migrated legacy content):** **Implementation Order:** 019 → 016 → 017 → 018 (this)
- **What Flux Has Today:** | Area | Summary | |------|---------| | **Expressions** | 20 variants (literals, if/else, match, call, lambda, pipe, array, hash, Some/Left/Right) | | **Statements** | 7 variant...
- **What Flux Does NOT Have:** | Gap | Impact | |-----|--------| | Tail-call optimization | Stack overflow on deep recursion (>2048 frames) | | Constant folding | `5 + 3` compiled as two constants + OpAdd ins...
- **Tier 1: Performance Foundations (Highest Priority):** These are pure runtime/compiler improvements with no syntax changes. They make existing Flux programs faster and more capable.
- **1.1 Tail-Call Elimination (Proposal 0016, Phase 1):** **What:** Add `OpTailCall` opcode. Self-recursive calls in tail position reuse the current frame instead of pushing a new one.

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

### Decision Points

These design questions should be resolved before implementation begins:

1. **Destructuring syntax**: `let [a, b] = x` (array) vs `let (a, b) = x` (tuple) — if tuples exist, both needed?

2. **For-loop desugaring**: Counter-based (preserves array semantics) vs iterator-protocol (future-proof but complex)?

3. **ADT representation**: Tagged union in `Object` enum (fast, requires variant) vs hash-table encoding (flexible, slower)?

4. **Record vs Hash overlap**: Should `{name: "Alice", age: 30}` be a hash or a record? Or should records use different syntax (`Point { x: 1, y: 2 }`)?

5. **Range semantics**: Lazy (generates values on demand) vs eager (creates array)? Lazy is better but needs iterator protocol.

6. **Type annotation syntax**: `fn f(x: Int) -> String` or inferred-only? Adding annotations early constrains future type system design.

### Decision Points

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

### Prioritized Roadmap

Features are grouped into tiers based on **impact** (what it unlocks for users), **effort** (implementation complexity), and **dependencies** (what must come first).

### Tier 5: Advanced Features (Future)

These are substantial features that require significant design work and should be considered after Tiers 1-3 are stable.

| Feature | Description | Effort | Dependencies |
|---------|-------------|--------|--------------|
| **List comprehensions** | `[x * 2 for x in arr if x > 0]` | 2 weeks | For loops, arrays |
| **Type inference** | Hindley-Milner style type checking | 6-8 weeks | ADTs, records |
| **Effect system** | `fn f() with IO { ... }` | 8-10 weeks | Type system |
| **Persistent collections** | Rc-based cons list + HAMT (Proposal 0017 revised) | 4-6 weeks | TCE |
| **Concurrency (actors)** | `spawn`, message passing, supervision | 8-12 weeks | Effect system |
| **Package manager** | Dependency resolution, versioned modules | 6-8 weeks | Module system |
| **REPL** | Interactive read-eval-print loop | 2-3 weeks | None |
| **Debugger** | Step-through execution, breakpoints | 4-6 weeks | LSP, source maps |

### Prioritized Roadmap

### Tier 5: Advanced Features (Future)
