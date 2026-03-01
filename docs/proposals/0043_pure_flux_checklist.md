- Feature Name: 043_pure_flux_checklist
- Start Date: 2026-02-26
- Proposal PR: pending (feature/type-system merge PR)
- Flux Issue: pending (type-system merge-readiness tracker, March 1, 2026)

# Proposal 0043: 043_pure_flux_checklist

## Summary
[summary]: #summary

Checklist proposal for shipping Flux as pure-by-default in typed paths, with deterministic diagnostics and VM/JIT parity.

## Motivation
[motivation]: #motivation

Pure-by-default policy must be testable and stable across compiler validation, runtime parity, and release gates.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### F. Public API Boundary Semantics (follow-up after strict baseline)

- [x] Replace naming heuristics with explicit `public fn` boundary.
- [x] Apply strict checks to exported surface only.

Pass criteria:
- Public boundary pass/fail fixtures exist.
- Private helpers are not over-constrained by strict public API rules.

Status:
- Completed on this branch with module-scoped public/private fixture coverage.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Consolidated technical points

- Pure-by-default checklist items A/B/C/D/E/G are implemented for milestone scope.
- Entry-point policy and strict/public boundary checks have dedicated fixture and parity coverage.
- CI/release parity gate is required for this track.

### Detailed specification (migrated legacy content)

Normative behavior is carried by checklist scope + fixture matrix + parity snapshots.

### Historical notes

- Milestone closure date recorded as February 26, 2026.

## Drawbacks
[drawbacks]: #drawbacks

- Stricter boundaries increase near-term migration work.
- Parity/snapshot governance adds maintenance overhead.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### D. Entry-Point Policy and Purity Boundary (032 + team decision)

- [x] Enforce chosen `main` policy and diagnostics deterministically.
- [x] Keep VM/JIT parity on boundary diagnostics.

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

Mandatory gate:
- `cargo test --all --all-features purity_vm_jit_parity_snapshots`

Rationale:
- This is the lock surface for pure/effect VM/JIT diagnostics parity.

## Prior art
[prior-art]: #prior-art

- No additional prior-art sources beyond the effect/purity proposals already cited.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions for this milestone scope.

## Future possibilities
[future-possibilities]: #future-possibilities

### 5.1 Milestone Scope Freeze

Milestone scope:
- `docs/proposals/0043_pure_flux_checklist.md` sections A/B/C/D/E/G
- parity snapshots under `tests/snapshots/purity_parity`
- parity CI/release gates

### 5.5 Milestone Closure Note

Milestone reached date:
- February 26, 2026
