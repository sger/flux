- Feature Name: Contextual Diagnostics — Call-Site Arguments, Let Annotations, and Function Return Types
- Start Date: 2026-02-28
- Proposal PR: pending (feature/type-system merge PR)
- Flux Issue: pending (type-system merge-readiness tracker, March 1, 2026)

# Proposal 0058: Contextual Diagnostics — Call-Site Arguments, Let Annotations, and Function Return Types

## Summary
[summary]: #summary

Complete the contextual diagnostics pass from 0057 by covering call-argument mismatches, let-annotation mismatches, and function-return mismatches.

## Motivation
[motivation]: #motivation

These three paths are high-frequency diagnostics where named context and dual spans materially improve fix speed.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Name called function + include definition-site context for argument mismatch diagnostics.
2. Show annotation span + value span for let mismatch.
3. Show return annotation span + return expression span for return mismatch.
4. Reuse 0057 infrastructure without new HM semantics.

### 4. Non-Goals

1. No HM rule changes.
2. No pass-2 continuation policy change.
3. No effect/purity diagnostics expansion.
4. No pattern-destructuring-specific diagnostics.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Consolidated technical points

- Uses `ReportContext` paths introduced in 0057.
- Keeps overlap suppression and concrete-type guards.
- Locks snapshot output for callsite/let/return contexts.

### Detailed specification (migrated legacy content)

Coverage and semantics are validated by dedicated fixtures and diagnostics snapshots.

### Historical notes

- Normalized to canonical template during branch integration.

## Drawbacks
[drawbacks]: #drawbacks

- More contextual labels increase snapshot churn risk if not tightly governed.

### 14. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Regressions in callsite HM paths | Focused compiler_rules/type_inference fixture coverage |
| Duplicate emissions | Preserve existing suppression strategy |
| Span plumbing mistakes | Add precise span tests and snapshot verification |

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- Extending existing context infrastructure is lower risk than introducing parallel reporting paths.

## Prior art
[prior-art]: #prior-art

- Builds directly on 0057 architecture.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions for current implemented scope.

## Future possibilities
[future-possibilities]: #future-possibilities

- Extend contextual diagnostics to additional boundary and effect mismatch classes.
