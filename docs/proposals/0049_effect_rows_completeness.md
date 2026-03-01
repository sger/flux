- Feature Name: Effect Rows Completeness and Principal Solving
- Start Date: 2026-02-26
- Proposal PR: pending (feature/type-system merge PR)
- Flux Issue: pending (type-system merge-readiness tracker, March 1, 2026)

# Proposal 0049: Effect Rows Completeness and Principal Solving

## Summary
[summary]: #summary

Complete Flux effect-row solving so outcomes are principal, deterministic, and aligned between HM inference and compiler validation.

## Motivation
[motivation]: #motivation

`0042` established practical row constraints; this proposal hardens completeness and ownership boundaries for merge-quality semantics.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Define principal row-solving behavior for supported row forms.
2. Specify deterministic normalization/equivalence rules.
3. Lock HM/compiler ownership boundaries.
4. Guarantee deterministic diagnostics for row failures.
5. Preserve VM/JIT parity for compile-time row diagnostics.

### 4. Non-Goals

1. Full row-polymorphism redesign.
2. New user-facing effect syntax.
3. Runtime effect representation changes.
4. Capability/security effect extensions.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Consolidated technical points

- Solver completeness matrix should classify covered behavior (`complete | partial | unsupported`).
- HM responsibilities: preserve resolved effect components in inferred function types.
- Compiler responsibilities: solve row constraints at call/boundary validation points.
- Integration rule: HM and compiler must not silently widen obligations.
- Deterministic diagnostics contract: stable code/title/primary-label intent and ordering.

### Detailed specification (migrated legacy content)

This proposal is the row-completeness hardening companion to `0042` and governs principal solving expectations.

### Solver Completeness Matrix (March 1, 2026 core lane)

| Row Form / Behavior | State | Expected Diagnostics | Evidence |
|---|---|---|---|
| Concrete atom equivalence (`with IO, Time` == `with Time, IO`) | complete | no diagnostic | `examples/type_system/162_effect_row_order_equivalence_ok.flx`, `tests/type_inference_tests.rs` |
| Higher-order propagation via row vars (`with e`) | complete | `E400` for missing ambient effects; `E419`/`E420` unresolved-variable paths | `examples/type_system/30_effect_poly_hof_nested_ok.flx`, `examples/type_system/failing/19_effect_polymorphism_missing_effect.flx`, compiler/tests |
| Deterministic first-failure effect selection for multi-missing obligations | complete | `E400` deterministic first missing effect | `examples/type_system/failing/194_effect_row_multi_missing_deterministic_e400.flx`, `tests/compiler_rules_tests.rs` |
| Strict unresolved boundary safeguards | complete (strict path) | `E425` | `examples/type_system/failing/61_strict_generic_unresolved_boundary.flx`, `examples/type_system/failing/192_perform_arg_unresolved_strict_e425.flx` |
| Row subtraction via surface normalization (`with A + B - B`) | complete (supported forms) | `E400` when remaining obligations are missing | `examples/type_system/33_effect_row_subtract_surface_syntax.flx`, `examples/type_system/failing/45_effect_row_subtract_missing_io.flx` |
| General constraint-level subtraction/absence solving | complete | `E421` concrete invalid subtraction, `E419`/`E420` unresolved subtraction variables, `E422` unsatisfied subset | `examples/type_system/failing/195_effect_row_invalid_subtract_e421.flx`, `examples/type_system/failing/196_effect_row_subtract_unresolved_single_e419.flx`, `examples/type_system/failing/197_effect_row_subtract_unresolved_multi_e420.flx`, `examples/type_system/failing/198_effect_row_subset_unsatisfied_e422.flx`, `examples/type_system/failing/199_effect_row_subset_ordered_missing_e422.flx`, `examples/type_system/failing/200_effect_row_absent_ordering_linked_violation_e421.flx` |

### HM / Compiler Ownership Boundary (Locked)

- HM inference is responsible for preserving inferred effect components in function types and callback shapes.
- Compiler validation is responsible for solving call-site/boundary row constraints and producing final effect contract diagnostics.
- Integration rule: call validation must not silently widen obligations; if obligations are missing/unresolved, emit deterministic diagnostics in existing families (`E400`, `E419`-`E422`, `E425`).

### Historical notes

- Legacy content normalized into canonical template structure.

### 14. Core-Lane Evidence (March 1, 2026)

- `cargo test --test type_inference_tests` — green.
- `cargo test --test compiler_rules_tests` — green.
- `cargo test --all --all-features purity_vm_jit_parity_snapshots` — green.
- Focused fixture checks (VM/JIT) lock row-order equivalence, propagation, subtraction, and strict unresolved paths.

### 15. Full Completion Evidence (March 1, 2026)

- Core semantics and diagnostics lock:
  - structural subtraction/absence constraints are preserved and solved in compiler call validation and inference-side row resolution.
  - deterministic diagnostics are locked for `E419`, `E420`, `E421`, `E422`, and existing `E400`/`E425` boundaries.
- Fixture matrix:
  - pass: `162`, `163`, `164`, `165`, `166`
  - fail: `194`, `195`, `196`, `197`, `198`, `199`, `200`
- Multi-argument `Absent` ordering closure:
  - `solve_row_constraints` now evaluates `Absent` constraints after row binding/link stabilization.
  - shared-row-var edge case is locked by `166_effect_row_absent_ordering_linked_ok.flx` and `200_effect_row_absent_ordering_linked_violation_e421.flx`.
- Parity snapshot coverage:
  - `tests/support/purity_parity.rs` curated suite includes `162..166` and `194..200` (category `C`) so `cargo test --all --all-features purity_vm_jit_parity_snapshots` records VM/JIT tuple parity snapshots for these paths.
- Verification commands:
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --test type_inference_tests`
  - `cargo test --test compiler_rules_tests`
  - `cargo test --all --all-features purity_vm_jit_parity_snapshots`

## Drawbacks
[drawbacks]: #drawbacks

- Tightening row solving may surface new compile-time failures in gradual paths.
- Additional targeted fixture/snapshot maintenance is required.

### 13. Risks and Mitigations

1. Risk: false-positive tightening in gradual code.
   - Mitigation: strict-first rollout and exception policy.
2. Risk: diagnostic churn.
   - Mitigation: snapshot gate and deterministic ordering.
3. Risk: HM/compiler disagreement.
   - Mitigation: shared compatibility checks and regression fixtures.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### 5. Semantic Decisions (Locked)

1. Row semantics: set-like concrete atoms plus row variables.
2. Concrete effect ordering is non-semantic (canonicalize before comparison).
3. Duplicate atoms are idempotent.
4. Missing-atom subtraction is a deterministic constraint failure.
5. Unresolved row vars in strict-relevant typed paths fail at compile time.
6. Prefer existing diagnostics families (`E419`-`E422`, `E400`) unless a new class is required.

## Prior art
[prior-art]: #prior-art

- Builds on the same row-polymorphism prior art as 0042.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions remain for this proposal's current merge scope.

## Future possibilities
[future-possibilities]: #future-possibilities

- Any post-MVP row-expansion work must preserve diagnostics stability and fixture coverage.
