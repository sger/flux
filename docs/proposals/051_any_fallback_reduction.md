# Proposal 051: Any Fallback Reduction and Typed-Path Soundness

**Status:** Implemented  
**Date:** 2026-02-26  
**Depends on:** `032_type_system_with_effects.md`, `043_pure_flux_checklist.md`, `046_typed_ast_hm_architecture.md`, `docs/internals/type_system_effects.md`

---

## 1. Summary

Reduce accidental unsoundness by replacing silent `Any` degradation with concrete type constraints or explicit unresolved diagnostics in high-value typed paths.

This proposal preserves Flux gradual typing where intentional, but narrows implicit fallback where the compiler already has enough information to be stricter.

Execution ownership note:
- This proposal is the HM zero-fallback owner for stage-1 week-1 in
  `054_0_0_4_hm_adt_exhaustiveness_critical_path.md`.
- It does not own ADT semantic hardening (`047`) or exhaustiveness ownership (`050`).

---

## 2. Problem Statement

`Any` is a deliberate gradual escape hatch, but current behavior still contains fallback sites that are effectively accidental:

1. HM expression paths that collapse disagreements to `Any` despite known constraints.
2. Member/index/projection paths where known typed context still degrades.
3. Pattern-binding propagation gaps that widen to `Any` prematurely.
4. Strict-boundary unresolved pockets that can be reduced by better typed propagation.

Impact:
- unsound behavior can pass compile-time checks unexpectedly,
- strict diagnostics (`E425` class) can appear where concrete typing should have resolved.

---

## 3. Goals

1. Inventory and classify all `Any` fallback hotspots.
2. Define clear allow/disallow policy for fallback.
3. Tighten typed/HM-known contexts first.
4. Keep intentional gradual behavior explicit and documented.
5. Improve deterministic diagnostics where fallback is blocked.

---

## 4. Non-Goals

1. Eliminate `Any` from Flux entirely.
2. Force fully static typing for all programs.
3. Introduce new syntax for gradual boundaries.
4. Redesign runtime boundary checking model.

---

## 5. Fallback Policy (Locked)

### 5.1 Allowed fallback

Fallback to `Any` remains allowed when:

1. source value is explicitly dynamic/unknown,
2. type information is truly unavailable after HM + contract resolution,
3. context is explicitly gradual and not strict-boundary-critical.

### 5.2 Disallowed fallback

Fallback is disallowed when:

1. HM has concrete type evidence at the expression site,
2. function/module contracts provide concrete expectations,
3. strict boundary validation requires determinism,
4. projection/member/index typing can be resolved from known shape.

### 5.3 When disallowed fallback is hit

1. emit concrete mismatch diagnostics (`E300`) when types conflict,
2. emit unresolved-boundary diagnostics (`E425`) only when type is genuinely unresolved after evidence paths are exhausted.

---

## 6. Hotspot Inventory Matrix (Required)

Each hotspot row must include: current behavior, target behavior, severity, and rollout stage.

Minimum hotspot categories:

1. HM branch joins (`if`/`match`) where known typed context exists.
2. Member access on known module/member contracts vs unknown objects.
3. Index/tuple-field projection known-shape typing.
4. Pattern-binding propagation into arm/body typing.
5. Infix/prefix typed arithmetic/logical constraints.
6. Strict unresolved boundary checks on call/return/typed let paths.

---

## 7. Tightening Strategy

### Stage 1 (strict + typed known contexts)

1. Remove disallowed fallback in strict and typed-contract-sensitive paths.
2. Keep explicit diagnostics stable.

### Stage 2 (default typed paths)

1. Expand disallowed fallback policy to non-strict typed/HM-known contexts.
2. Preserve gradual behavior only in documented allowed classes.

### Stage 3 (cleanup)

1. Remove transitional fallback branches.
2. Freeze hotspot matrix and diagnostics behavior.

---

## 8. Diagnostics Contract

