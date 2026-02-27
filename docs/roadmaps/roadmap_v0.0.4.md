# Flux v0.0.4 Implementation Plan

## Overview

Create a focused plan for Flux v0.0.4 as a **hardening release**: complete type-system correctness work, lock VM/JIT diagnostics parity, and land only low-risk performance improvements that do not change language behavior.

---

## Current State (v0.0.3 - Complete)

**Implemented Foundations:**
- Typed/effect checker baseline with pure-by-default direction
- Effect signatures, propagation, and handler flows
- Strict-mode checks for public API boundaries
- Real-program fixtures and `Flow.FTest` unit-test workflow
- VM/JIT execution modes with parity snapshot infrastructure

**Current Gaps for v0.0.4 Target:**
- HM hardening still has unresolved/fallback edge pockets
- ADT semantics and module-boundary behavior need deterministic lock-in
- Exhaustiveness must be stronger and consistent across non-ADT + ADT spaces
- Some runtime/backend behavior gaps still appear in complex workloads

---

## Version Goals for v0.0.4

**Primary Objectives:**
1. **HM Soundness:** zero typed-path fallback on runtime compatibility paths.
2. **ADT Hardening:** stabilize constructor/module semantics for supported surface.
3. **Totality:** strong, deterministic compile-time exhaustiveness behavior.
4. **Parity:** VM/JIT compile diagnostics and curated fixture parity remain locked.
5. **Performance (Safe):** benchmark-first, low-risk parser/lexer/compiler micro-wins only.

**Success Criteria:**
- No broad `Any`-style rescue in typed validation paths.
- Stable diagnostics boundaries: `E300`, `E425`, `E055`, `E015`, `E083`, `E400` family.
- Curated parity suite green for VM/JIT.
- No language syntax expansion required for release sign-off.
- Measurable low-risk perf win(s) with no diagnostics drift.

**Timeline:** 4 weeks (through end of March 2026)
- Week 1: HM zero-fallback completion
- Week 2: ADT semantics hardening
- Week 3: strong exhaustiveness completion + runtime stabilization
- Week 4: parity freeze + release sign-off + safe perf wins

---

## Quick Reference: Timeline Overview

```
┌─────────────────────────────────────────────────────────────────┐
│ Week 1: HM Zero-Fallback Completion                            │
│   ✓ Strict HM typed-path authority                             │
│   ✓ Remove typed-path runtime-compat rescue                    │
│   ✓ Deterministic typed mismatch diagnostics                   │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Week 2: ADT Semantics Hardening                                │
│   ✓ Constructor/arity/type checks stabilized                   │
│   ✓ Module boundary policy behavior locked                     │
│   ✓ Predictable ADT behavior for supported release contract    │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Week 3: Strong Exhaustiveness + Runtime Stability              │
│   ✓ General + ADT coverage behavior finalized                  │
│   ✓ Guard semantics deterministic                              │
│   ✓ Backend/runtime edge-case fixes blocking parity            │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Week 4: Parity Freeze + Sign-off + Safe Perf                  │
│   ✓ VM/JIT parity lock-in                                      │
│   ✓ Release checklist closure                                  │
│   ✓ Low-risk perf improvements with measurement evidence       │
└─────────────────────────────────────────────────────────────────┘

Total: 4 weeks (March 2026 release window)
```

### Week 1 Evidence (HM Zero-Fallback)

1. `cargo check --all --all-features`  
   Outcome: clean compile with strict HM typed-path flow and diagnostics suppression wiring intact.

2. `cargo test --test compiler_rules_tests`  
   Outcome: 61/61 passed, including unresolved boundary and HM mismatch regression coverage (`E300`/`E425` policy paths).

3. `cargo test --lib bytecode::compiler::compiler_test`  
   Outcome: 17/17 passed, including explicit guard tests that typed-let inference does not use runtime-compat fallback helpers.

4. `cargo test --all --all-features purity_vm_jit_parity_snapshots`  
   Outcome: parity snapshot gate passed (`purity_vm_jit_parity_snapshots` green), preserving VM/JIT tuple invariants.

5. `cargo fmt --all -- --check`  
   Outcome: passing after formatting normalization.

**Post v0.0.4 (deferred tracks):**
- Auto-currying / placeholder partial application (`052`)
- Traits / typeclasses (`053`)
- Typed records (`048`)
- Larger architecture/perf refactor (`044`, `055`, `056` full execution)

