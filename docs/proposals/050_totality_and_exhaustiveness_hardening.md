# Proposal 050: Totality and Exhaustiveness Hardening

**Status:** Draft  
**Date:** 2026-02-26  
**Depends on:** `032_type_system_with_effects.md`, `043_pure_flux_checklist.md`, `047_adt_semantics_deepening.md`, `docs/internals/type_system_effects.md`

---

## 1. Summary

Strengthen compile-time totality/exhaustiveness guarantees for supported match spaces so missing cases are caught deterministically and runtime match failures are minimized.

This proposal formalizes coverage rules and guard behavior without introducing new pattern syntax.

---

## 2. Problem Statement

Flux has improved exhaustiveness checks, but totality coverage is still uneven across domains and nested pattern shapes.

Current pain points:

1. Coverage precision differs by domain (Bool/list/sum-like/tuple/ADT nested spaces).
2. Guarded-arm treatment can still be misunderstood in user expectations.
3. Residual runtime failure boundary for unsupported pattern-space reasoning is not centralized in one explicit contract.

Impact:
- users can still encounter runtime misses in cases they expected compile-time rejection,
- diagnostics can be less predictable across equivalent pattern structures.

---

## 3. Goals

1. Define domain-by-domain totality behavior as an explicit contract.
2. Normalize guard semantics across all match coverage logic.
3. Improve nested constructor-space and tuple/list coverage where currently partial.
4. Make residual runtime-failure boundary explicit and narrow.
5. Preserve deterministic diagnostics and VM/JIT parity expectations.

---

## 4. Non-Goals

1. Full theorem proving over arbitrary guards.
2. New pattern syntax or or-pattern redesign.
3. Runtime pattern engine redesign.
4. Record-pattern totality (until record typing proposal lands).

---

## 5. Coverage Domains (Canonical Matrix)

Each domain must be tagged `guaranteed | conservative | unsupported`.

1. Bool (`true`/`false`).
2. Option (`None`/`Some`).
3. Either (`Left`/`Right`).
4. List partition (`[]` and `[h | t]`).
5. Tuple finite-position coverage (shape-aware, value-space conservative).
6. ADT constructors (top-level and nested constructor-space where supported).

Goal state:
1. Bool/Option/Either/List = `guaranteed`.
2. ADT nested constructor-space = `guaranteed` for declared-supported shapes.
3. Tuple = `conservative` with explicit catch-all requirements where full value-space enumeration is infeasible.

---

## 6. Guard Semantics (Locked)

1. Guarded arms never provide unconditional coverage on their own.
2. A guarded wildcard does not satisfy catch-all requirements.
3. Only unguarded wildcard/identifier catch-all arms provide unconditional fallback.
4. Diagnostics must clearly state guard-conditional non-coverage when relevant.

---

## 7. Formalized Coverage Algorithm (Implementation Contract)

1. Build domain-specific constructor/value partitions for the scrutinee type.
2. For each arm:
   - compute covered partition subset,
   - mark as conditional if guarded.
3. Compute unconditional union of unguarded arm coverage.
4. If unconditional union does not cover full domain, emit non-exhaustive failure.
5. Keep conservative fallback when scrutinee domain is not fully known.

Determinism requirements:
1. stable first-missing-case reporting,
2. stable diagnostic ordering when multiple misses exist.

---

## 8. Diagnostics Policy

1. Reuse `E015` for non-exhaustive match where class is unchanged.
2. Reuse existing ADT exhaustiveness diagnostics where applicable (`E083` class if active).
3. Add new codes only if a materially new failure class is introduced.
4. Message templates must include:
   - uncovered domain summary,
   - note when guards were excluded from unconditional coverage.

---

## 9. Residual Runtime-Failure Policy

Runtime match failure is acceptable only when one of the following holds:

1. scrutinee domain is intentionally dynamic/unknown (`Any`-driven path),
2. pattern-space reasoning for the shape is explicitly unsupported,
3. code is in a documented gradual fallback path.

All such cases must be documented in one support table and not silently expanded.

---

## 10. Staged Rollout Policy

### Stage 1 (strict-first)

1. Apply strongest exhaustiveness hardening in strict and typed/HM-known contexts.
2. Keep conservative behavior in unresolved gradual contexts.

### Stage 2 (default typed)

1. Promote hardened checks to default typed contexts after fixture confidence.
2. Keep explicit exceptions for unsupported domains.

### Stage 3 (stabilization)

1. Freeze diagnostics snapshots.
2. Remove transitional compatibility conditions.

---

## 11. Test and Snapshot Strategy

### Required scenarios

1. Guarded wildcard-only non-coverage.
2. Nested ADT constructor-space miss cases.
3. Bool/list complete vs incomplete cases.
4. Tuple conservative behavior with and without catch-all.
5. Mixed guarded + unguarded-arm coverage.

### Required assets

1. Unit coverage tests for domain partition logic.
2. Compiler integration tests for diagnostics shape.
3. Fixture snapshots for representative pass/fail cases.
4. At least one parity snapshot case per major domain category.

---

## 12. VM/JIT Parity Contract

Compile-time diagnostics for curated totality fixtures must match exactly on:

1. code,
2. title,
3. primary label.

No backend-specific exhaustiveness diagnostic drift is allowed.

---

## 13. Release Gate

A release candidate passes only if:

1. domain matrix and support table are complete and up to date,
2. required totality fixtures are green,
3. parity snapshots are green for totality categories,
4. residual runtime-failure cases are explicitly documented and unchanged.

---

## 14. Risks and Mitigations

1. Risk: false-positive non-exhaustive errors.
   - Mitigation: conservative-domain labeling and support-table transparency.
2. Risk: user confusion around guards.
   - Mitigation: explicit guard semantic messaging in diagnostics/docs.
3. Risk: diagnostic churn across nested ADT paths.
   - Mitigation: snapshot gating and deterministic ordering rules.

---

## 15. Assumptions and Defaults

1. Existing pattern syntax remains unchanged.
2. Guard semantics are intentionally conservative.
3. Rollout is staged with strict-first hardening.
4. Canonical semantics source remains `docs/internals/type_system_effects.md`.
