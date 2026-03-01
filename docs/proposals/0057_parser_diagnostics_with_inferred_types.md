- Feature Name: Rich Diagnostics with Inferred Types — Elm-style Errors & Generic Parser
- Start Date: 2026-02-27
- Proposal PR: pending (feature/type-system merge PR)
- Flux Issue: pending (type-system merge-readiness tracker, March 1, 2026)

# Proposal 0057: Rich Diagnostics with Inferred Types — Elm-style Errors & Generic Parser

## Summary
[summary]: #summary

Use HM-inferred types to produce contextual diagnostics (arity, branch/arm mismatch, function mismatch decomposition) and add parser recovery abstractions that reduce ad hoc error wiring.

## Motivation
[motivation]: #motivation

This track improves diagnostic quality and consistency without changing language semantics.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Compile-time arity mismatch detection.
2. Contextual dual-label diagnostics for `if` and `match` mismatches.
3. Multiple independent type errors per file where safe.
4. Function mismatch decomposition (params/return).
5. Resilient parser recovery for malformed type annotations.
6. Reusable reporting architecture (`ReportContext`).

### 4. Non-Goals

1. No new syntax.
2. No runtime type-check semantics changes.
3. No HM rule changes.
4. No effect-policy changes.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Consolidated technical points

- Context-aware unification reporting is the core architecture change.
- Parser recovery helpers centralize token-sync behavior.
- Contextual diagnostics only fire for concrete, non-`Any` comparisons to avoid false positives.

### Detailed specification (migrated legacy content)

Behavior is validated by parser/type fixtures and snapshot tracks.

### Historical notes

- Proposal content normalized to template form during this branch.

## Drawbacks
[drawbacks]: #drawbacks

- Richer diagnostics increase snapshot maintenance surface.

### 14. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| False positives on gradual code | Guard on concrete, non-`Any` type pairs |
| Misleading cascades | Keep controlled continuation strategy |
| Recovery token drift | Conservative synchronization points |

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- Architectural reuse is favored over per-case custom diagnostics.

## Prior art
[prior-art]: #prior-art

- Elm-style diagnostic UX as quality benchmark.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions for the implemented scope.

## Future possibilities
[future-possibilities]: #future-possibilities

- Extend contextual reporting to additional type/effect classes via the same context architecture.