### Week 2 Evidence (ADT Semantics Hardening)

1. `cargo check --all --all-features`  
   Outcome: compile passes after HM ADT constructor-call/pattern typing deepening.

2. `cargo test --test type_inference_tests`  
   Outcome: ADT HM coverage tests pass (constructor generic instantiation + constructor-pattern type propagation).

3. `cargo test --test compiler_rules_tests`  
   Outcome: module ADT constructor boundary regression passes with dedicated class (`E084`).

4. `cargo run -- --no-cache --root examples/type_system examples/type_system/failing/66_module_constructor_not_public_api.flx`  
   Outcome: deterministic ADT boundary failure on `E084` (no `E004` fallback path).

5. `cargo run --features jit -- --no-cache --root examples/type_system examples/type_system/failing/66_module_constructor_not_public_api.flx --jit`  
   Outcome: JIT path matches VM diagnostic class/title for ADT boundary misuse.

6. `cargo test --all --all-features purity_vm_jit_parity_snapshots`  
   Outcome: parity suite green after intentional snapshot update (`E004 -> E084` for fixture 66).

### Week 3 Evidence (Strong Exhaustiveness Completion)

1. `cargo fmt --all -- --check`  
   Outcome: formatting gate passed after exhaustiveness routing/test updates.

2. `cargo check --all --all-features`  
   Outcome: clean compile with deterministic general-vs-ADT exhaustiveness routing and guard semantics lock.

3. `cargo test --test compiler_rules_tests`  
   Outcome: 66/66 passed, including Week-3 lock tests:
   - `match_bool_guarded_only_reports_deterministic_missing_order`
   - `adt_match_all_constructor_arms_guarded_is_non_exhaustive_e083`
   - `adt_match_guarded_constructor_with_unguarded_fallback_is_exhaustive`
   - `adt_match_mixed_constructor_spaces_reports_e083`

4. `cargo test --test pattern_validation`  
   Outcome: 9/9 passed, including guarded wildcard non-exhaustive behavior (`E015`) and guarded+fallback acceptance.

5. `cargo test --all --all-features purity_vm_jit_parity_snapshots`  
   Outcome: parity snapshot gate passed; compile-diagnostic tuples remain aligned across VM/JIT for curated purity/type fixtures.

---

## Proposed Milestones

### M1: HM Zero-Fallback Completion
**Priority:** CRITICAL | **Effort:** Medium | **Risk:** Medium  
**Proposals:** [051_any_fallback_reduction.md](../proposals/051_any_fallback_reduction.md), [054_0_0_4_hm_adt_exhaustiveness_critical_path.md](../proposals/054_0_0_4_hm_adt_exhaustiveness_critical_path.md)

**Goal:** Make HM strict-path authority for typed validation callsites.

**Implementation Focus:**
- Eliminate typed-path reliance on runtime-compat expression typing for strict HM validation paths.
- Ensure unresolved handling is policy-accurate (`E425` strict path only where intended).
- Keep typed mismatch class stable on `E300`.
- Keep runtime-boundary mismatch class on `E055` only.

**Required Validation:**
- typed-let mismatch coverage (identifier and call-return)
- operator/condition/index/member typed checks
- strict unresolved regression fixtures
- VM/JIT diagnostics parity on updated HM fixtures

---

### M2: ADT Semantics Hardening
**Priority:** CRITICAL | **Effort:** Medium | **Risk:** Medium  
**Proposals:** [047_adt_semantics_deepening.md](../proposals/047_adt_semantics_deepening.md), [054_0_0_4_hm_adt_exhaustiveness_critical_path.md](../proposals/054_0_0_4_hm_adt_exhaustiveness_critical_path.md)

**Goal:** Stabilize ADT constructor typing and supported module-boundary behavior.

**Implementation Focus:**
- Constructor arity/type diagnostics consistency.
- Generic constructor behavior in supported contexts.
- Module boundary policy lock-in for 0.0.4 documented contract.

**Required Validation:**
- pass/fail constructor fixtures (arity/type mismatch)
- nested ADT behavior fixture matrix
- no unintended diagnostics class churn (`E082`, `E083` where applicable)

---

### M3: Strong Exhaustiveness Completion
**Priority:** CRITICAL | **Effort:** Medium | **Risk:** Medium  
**Proposals:** [050_totality_and_exhaustiveness_hardening.md](../proposals/050_totality_and_exhaustiveness_hardening.md), [054_0_0_4_hm_adt_exhaustiveness_critical_path.md](../proposals/054_0_0_4_hm_adt_exhaustiveness_critical_path.md)