1. Prefer existing mismatch class `E300` when concrete conflict exists.
2. Use `E425` only for genuine unresolved strict-boundary cases.
3. Avoid introducing new codes unless a distinct fallback-blocked class appears.
4. Preserve deterministic ordering and primary-label stability.

---

## 9. Test Plan

### Unit/HM tests

1. Known-shape expressions must not collapse to `Any`.
2. Pattern-binding constrained cases propagate concrete types.
3. Branch join behavior in typed contexts yields deterministic mismatch, not silent widen.

### Compiler integration tests

1. Typed member/index/projection checks without permissive fallback.
2. Strict unresolved boundary tests that verify reduced false `E425` incidence.
3. Contract-driven typed-call checks with deterministic `E300` vs `E425` split.

### Fixtures/parity

1. Add fixtures for each hotspot class (pass + fail).
2. Extend parity suite with at least one `Any`-reduction scenario.
3. Snapshot diagnostics for tightened paths.

---

## 10. Coordination with Proposals 049 and 050

1. Consume effect-row completion results from `049` before removing related fallback in effect-typed paths.
2. Consume exhaustiveness/totality guarantees from `050` before tightening match-join fallback behavior.
3. Keep one shared staged rollout model across all three proposals.

---

## 11. Release Gate

A release candidate passes only if:

1. hotspot matrix is complete and each row has target state + test coverage,
2. disallowed fallback is removed for strict and typed-known contexts,
3. diagnostics remain deterministic in snapshots,
4. parity suite includes `Any`-reduction category and is green.

---

## 12. Risks and Mitigations

1. Risk: breaking existing gradual code unexpectedly.
   - Mitigation: staged rollout and explicit allowed-fallback table.
2. Risk: increased diagnostic volume.
   - Mitigation: keep mismatch/unresolved split clear and deterministic.
3. Risk: implementation drift across HM and compiler validators.
   - Mitigation: hotspot matrix ownership and shared regression tests.

---

## 13. Assumptions and Defaults

1. `Any` remains part of Flux as intentional gradual mechanism.
2. This proposal targets accidental fallback reduction, not gradual-typing removal.
3. Rollout is strict-first, then expanded to default typed contexts.
4. Canonical semantics source remains `docs/internals/type_system_effects.md`.

---

## 14. Strict-First Hotspot Matrix (S0)

