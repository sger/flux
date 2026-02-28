# Proposal 047: ADT Semantics Deepening (Post-Sugar Hardening)

**Status:** Implemented  
**Date:** 2026-02-26  
**Depends on:** `032_type_system_with_effects.md`, `043_pure_flux_checklist.md`, `046_typed_ast_hm_architecture.md`

---

## 1. Summary

After landing `type ... = ... | ...` ADT declaration sugar, deepen ADT semantics without introducing new syntax.

This proposal focuses on stronger type/diagnostic guarantees for ADT constructors, generic instantiation, module boundaries, and nested exhaustiveness precision.

Execution ownership note:
- This proposal is the ADT-semantic owner for stage-1 week-2 in
  `054_0_0_4_hm_adt_exhaustiveness_critical_path.md`.
- It does not own HM zero-fallback sequencing (`051`) or global rollout orchestration (`054`).

---

## 2. Motivation

Current ADT support is functional, but semantic precision remains uneven across:
- constructor field typing under generics,
- module-qualified constructor typing and visibility boundaries,
- HM integration for nested constructor expressions,
- deeper constructor-space exhaustiveness diagnostics.

The next step is semantic hardening, not syntax expansion.

---

## 3. Goals

1. Enforce constructor field type constraints consistently across generic instantiations.
2. Stabilize module-qualified constructor typing and boundary policy diagnostics.
3. Improve HM inference around nested ADT constructor calls/patterns.
4. Strengthen nested exhaustiveness behavior and diagnostics determinism.
5. Preserve VM/JIT diagnostic parity for ADT failure cases.

---

## 4. Non-Goals

1. New ADT declaration syntax.
2. Trait/typeclass system.
3. Runtime ADT representation redesign.
4. Pattern theorem-proving beyond constructor-space + guard policy.

---

## 5. Scope Areas

### 5.1 Constructor field type enforcement
- Ensure constructor argument checks use instantiated generic parameter mapping.
- Eliminate permissive `Any` paths where concrete constructor types are known.

### 5.2 Module-qualified constructor policy
- Keep 0.0.4 factory-only public boundary policy explicit.
- Ensure diagnostics for direct constructor boundary misuse are stable and actionable.

### 5.3 HM integration
- Tighten HM typing for nested constructor expressions and matches.
- Ensure constructor-return and pattern-binding types are propagated consistently.

### 5.4 Exhaustiveness precision
- Extend nested constructor-space coverage checks for generic ADTs.
- Keep guard policy deterministic (guarded arms contribute conditional coverage only).

---

## 6. Implementation Plan

1. Audit ADT constructor typing callsites in compiler expression/match paths.
2. Introduce/normalize generic-instantiation mapping for constructor checks.
3. Harden module constructor boundary diagnostics and cascade ordering.
4. Improve nested ADT HM propagation in expression typing paths.
5. Expand exhaustiveness checks + fixtures for nested/generic patterns.
6. Update parity matrix snapshots only for intentional diagnostic changes.

---

## 7. Fixtures and Tests

### Passing additions
- Generic constructor chains with nested matches.
- Module factory/accessor flows using ADT generics.

### Failing additions
- Constructor arg type mismatch under instantiated generics.
- Module boundary constructor misuse diagnostics.
- Nested generic non-exhaustive patterns.

### Required gates

```bash
cargo fmt --all -- --check
cargo check --all --all-features
cargo test --all --all-features --lib
cargo test --all --all-features purity_vm_jit_parity_snapshots
```

---

## 8. Diagnostics Compatibility Contract

Preserve by default:
- code
- title
- primary label
- ordering class

Week-2 lock-in note:
1. constructor call-site arity mismatch uses `E082`,
2. constructor-pattern arity mismatch uses `E085`,
3. ADT non-exhaustive match uses `E083`,
4. cross-module direct constructor access uses policy split:
   - strict mode: `E086` (blocking error)
   - non-strict mode: `W201` (warning-only; compilation continues)
5. constructor arity checks are diagnostic-driven (no panic path).

Any intentional changes require snapshot review and proposal note.

---

## 9. Acceptance Criteria

1. Constructor field types are enforced deterministically under generics.
2. Module constructor boundary semantics are explicit and stable in diagnostics.
3. HM behavior for nested ADT constructor usage is less permissive/ambiguous.
4. Exhaustiveness precision improves for nested generic constructor spaces.
5. VM/JIT parity remains green on curated ADT matrix.

---

## 10. Explicit Assumptions and Defaults

1. ADT sugar (`type ... = ... | ...`) already landed and desugars to `data`.
2. Factory-only module ADT API policy remains active for this track.
3. No runtime representation changes are required.
4. This is a semantic hardening track after 0.0.4 syntax alignment.

---

## 11. 047 Closure Note (Regression Lock)

Implemented and locked behavior:
1. Constructor arity is enforced with diagnostics in both pattern-lowering paths (`E085`) and constructor call paths (`E082`) (no panic behavior).
2. Cross-module module-qualified constructor boundary policy is locked as:
   - strict mode: reject with `E086`
   - non-strict mode: emit `W201` warning and continue.
3. Existing fixture anchors:
   - pattern arity: `146..148`
   - strict/non-strict boundary split: `149..150`
   - legacy arity fixtures (`26`, `80`) now align with `E085`.

Validation outcomes:
1. `cargo check --all --all-features` -> PASS
2. `cargo test --test error_codes_registry_tests` -> PASS
3. `cargo test --test compiler_rules_tests` -> PASS
4. `cargo test --test snapshot_diagnostics` -> PASS
5. `cargo test --all --all-features purity_vm_jit_parity_snapshots` -> PASS
6. `cargo test --test examples_fixtures_snapshots` -> FAIL (informational, unrelated churn)

Known external churn:
1. `tests/snapshots/examples_fixtures/aoc__2024__day03.snap.new`
2. Out-of-scope for 047 ADT arity/boundary policy lock.