**Goal:** Deterministic compile-time totality checks for supported pattern spaces.

**Implementation Focus:**
- Preserve diagnostic class split:
  - general non-exhaustive: `E015`
  - ADT constructor-space non-exhaustive: `E083`
- Guard semantics: guarded-only arms do not close coverage.

**Required Validation:**
- bool/list/tuple/general-match fail fixtures
- guarded wildcard edge cases
- ADT non-exhaustive regressions remain deterministic

---

### M4: VM/JIT Parity Freeze + Runtime Stability
**Priority:** CRITICAL | **Effort:** Medium | **Risk:** High  
**Proposals:** [043_pure_flux_checklist.md](../proposals/043_pure_flux_checklist.md), [054_0_0_4_hm_adt_exhaustiveness_critical_path.md](../proposals/054_0_0_4_hm_adt_exhaustiveness_critical_path.md)

**Goal:** Ensure parity and runtime behavior are release-safe.

**Implementation Focus:**
- Eliminate backend inconsistencies in curated type/effect and AoC-style real workloads.
- Lock snapshot tuple invariants (`code/title/primary label`).

**Required Validation:**
```bash
cargo fmt --all -- --check
cargo check --all --all-features
cargo test --all --all-features purity_vm_jit_parity_snapshots
```

---

### M5: Safe Performance Track (Release-Compatible)
**Priority:** HIGH | **Effort:** Small-Medium | **Risk:** Low  
**Proposals:** [055_lexer_performance_and_architecture.md](../proposals/055_lexer_performance_and_architecture.md), [056_parser_performance_and_architecture.md](../proposals/056_parser_performance_and_architecture.md), [044_compiler_phase_pipeline_refactor.md](../proposals/044_compiler_phase_pipeline_refactor.md)

**Goal:** Land measurable wins without semantic risk.

**Implementation Focus (allowed in 0.0.4 window):**
- Baseline instrumentation and benchmark harness.
- 1-2 hot-path optimizations with clear before/after metrics.
- No parser grammar changes, no diagnostics policy changes.

**Deferred:**
- Large module/pipeline refactors and deeper architecture split continue post-0.0.4.

---

## Diagnostics Contract (Release Lock)

Keep these classes stable for v0.0.4:
- `E300` HM/type mismatch
- `E425` strict unresolved typed boundary
- `E055` runtime boundary mismatch
- `E015` general non-exhaustive match
- `E083` ADT constructor-space non-exhaustive
- `E400` family effect contract violations

Any intentional change requires snapshot review + proposal note.

---

## Release Gate Checklist (v0.0.4)

1. HM/ADT/exhaustiveness fixture matrix is green in VM.
2. Same matrix and parity snapshots are green in JIT.
3. No unresolved typed-path fallback gaps remain in strict-path validations.
4. Docs aligned:
   - `docs/internals/type_system_effects.md`
   - `docs/proposals/043_pure_flux_checklist.md`
   - `docs/proposals/000_index.md`
5. Safe perf evidence captured (if perf items landed in release).

---

## Explicit Non-Goals for v0.0.4

1. No trait/typeclass implementation (`053`) in this release.
2. No currying/placeholder partials (`052`) in this release.
3. No typed records (`048`) in this release.
4. No large architecture split as a release blocker (`044` major phase).

---

## Risk Management

1. **Scope Creep Risk:** feature-track items consume hardening time.  
   **Mitigation:** strict blocker-first milestone sequencing.

2. **Parity Drift Risk:** diagnostics diverge across VM/JIT while hardening.  
   **Mitigation:** frequent parity snapshot runs and tuple-lock policy.

3. **Runtime Stability Risk:** deep recursive workloads reveal VM limits late.  
   **Mitigation:** include real workload smoke tests in week-3/4 gates.

4. **Perf Regression Risk:** rushed optimizations before release.  
   **Mitigation:** benchmark-first rule and low-risk-only filter.

---

## Suggested Ownership Map

- HM hardening owner: Proposal `051`
- ADT semantics owner: Proposal `047`
- Exhaustiveness owner: Proposal `050`
- Release orchestration owner: Proposal `054`
- Perf starters: Proposals `055` + `056` (safe subset only for v0.0.4)
