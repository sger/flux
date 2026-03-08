- Feature Name: Effect Rows and Constraint Solving for `with e`
- Start Date: 2026-02-25
- Completion Date: 2026-03-03
- Status: Implemented
- Proposal PR: pending (feature/type-system merge PR)
- Flux Issue: pending (type-system merge-readiness tracker, March 1, 2026)

# Proposal 0042: Effect Rows and Constraint Solving for `with e`

## Summary
[summary]: #summary

Define practical row-constraint semantics for `with e` so effect polymorphism is predictable, normalized, and diagnosable.

## Motivation
[motivation]: #motivation

Proposal 0032 introduced effect-aware typing. This proposal formalizes row-style constraints so higher-order effect propagation and handler discharge are checked consistently.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 2. Goals

1. Make `with e` constraints explicit and deterministic.
2. Support row extension/normalization for currently supported forms.
3. Improve diagnostics for missing/extra/incompatible effect obligations.
4. Preserve existing Flux surface syntax.

### 3. Non-Goals (for this proposal)

1. Full research-grade row-polymorphism redesign.
2. Runtime representation changes for effects.
3. Capability/security effect system.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Consolidated technical points

- Surface model stays `with ...`, with row-aware solver behavior internally.
- Solver handles equality/subset/subtraction paths used by typed checks.
- Canonicalization treats concrete effect ordering as non-semantic.
- Diagnostics must stay deterministic (code/title/primary-label intent).

### Detailed specification (migrated legacy content)

This proposal remains the canonical row-constraint policy for current `with e` behavior, together with 0049 completeness hardening.

### Historical notes

- Legacy text normalized to template while preserving semantics.

## Drawbacks
[drawbacks]: #drawbacks

- Tighter effect checking can initially increase compile-time diagnostics for gradual code.
- Row diagnostics need disciplined snapshot governance to avoid churn.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- Keep Flux syntax ergonomic while implementing principled solver behavior.
- Prioritize deterministic diagnostics over aggressively exposing internal row terms.

## Prior art
[prior-art]: #prior-art

- Koka / Eff row-polymorphism traditions informed constraints and discharge ideas.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

### 12. Resolution Log (March 1, 2026)

1. **Expose explicit row-tail syntax in user code**
   - Outcome: **Rejected for v0.0.4**.
   - Decision: keep `with ...` surface syntax only.
2. **Add absence constraints in v1**
   - Outcome: **Deferred (linked follow-up proposal)**.
   - Follow-up: `docs/proposals/0049_effect_rows_completeness.md`.
3. **How much row detail appears by default in diagnostics**
   - Outcome: **Accepted now**.
   - Decision: default diagnostics stay concise; internal row detail appears only when materially actionable.
4. **Require explicit effect annotations for strict public higher-order APIs**
   - Outcome: **Deferred (linked follow-up proposal)**.
   - Follow-up: `docs/proposals/0049_effect_rows_completeness.md` and strict-boundary policy updates.

### Completion Criteria (March 1, 2026 closure gate)

1. `with e` propagation is deterministic for supported higher-order call paths.
2. Effect-row normalization/equivalence treats concrete effect ordering as non-semantic.
3. Supported subtraction forms preserve remaining obligations deterministically.
4. Diagnostics lock surface remains stable as `code/title/primary label`.

### Evidence Matrix (March 1, 2026)

| Criterion | Fixture Evidence | Test Evidence | Parity / Snapshot Evidence |
|---|---|---|---|
| Deterministic `with e` propagation | `examples/type_system/30_effect_poly_hof_nested_ok.flx`, `examples/type_system/failing/19_effect_polymorphism_missing_effect.flx` | `tests/type_inference_tests.rs`, `tests/compiler_rules_tests.rs` | VM/JIT fixture runs for `30` and `19` agree on behavior |
| Row normalization/equivalence | `examples/type_system/33_effect_row_subtract_surface_syntax.flx`, `examples/type_system/100_effect_row_order_equivalence_ok.flx` | `tests/type_inference_tests.rs` (`infer_effect_row_order_equivalence_for_function_params`) | VM/JIT fixture run for `30` and type-level ordering invariants |
| Supported subtraction behavior | `examples/type_system/failing/45_effect_row_subtract_missing_io.flx` | `tests/compiler_rules_tests.rs` (effect-row call contract checks) | VM/JIT fixture runs for `45` both emit `E400` |
| Stable diagnostics contract | `examples/type_system/failing/61_strict_generic_unresolved_boundary.flx` | `tests/compiler_rules_tests.rs` (`strict_unresolved_generic_boundary_has_stable_diagnostic_shape`) | `cargo test --all --all-features purity_vm_jit_parity_snapshots` |

### 13. Closure Evidence (March 1, 2026)

- `cargo test --test type_inference_tests` — green.
- `cargo test --test compiler_rules_tests` — green.
- `cargo test --all --all-features purity_vm_jit_parity_snapshots` — green.
- Focused VM/JIT fixture checks:
  - `30_effect_poly_hof_nested_ok.flx` (success in VM/JIT),
  - `19_effect_polymorphism_missing_effect.flx` (`E400` in VM/JIT),
  - `45_effect_row_subtract_missing_io.flx` (`E400` in VM/JIT),
  - `61_strict_generic_unresolved_boundary.flx` (`E425` in strict VM/JIT).

## Future possibilities
[future-possibilities]: #future-possibilities

- Expand absence constraints and advanced rows only with dedicated proposal + fixture coverage.