| Hotspot | Current Behavior | Policy Class | Strict-First Target | Status |
|---|---|---|---|---|
| Known tuple projection (`136`) | precise projected type (`t_i`) | Disallowed fallback | keep concrete projected type, avoid unresolved drift | Implemented |
| Match scrutinee family propagation (`138/139`) | family-consistent arms constrain scrutinee | Disallowed fallback | keep propagation; mixed families stay gradual | Implemented |
| Unannotated self recursion (`140/141`) | bounded second pass refines return type | Disallowed fallback | keep concrete recursive return where derivable | Implemented |
| Concrete heterogeneous array literal (`151/152`) | degraded to `Array[Any]` without root-cause mismatch | Disallowed fallback | emit concrete `E300` at literal site; avoid follow-on strict `E425` | Implemented (this slice) |
| Strict unresolved tuple projection (`154`) | unresolved strict boundary | Allowed fallback | keep `E425` when source is genuinely unresolved | Locked |
| Strict unresolved member access (`155`) | unresolved strict boundary | Allowed fallback | keep `E425` when source is genuinely unresolved | Locked |
| Strict unresolved call arg (`156`) | unresolved strict boundary | Allowed fallback | keep `E425` when source is genuinely unresolved | Locked |
| Tuple destructure concrete mismatch (`161`) | destructure shape mismatch can flow through as unresolved boundary noise | Disallowed fallback | emit concrete `E300` for concrete destructure mismatch | Implemented (this slice) |
| Tuple destructure unresolved strict (`162`) | unresolved tuple-destructure source at strict boundary | Allowed fallback | keep `E425` only when genuinely unresolved | Locked |
| Match concrete disagreement strict (`163`) | concrete arm disagreement can still co-emit unresolved strict noise | Disallowed fallback | keep concrete `E300` as primary signal in strict path | Implemented (this slice) |
| Match unresolved arm suppression (`164`) | unresolved/Any arm paths should not emit contextual mismatch noise | Allowed fallback | preserve suppression for unresolved gradual arms | Locked |
| Self-recursive concrete precision (`165`) | recursive refinement can regress to Any in downstream typed use | Disallowed fallback | keep concrete recursive precision and surface `E300` at typed boundary | Implemented (this slice) |
| Self-recursive unresolved guard (`166`) | unresolved-heavy self recursion may trigger false-positive recursion noise | Allowed fallback | keep unresolved-heavy paths stable without false-positive recursion diagnostics | Locked |
| Tuple destructure ordering-sensitive concrete mismatch (`167`) | concrete tuple shape/arity conflict can degrade to unresolved noise | Disallowed fallback | deterministic `E300` for concrete tuple destructure conflicts | Implemented (remaining slice) |
| Tuple destructure unresolved guard (`168`) | unresolved destructure source should remain unresolved class in strict mode | Allowed fallback | keep strict `E425` only for genuinely unresolved destructure source | Locked |
| Match disagreement with unresolved first arm (`169`) | concrete disagreement can be skipped when first arm is unresolved | Disallowed fallback | contextual `E300` still emitted via concrete pivot arm | Implemented (remaining slice) |
| Match all-concrete ordering invariance (`170`) | concrete disagreement diagnostics can depend on arm ordering | Disallowed fallback | `E300` emission invariant to arm ordering for concrete conflicts | Implemented (remaining slice) |
| Self-recursive concrete-chain refinement (`171`) | self-recursive chain may still drift toward Any in typed mismatch paths | Disallowed fallback | keep concrete refined return and surface typed mismatch as `E300` | Implemented (remaining slice) |
| Self-recursive unresolved guard (`172`) | unresolved-heavy recursion may gain false-positive recursion mismatch noise | Allowed fallback | keep unresolved baseline diagnostics without added recursion mismatch noise | Locked |

## 15. Strict-First Implementation Notes (S1/S2)

1. HM array-literal inference now emits concrete mismatch diagnostics for concrete heterogeneous elements, instead of only degrading silently to `Any`.
2. Strict unresolved-boundary reporting now suppresses follow-on `E425` when a concrete overlapping `E300` root-cause already exists for the same expression.
3. Gradual unresolved paths remain unchanged: unresolved strict boundaries still report `E425` when no concrete mismatch evidence exists.

## 16. Regression Fixtures and Tests (S3)

Added fixtures:
- `151_array_literal_concrete_conflict_prefers_e300.flx`
- `152_array_literal_callarg_conflict_prefers_e300.flx`
- `153_match_branch_conflict_prefers_e300.flx`
- `154_unresolved_projection_strict_e425.flx`
- `155_unresolved_member_access_strict_e425.flx`
- `156_unresolved_call_arg_strict_e425.flx`
- `161_tuple_destructure_concrete_mismatch_prefers_e300.flx`
- `162_tuple_destructure_unresolved_strict_e425.flx`
- `163_match_concrete_disagreement_prefers_e300.flx`
- `164_match_unresolved_arm_stays_suppressed.flx`
- `165_self_recursive_precision_prefers_e300.flx`
- `166_self_recursive_guard_stable_unresolved.flx`
- `167_tuple_destructure_ordered_concrete_conflict_e300.flx`
- `168_tuple_destructure_unresolved_guard_strict_e425.flx`
- `169_match_disagreement_first_arm_unresolved_still_e300.flx`
- `170_match_disagreement_all_concrete_ordering_invariant_e300.flx`
- `171_self_recursive_refinement_concrete_chain_e300.flx`
- `172_self_recursive_unresolved_guard_no_false_positive.flx`

