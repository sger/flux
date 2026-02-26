# Proposal 049: Effect Rows Completeness and Principal Solving

**Status:** Draft  
**Date:** 2026-02-26  
**Depends on:** `032_type_system_with_effects.md`, `042_effect_rows_and_constraints.md`, `043_pure_flux_checklist.md`, `docs/internals/type_system_effects.md`

---

## 1. Summary

Complete Flux effect-row semantics so row-variable solving is principal, deterministic, and consistent across HM-aware typing and compiler validation paths.

This proposal hardens existing row behavior rather than introducing new syntax.

---

## 2. Problem Statement (Current Partial State)

`042` landed practical row constraints and useful diagnostics, but row reasoning is still partial in several high-value cases:

1. Row-variable propagation is strongest at call-site compiler validation, not fully principal across all HM-influenced expression flows.
2. Normalization behavior for mixed row operations (`+` and `-`) is under-specified for edge combinations.
3. Equality/subset obligations can become ambiguous in nested higher-order chains with imported/module-qualified contracts.
4. Diagnostics are improved but still not fully normalized to one deterministic shape for all unresolved/ambiguous classes.

Impact:
- typed programs can still encounter unresolved/ambiguous pockets that degrade confidence,
- strict-mode behavior may differ by expression shape even when semantic obligations are equivalent.

---

## 3. Goals

1. Define principal row-solving behavior for currently supported row forms.
2. Specify deterministic row normalization and equivalence.
3. Make HM/compiler ownership boundaries explicit and stable.
4. Guarantee deterministic diagnostics for unresolved/ambiguous row outcomes.
5. Preserve VM/JIT parity for compile-time row failures.

---

## 4. Non-Goals

1. Full research-grade row-polymorphism redesign.
2. New user-facing effect syntax.
3. Runtime effect representation changes.
4. Capability/security effect system extensions.

---

## 5. Semantic Decisions (Locked)

1. Row semantics use set-like concrete atoms plus row variables.
2. Concrete-effect ordering is non-semantic; canonicalization is required before comparison.
3. Duplicate atoms are idempotent.
4. Subtraction removes concrete atoms only when present; missing-atom subtraction is a deterministic constraint violation.
5. Unresolved row variables in strict-relevant typed contexts are compile-time failures; in non-strict gradual contexts they may remain recoverable where explicitly allowed by policy.
6. Existing diagnostics classes are preferred (`E419`-`E422`, `E400`), with new codes only for materially distinct failure classes.

---

## 6. Solver Completeness Matrix

Each row below must be classified and covered by tests as `complete | partial | unsupported`.

1. `with e` direct callback forwarding.
2. `with IO, e` row extension through one and multiple wrapper layers.
3. `with e + IO - Console` normalization and solve stability.
4. Module-qualified polymorphic callback contracts crossing import aliases.
5. Equality constraints between inferred callback row and declared parameter row.
6. Subset obligations at ambient-effect boundary checks.
7. Nested higher-order chains with mixed typed/untyped wrappers.

Acceptance requirement: no `partial` entries remain in supported surface forms after implementation.

---

## 7. HM vs Compiler Ownership Boundary

### 7.1 HM responsibilities

1. Carry function effect sets consistently through inferred function types.
2. Preserve resolved effect components on inferred function types.
3. Avoid introducing ad hoc row fallback behavior that bypasses compiler row solver rules.

### 7.2 Compiler responsibilities

1. Collect and solve row constraints from function contracts and call arguments.
2. Apply subset/equality/subtraction rules at call and ambient-boundary validation points.
3. Emit deterministic diagnostics for unsatisfied or ambiguous row constraints.

### 7.3 Integration rule

HM-derived function types and compiler row constraints must agree on function-effect compatibility; neither side may silently widen obligations.

---

## 8. Deterministic Diagnostics Contract

For row-related failures, diagnostics must be deterministic in:

1. code,
2. title,
3. primary label intent text,
4. ordering (first material violation wins).

Primary classes:
1. unresolved single variable (`E419`),
2. ambiguous multiple variables (`E420`),
3. invalid subtraction (`E421`),
4. unsatisfied subset (`E422`),
5. missing ambient concrete effect (`E400`).

---

## 9. Staged Rollout Policy

### Stage 1 (strict-first)

1. Enable full deterministic row failure behavior in strict and typed-boundary-sensitive paths.
2. Keep non-strict gradual recovery only where currently intentional.

### Stage 2 (expanded default)

1. Promote remaining row-hardening checks to default typed paths once fixture/parity confidence is green.
2. Keep explicit documentation for any retained gradual exceptions.

### Stage 3 (cleanup)

1. Remove temporary compatibility branches.
2. Freeze diagnostic contracts in snapshots.

---

## 10. VM/JIT Compatibility Matrix

For each curated row fixture class, VM and JIT must match on:

1. compile success/failure,
2. diagnostic code,
3. title,
4. primary label.

Required matrix categories:
1. nested HOF row propagation success,
2. nested HOF missing-effect failure,
3. subtraction conflict failure,
4. ambiguous/unresolved row variable failure,
5. module-qualified polymorphic row behavior.

---

## 11. Test and Fixture Plan

### Unit/solver-level

1. Row normalization idempotency and order-insensitivity.
2. Add/subtract/equality/subset constraint resolution determinism.
3. Ambiguous and unresolved variable classification.

### Compiler integration

1. Nested polymorphic wrappers with module-qualified callbacks.
2. Typed context failures with deterministic row diagnostics.
3. Strict-mode unresolved row paths.

### Fixtures/parity

1. Expand `examples/type_system` row fixtures for edge combinations.
2. Add failing fixtures for ambiguity/subtraction conflicts.
3. Extend purity parity matrix with at least one new row-hardening case.

---

## 12. Release Gate

A release candidate passes only if:

1. all supported row forms are marked `complete` in the completeness matrix,
2. row diagnostics are deterministic in snapshots,
3. VM/JIT parity suite is green for row categories,
4. no documented strict-path unresolved row escapes remain.

---

## 13. Risks and Mitigations

1. Risk: false-positive tightening in gradual code.
   - Mitigation: strict-first rollout and explicit exception table.
2. Risk: diagnostic churn.
   - Mitigation: snapshot gate and deterministic ordering rule.
3. Risk: HM/compiler disagreement on function effects.
   - Mitigation: shared compatibility checks and targeted regression tests.

---

## 14. Assumptions and Defaults

1. `042` is baseline but incomplete.
2. Rollout is staged with strict-first enforcement.
3. No syntax changes are required in this proposal.
4. Canonical semantics remain anchored to `docs/internals/type_system_effects.md`.
