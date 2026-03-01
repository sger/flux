- Feature Name: Hygienic Macro System for Flux
- Start Date: 2026-02-20
- Proposal PR: 
- Flux Issue: 

# Proposal 0040: Hygienic Macro System for Flux

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Hygienic Macro System for Flux in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Syntax (Phase 1)

Definition:

```flux
macro_rules! assert_eq {
    ($a, $b) => {
        if $a != $b {
            panic("assert_eq failed")
        }
    }
}
```

Use:

```flux
assert_eq!(x + 1, y)
```

Rules:

- invocation uses `name!(...)`
- macro names live in a dedicated namespace
- explicit import/export for cross-module usage

### 3. Syntax (Phase 1)

Definition:

Use:

```flux
assert_eq!(x + 1, y)
```

Rules:

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **1. Goal:** Add a macro system that increases language expressiveness without sacrificing determinism, tooling, or VM/JIT parity. - **Phase 1 (v1): Hygienic Pattern Macros:**...
- **1. Goal:** Add a macro system that increases language expressiveness without sacrificing determinism, tooling, or VM/JIT parity.
- **Phase 1 (v1): Hygienic Pattern Macros:** - `macro_rules`-style expression/statement macros - token-tree/pattern matching + template expansion - no arbitrary compile-time execution - module-local macro definitions + exp...
- **Phase 2: Typed Expansion Validation:** - macro expansion outputs must type-check in destination context - diagnostics point to both call site and expanded fragment
- **Phase 3: Limited `comptime` Evaluation:** - allow evaluation of pure functions during compilation - forbid IO/time/control effects at compile time
- **Phase 4: Attribute/derive-style Macros:** - structured code generation for declarations - still deterministic and hygiene-preserving

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