Coverage updates:
- `tests/type_inference_tests.rs`
  - `infer_array_literal_with_concrete_heterogeneous_elements_emits_e300`
- `tests/compiler_rules_tests.rs`
  - fixture locks for `151..156`, `161..166`, and `167..172` (`E300` vs `E425` split + suppression guards)

## 17. Evidence (S4)

Validation commands (strict-first slice):
- `cargo check --all --all-features`
- `cargo test --test type_inference_tests`
- `cargo test --test compiler_rules_tests`
- `cargo test --test snapshot_diagnostics`
- `cargo test --test examples_fixtures_snapshots`
- `cargo test --all --all-features purity_vm_jit_parity_snapshots`

Gate interpretation:
1. Blocking: `check`, `type_inference_tests`, `compiler_rules_tests`, `snapshot_diagnostics`, `purity_vm_jit_parity_snapshots`
2. Informational-only when unrelated external churn is known and explicitly attributed: `examples_fixtures_snapshots`

Command outcomes (this execution slice):
- `cargo check --all --all-features` -> PASS
- `cargo test --test type_inference_tests` -> PASS
- `cargo test --test compiler_rules_tests` -> PASS
- `cargo test --test snapshot_diagnostics` -> PASS
- `cargo test --all --all-features purity_vm_jit_parity_snapshots` -> PASS
- `cargo test --test examples_fixtures_snapshots` -> FAIL (informational, unrelated external churn)

Known external churn:
- `tests/snapshots/examples_fixtures/aoc__2024__day03.snap.new`
  - Current failure is from `examples/aoc/2024/day03.flx` snapshot drift (top-level effect/private-member output change), not from 051 strict-first Any-fallback paths (`151..172`).

Strict-only fixture expectation policy:
1. Fixtures `154`, `155`, `156`, `162`, and `168` are strict-only expectation fixtures.
2. Canonical correctness checks run with `--strict` and are enforced in `compiler_rules_tests`.
3. Non-strict runs (including generic `examples_fixtures_snapshots` transcripts) may show unresolved baseline `E004` first; this is expected and not a 051 regression.

#### 051 Closure Note (Regression Lock)
Implemented and locked behavior:
1. Tuple destructure strict-first hardening:
   - concrete destructure mismatch emits `E300` (`161`, `167`)
   - genuinely unresolved strict destructure source emits `E425` (`162`, `168`)
2. Match disagreement surfacing:
   - concrete arm disagreement remains surfaced as contextual `E300`
   - ordering-sensitive gap (unresolved first arm) is covered by concrete-pivot conflict lock (`169`, `170`)
3. Self-recursive precision:
   - refined self-recursive concrete chains remain precise at typed boundary (`165`, `171`)
   - unresolved-heavy self-recursive guard cases remain free of recursion-specific false-positive mismatch noise (`166`, `172`)

Regression lock mapping:
1. HM/unit-level behavior checks in `tests/type_inference_tests.rs`
2. Compile-pipeline fixture locks in `tests/compiler_rules_tests.rs`
3. Fixture catalog lock in `examples/type_system/failing/README.md` for `151..172`

Non-goals that remain explicit:
1. Mutual recursion/SCC fixpoint inference is deferred.
2. Gradual unresolved behavior remains intentionally allowed outside concrete-evidence strict paths.

Focused proof commands:
1. `cargo run -- --no-cache examples/type_system/failing/167_tuple_destructure_ordered_concrete_conflict_e300.flx --strict`
2. `cargo run -- --no-cache examples/type_system/failing/168_tuple_destructure_unresolved_guard_strict_e425.flx --strict`
3. `cargo run -- --no-cache examples/type_system/failing/169_match_disagreement_first_arm_unresolved_still_e300.flx --strict`
4. `cargo run -- --no-cache examples/type_system/failing/171_self_recursive_refinement_concrete_chain_e300.flx`
5. `cargo run -- --no-cache examples/type_system/failing/172_self_recursive_unresolved_guard_no_false_positive.flx`
