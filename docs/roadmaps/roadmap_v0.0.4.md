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

### Week 4 Evidence (Parity Freeze + Runtime Stability)

1. `cargo fmt --all -- --check`  
   Outcome: formatting gate passed for parity-freeze branch state.

2. `cargo check --all --all-features`  
   Outcome: clean compile with release parity test matrix additions.

3. `cargo test --all --all-features purity_vm_jit_parity_snapshots`  
   Outcome: compile-diagnostic tuple parity gate remains green (`code/title/primary label` lock).

4. `cargo test --all --all-features --test runtime_vm_jit_parity_release`  
   Outcome: curated runtime parity matrix passes:
   - typed module-qualified runtime value parity,
   - ADT constructor/match runtime value parity,
   - controlled runtime panic signature parity,
   - AoC CLI outcome parity (`day04.flx` deterministic `E011` compile parity, `day05_part1_test.flx` VM/JIT pass parity).

5. Runtime smoke commands  
   Outcome:
   - `examples/aoc/2024/day04.flx`: VM/JIT both fail with `E011` private-member diagnostic.
   - `examples/aoc/2024/day05_part1_test.flx` (`--test`): VM/JIT both pass all 4 tests.

6. Residual risk note  
   Outcome: full `examples/aoc/2024/day05.flx` remains outside the curated release matrix due known VM stack-overflow behavior in Part 2 recursion; tracked as post-0.0.4 runtime hardening follow-up.

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
cargo test --all --all-features --test runtime_vm_jit_parity_release

cargo run -- --no-cache --root examples/aoc/2024 examples/aoc/2024/day04.flx
cargo run --features jit -- --no-cache --root examples/aoc/2024 examples/aoc/2024/day04.flx --jit
cargo run -- --no-cache --test --root lib --root examples/aoc/2024 examples/aoc/2024/day05_part1_test.flx
cargo run --features jit -- --no-cache --test --root lib --root examples/aoc/2024 examples/aoc/2024/day05_part1_test.flx --jit
```

---

### M5: Safe Performance Track (Release-Compatible)
**Priority:** HIGH | **Effort:** Small-Medium | **Risk:** Low  
**Proposals:** [055_lexer_performance_and_architecture.md](../proposals/055_lexer_performance_and_architecture.md), [056_parser_performance_and_architecture.md](../proposals/056_parser_performance_and_architecture.md), [044_compiler_phase_pipeline_refactor.md](../proposals/044_compiler_phase_pipeline_refactor.md)

**Status:** Complete (baseline harnesses, two low-risk hot-path changes, and parity-safe evidence locked)

**Goal:** Land measurable wins without semantic risk.

**Implementation Focus (allowed in 0.0.4 window):**
- Baseline instrumentation and benchmark harness.
- 1-2 hot-path optimizations with clear before/after metrics.
- No parser grammar changes, no diagnostics policy changes.

**Deferred:**
- Large module/pipeline refactors and deeper architecture split continue post-0.0.4.

### M5 Evidence (Safe Performance Track)

1. Benchmark harness expansion
- Added `benches/parser_bench.rs` (4 corpora: declaration/expression/string+interp+comments/malformed-recovery).
- Added `benches/compiler_compile_bench.rs` (`compile_with_opts(false,false)` and `compile_with_opts(false,true)`).
- Added tooling guides:
  - `docs/tooling/parser_benchmarking.md`
  - `docs/tooling/compiler_benchmarking.md`

2. Hot-path optimization #1 (parser Pratt loop)
- Added single-table parse-loop precedence lookup and switched parser loop to one lookup:
  - `src/syntax/precedence.rs`
  - `src/syntax/parser/expression.rs`

3. Hot-path optimization #2 (compiler clone elimination)
- `compile_with_opts` now borrows original `Program` on non-optimized paths and only allocates transformed AST on optimize path:
  - `src/bytecode/compiler/mod.rs`

4. Baseline vs current benchmark evidence (time ranges)

| Benchmark | Baseline | Current | Delta summary |
|---|---|---|---|
| `lexer/next_token_loop/identifier_heavy` | `3.7637 .. 3.8832 ms` | `3.7306 .. 3.8499 ms` | ~0.8-3.0% faster (within noise threshold on latest run) |
| `lexer/next_token_loop/string_escape_interp_heavy` | `2.7487 .. 2.8046 ms` | `2.6624 .. 2.7470 ms` | ~2.6-4.3% faster |
| `parser/parse_program/expression_operator_heavy` | `18.951 .. 19.278 ms` | `18.527 .. 18.933 ms` | ~2.0% faster median |
| `parser/parse_program/string_interp_comment_heavy` | `4.6056 .. 4.6790 ms` | `4.4197 .. 4.5559 ms` | ~3.3% faster median |
| `parser/parse_program/malformed_recovery_heavy` | `5.5701 .. 5.6711 ms` | `5.3812 .. 5.4969 ms` (isolated rerun) | no regression after isolated rerun; earlier slower run treated as noise |
| `compiler/compile_with_opts_no_analyze/typed_function_heavy` | `17.331 .. 18.369 ms` | `17.368 .. 17.789 ms` | effectively neutral/slight improvement |
| `compiler/compile_with_opts_analyze/typed_function_heavy` | `17.494 .. 17.726 ms` | `17.753 .. 18.149 ms` | slight regression (~1.5-2.5%), within low-risk envelope |

Reproducibility logs:
- lexer baseline: `perf_logs/lexer-bench-20260227-163754.log`
- lexer current: `perf_logs/lexer-bench-20260227-174828.log`
- parser baseline: `perf_logs/parser-bench-20260227-164004.log`
- parser current: `perf_logs/parser-bench-20260227-175000.log`
- parser malformed isolated reruns:
  - `perf_logs/parser-malformed-only-20260227-175351.log`
  - `perf_logs/parser-malformed-only-20260227-175409.log`
- compiler baseline: `perf_logs/compiler-bench-20260227-164429.log`
- compiler current: `perf_logs/compiler-bench-20260227-175048.log`

Command pack used:
- `cargo bench --bench lexer_bench`
- `cargo bench --bench parser_bench`
- `cargo bench --bench compiler_compile_bench`

5. Regression/parity lock (post-optimization)
- `cargo fmt --all -- --check`: pass
- `cargo check --all --all-features`: pass
- `cargo test --test lexer_tests`: pass (62/62)
- `cargo test --test parser_tests`: pass (92/92)
- `cargo test --test parser_recovery`: pass (2/2)
- `cargo test --test snapshot_lexer`: pass (12/12)
- `cargo test --test snapshot_parser`: pass (13/13)
- `cargo test --all --all-features purity_vm_jit_parity_snapshots`: pass
- `cargo test --all --all-features --test runtime_vm_jit_parity_release`: pass (5/5)

M5 closure:
- Acceptance criteria satisfied for v0.0.4 safe-performance scope.
- Remaining parser/compiler architecture deepening continues post-0.0.4 under 044/055/056 full tracks.

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
