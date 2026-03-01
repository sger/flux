- Feature Name: 043_pure_flux_checklist
- Start Date: 2026-02-26
- Proposal PR: 
- Flux Issue: 

# Proposal 0043: 043_pure_flux_checklist

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for 043_pure_flux_checklist in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### F. Public API Boundary Semantics (follow-up after strict baseline)

- [x] F1. Replace naming-convention heuristics with explicit visibility (`public fn`) or equivalent.
- [x] F2. Strict checks target real exported surface only.

Pass criteria:
- `public fn` boundary fixtures exist for pass/fail cases.
- Non-exported helper functions are not over-constrained by strict API rules.

Status:
- Completed with explicit `public fn` visibility as the strict API boundary, underscore naming treated as style-only, and module-scoped public/private fixture coverage.
- 0.0.4 module ADT boundary is factory-only: cross-module code uses `public fn` factories/accessors, not direct module-qualified constructor calls.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **__preamble__:** # 043 — Pure Flux Checklist - **1. Goal:** Define a practical checklist to make Flux **pure-by-default** while preserving explicit, typed effects at boundari...
- **1. Goal:** Define a practical checklist to make Flux **pure-by-default** while preserving explicit, typed effects at boundaries.
- **2. Definition of "Pure" (for Flux):** A Flux program is considered "pure by default" when: - A function without effect requirements cannot perform side effects. - Side effects are only allowed through explicit effec...
- **3. Priority Order:** 1. Effect soundness and static enforcement (Phase 4 hardening from 032) 2. Effect polymorphism completion (`with e`) and row constraints (042) 3. Strict boundary discipline for...
- **A. Effect Soundness (032 / Phase 4):** - [x] A1. All direct effectful builtins/primops are statically effect-checked in function bodies. - [x] A2. Effect propagation through call chains is complete (typed, inferred,...
- **B. Handle/Perform Static Correctness (032 / Phase 4):** - [x] B1. `perform Effect.op(...)` validates declared effect and operation at compile time. - [x] B2. `handle Effect { ... }` validates unknown/missing operations statically. -...

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

### D. Entry-Point Policy and Purity Boundary (032 + team decision)

- [x] D1. Enforce chosen `main` policy exactly (hybrid policy currently selected by team).
- [x] D2. Validate `main` signature/effect-root behavior consistently.
- [x] D3. Keep diagnostic messages and hints stable across VM/JIT.

Pass criteria:
- Programs violating chosen policy always fail with deterministic diagnostics.
- Programs following policy succeed without requiring runtime effect fallback.

Status:
- Completed with dedicated entry-point fixtures for duplicate/invalid `main` signatures, top-level effect rejection with and without `main`, strict missing-main enforcement, and root-discharge gating to avoid redundant `E406` cascades when `main` signature is invalid.

D verification matrix:

| Context | Expected |
|---|---|
| Duplicate top-level `fn main` | Reject (`E410`) |
| `fn main` with parameters | Reject (`E411`) |
| `fn main` with non-`Unit` return | Reject (`E412`) |
| Effectful top-level, no `main` | Reject (`E413`, `E414`) |
| Effectful top-level, valid `main` present | Reject (`E413` only) |
| Custom effect escapes valid `main` boundary | Reject (`E406`) |
| Strict mode without `main` | Reject (`E415`) |

### 5.4 CI/Release Gate Rationale

Mandatory sign-off gates:
- CI includes: `cargo test --all --all-features purity_vm_jit_parity_snapshots`
- Release gate includes the same parity step before artifacts

Rationale:
- this parity suite is the regression guard for pure/effect diagnostics across VM and JIT.

D verification matrix:

### 5.4 CI/Release Gate Rationale

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

### 5.1 Milestone Scope Freeze

Milestone scope is frozen to:
- `docs/proposals/0043_pure_flux_checklist.md` sections A/B/C/D/E/G
- parity suite and snapshots under `tests/snapshots/purity_parity`
- CI/release parity gates in `.github/workflows/ci.yml` and `.github/workflows/release.yml`

Baseline tag convention:
- `milestone/pure-baseline-YYYYMMDD`
- include the commit hash in milestone sign-off notes.

### 5.5 Milestone Closure Note

Milestone reached date:
- February 26, 2026

Criteria satisfied by:
- completed sections A/B/C/D/E/G in this checklist
- green parity suite snapshots under `tests/snapshots/purity_parity`
- CI/release parity steps enabled

Verification reference:
- use the command pack in section 5.2

Out-of-scope reference:
- see section 6.

### 5.1 Milestone Scope Freeze

### 5.5 Milestone Closure Note

Milestone reached date:
- February 26, 2026

Out-of-scope reference:
- see section 6.
