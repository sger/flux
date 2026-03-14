- Feature Name: 0.0.4 HM/ADT/Exhaustiveness Critical Path + Post-0.0.4 Hardening Roadmap
- Start Date: 2026-02-26
- Completion Date: 2026-03-01
- Status: Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0054: 0.0.4 HM/ADT/Exhaustiveness Critical Path + Post-0.0.4 Hardening Roadmap

## Summary
[summary]: #summary

This proposal is the execution orchestrator for the full hardening roadmap: This proposal is the execution orchestrator for the full hardening roadmap:

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Week 2: ADT Semantics Hardening (owner: 047 track)

Objective:
- deterministic constructor typing and module policy diagnostics.

File focus:
- compiler constructor typing and match-check paths
- module constructor boundary diagnostics

Gates:
- constructor arity/type determinism
- generic constructor field mismatch
- module constructor misuse diagnostics
- nested ADT pass/fail consistency

### Week 2: ADT Semantics Hardening (owner: 047 track)

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **In scope:** 1. Compiler semantic hardening and diagnostics stability. 2. Fixture/snapshot parity governance. 3. Feature rollout sequencing for `052`, `053`, `048`. 4. Intern...
- **In scope:** 1. Compiler semantic hardening and diagnostics stability. 2. Fixture/snapshot parity governance. 3. Feature rollout sequencing for `052`, `053`, `048`. 4. Internal architecture/...
- **Out of scope:** 1. Concurrency (`026`) as release blocker. 2. GC (`045`) as release blocker. 3. Higher-rank polymorphism and theorem-proving style exhaustiveness.
- **3. Locked Global Diagnostics Contract:** Preserve existing classes where semantics match: 1. `E300` HM/type mismatch 2. `E400` effect boundary family 3. `E055` runtime boundary mismatch 4. `E015` general non-exhaustive...
- **Week 1: HM Zero-Fallback Completion (owner: 051 track):** Objective: - typed/inferred validation does not depend on runtime-compat rescue.
- **Week 3: Strong Exhaustiveness Completion (owner: 050 track):** Objective: - deterministic compile-time totality over supported domains.

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 8. Risks and Mitigations

1. Risk: diagnostics churn across tracks.  
   Mitigation: lock class boundaries and enforce snapshot review.

2. Risk: scope creep in post-0.0.4 features.  
   Mitigation: maintain stage boundaries and non-goals explicitly.

3. Risk: parity regressions while adding features.  
   Mitigation: parity fixture expansion per track before merge.

### 8. Risks and Mitigations

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
