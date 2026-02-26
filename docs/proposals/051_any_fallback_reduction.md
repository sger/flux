# Proposal 051: Any Fallback Reduction and Typed-Path Soundness

**Status:** Draft  
**Date:** 2026-02-26  
**Depends on:** `032_type_system_with_effects.md`, `043_pure_flux_checklist.md`, `046_typed_ast_hm_architecture.md`, `docs/internals/type_system_effects.md`

---

## 1. Summary

Reduce accidental unsoundness by replacing silent `Any` degradation with concrete type constraints or explicit unresolved diagnostics in high-value typed paths.

This proposal preserves Flux gradual typing where intentional, but narrows implicit fallback where the compiler already has enough information to be stricter.

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
