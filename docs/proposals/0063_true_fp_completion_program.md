- Feature Name: True FP completion program
- Start Date: 2026-02-28
- Status: Partially Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0063: True FP completion program

## Summary
[summary]: #summary

This proposal defines the remaining feature work required for Flux to be considered a true functional programming language in practice. It scopes delivery to semantic and feature completeness, organized into four lanes: principal effect semantics, deterministic typing and totality, immutable typed data modeling, and core FP abstraction ergonomics.

## Motivation
[motivation]: #motivation

Flux has implemented major foundations (pure-by-default typed paths, baseline effects, ADT semantics), but there are still roadmap-level gaps that prevent release-grade FP completeness: [motivation]: #motivation

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

This proposal is a delivery contract for "true FP readiness" in Flux.

What it means in practice:

1. Effects are principal and deterministic in supported `with` forms.
2. Typed paths are strict-first and avoid disallowed `Any` fallback.
3. Immutable typed records are available for structured product modeling.
4. Core FP ergonomics (currying/partials + baseline traits) are available without dynamic fallbacks.

The work is grouped into four delivery lanes:

1. Lane A: Principal Effect System Completion (`049` over `042` baseline)
2. Lane B: Typed Determinism and Totality (`050`, `051`, with `047` dependencies)
3. Lane C: Immutable Typed Data Modeling (`048`)
4. Lane D: Core FP Abstractions (`052`, `053`)

Non-goals:

1. no runtime representation redesign unless required by scoped tracks,
2. no macro system/package manager/concurrency expansion,
3. no parser-DX/performance-only work,
4. no LSP/editor integration work.

Milestone model:

1. `M0`: readiness freeze (normalize contracts + publish unified readiness matrix),
2. `M1`: semantic core closure (Lane A/B complete),
3. `M2`: data + abstraction closure (Lane C/D complete),
4. `M3`: sign-off (all exit conditions met; final evidence published).

What it means in practice:

The work is grouped into four delivery lanes:

Non-goals:

Milestone model:

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Current grounded state:** 1. pure-by-default baseline implemented and parity-guarded (`043`), 2. effects and row constraints partially solved (`032`, `042`), with principal...
- **Current grounded state:** 1. pure-by-default baseline implemented and parity-guarded (`043`), 2. effects and row constraints partially solved (`032`, `042`), with principal completeness still tracked (`0...
- **Lane outcomes and exit conditions:** 1. principal row solving for supported `with` forms, 2. deterministic normalization/equivalence for extension and subtraction, 3. deterministic diagnostics for unresolved/ambigu...
- **Validation pack:** Blocking: 1. `cargo check --all --all-features` 2. `cargo test --test type_inference_tests` 3. `cargo test --test compiler_rules_tests` 4. `cargo test --test pattern_validation`...
- **Evidence contract:** Each 063 task PR must include: 1. lane + milestone tag, 2. commands run with PASS/FAIL classification, 3. fixture/test mapping to acceptance requirements, 4. diagnostic/snapshot...
- **Initial backlog:** 1. `T0`: unified FP readiness matrix (`must-have for 063` vs deferred), 2. `T1`: effect rows principal completion (`049` strict-first closure), 3. `T2`: `Any` fallback and exhau...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

1. Broad scope across multiple lanes increases coordination overhead.
2. Cross-track sequencing can temporarily slow velocity in individual feature branches.
3. Tight acceptance gating may defer low-priority enhancements that are useful but not must-have for readiness.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

Why this design:

1. It creates one auditable execution frame for completion-critical tracks.
2. It ties implementation work to deterministic acceptance gates and evidence.
3. It reduces scope creep by making non-goals explicit and milestone-gated.

Alternatives considered:

1. Independent delivery of `048/049/050/051/052/053` without an umbrella proposal.
   Rationale for rejection: higher risk of inconsistent gating, drift in acceptance criteria, and unclear sign-off semantics.
2. A larger all-in roadmap proposal covering tooling/perf/editor concerns as well.
   Rationale for rejection: dilutes semantic closure focus and makes completion criteria less testable.

Impact of not doing this:

1. readiness status remains ambiguous,
2. deterministic semantics closure is harder to audit,
3. cross-track regressions are more likely to slip past inconsistent validation.

Why this design:

Alternatives considered:

Impact of not doing this:

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

1. Language roadmaps commonly use umbrella milestones plus owner-track specs for closure-critical semantics.
2. Flux-specific precedent already exists in linked proposals (`032`, `042`, `043`, `047`, `048`, `049`, `050`, `051`, `052`, `053`) and this proposal formalizes delivery integration.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Which edge scenarios in row subtraction/normalization require additional fixture classes beyond current `049` coverage?
2. Are additional parity suites needed for traits/typeclass lowering in VM/JIT beyond existing diagnostic and purity parity tracks?
3. What is the minimal accepted MVP boundary for typed records in `048` for 063 sign-off?

## Future possibilities
[future-possibilities]: #future-possibilities

1. Expand traits/typeclasses beyond baseline (`Eq`, `Ord`, `Show`) after 063 closure.
2. Deepen exhaustive analysis beyond conservative mode where theorem-proving cost is justified.
3. Evaluate post-063 enhancements for toolchain, package ecosystem, and editor integration under separate proposals.
