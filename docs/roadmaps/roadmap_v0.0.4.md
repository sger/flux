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
   Outcome: full `examples/aoc/2024/day05.flx` remains outside the curated release matrix due known VM stack-overflow behavior in Part 2 recursion; follow-up: [0016_tail_call_accumulator_optimization.md](../proposals/implemented/0016_tail_call_accumulator_optimization.md) (owner: Proposal 0016 track).

7. Release-note diagnostics summary (0058/0059)  
   Outcome:
   - `0058` contextual diagnostics are delivered for call-site argument mismatch, let annotation mismatch, and function return mismatch; named context + dual-span behavior is locked, and the snapshot/parity lock surface is enforced as `code/title/primary label`.
   - `0059` parser UX is delivered for keyword alias suggestions, structural contextual parser messages, and symbol-level corrections (including let/match structural guidance); targeted parser paths remain deterministic without cascade-heavy generic fallback, and the parser snapshot lock surface is governed as `code/title/primary label`.

---

## Proposed Milestones

### M1: HM Zero-Fallback Completion
**Priority:** CRITICAL | **Effort:** Medium | **Risk:** Medium  
**Proposals:** [0051_any_fallback_reduction.md](../proposals/implemented/0051_any_fallback_reduction.md), [0054_0_0_4_hm_adt_exhaustiveness_critical_path.md](../proposals/implemented/0054_0_0_4_hm_adt_exhaustiveness_critical_path.md)

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
**Proposals:** [0047_adt_semantics_deepening.md](../proposals/implemented/0047_adt_semantics_deepening.md), [0054_0_0_4_hm_adt_exhaustiveness_critical_path.md](../proposals/implemented/0054_0_0_4_hm_adt_exhaustiveness_critical_path.md)

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
**Proposals:** [0050_totality_and_exhaustiveness_hardening.md](../proposals/implemented/0050_totality_and_exhaustiveness_hardening.md), [0054_0_0_4_hm_adt_exhaustiveness_critical_path.md](../proposals/implemented/0054_0_0_4_hm_adt_exhaustiveness_critical_path.md)

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
**Proposals:** [0043_pure_flux_checklist.md](../proposals/0043_pure_flux_checklist.md), [0054_0_0_4_hm_adt_exhaustiveness_critical_path.md](../proposals/implemented/0054_0_0_4_hm_adt_exhaustiveness_critical_path.md)

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
**Proposals:** [0055_lexer_performance_and_architecture.md](../proposals/implemented/0055_lexer_performance_and_architecture.md), [0056_parser_performance_and_architecture.md](../proposals/implemented/0056_parser_performance_and_architecture.md), [0044_compiler_phase_pipeline_refactor.md](../proposals/0044_compiler_phase_pipeline_refactor.md)

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

---

## Open Tasks (Merge + Post-0.0.4 Tracking)

### R4-T01: Add 0058/0059 release-note summary in roadmap
**Status:** Complete (pre-merge)  
Week-4 release evidence now includes explicit 0058/0059 diagnostics release-note bullets and lock surface (`code/title/primary label`) under Week-4 Evidence item 7.

### R4-T02: Track Day05 Part 2 VM recursion risk as explicit follow-up
**Status:** Complete (pre-merge)  
Week-4 Evidence item 6 now includes the concrete follow-up tracker [0016_tail_call_accumulator_optimization.md](../proposals/implemented/0016_tail_call_accumulator_optimization.md) and explicit owner (`Proposal 0016 track`) for the Day05 Part 2 VM recursion risk.

### R4-T03: Add contextual boundary/effect mismatch fixtures (0058 follow-up)
**Status:** Complete (post-0.0.4 follow-up landed)  
Fixtures `161` (pass) and `189/190/191` (fail) are present with runnable VM/JIT commands in `examples/type_system/README.md` and `examples/type_system/failing/README.md`, and validation confirms expected outcomes (`E425`, `E1004`, `E400`) with `--root examples/type_system` where required.

### R4-T04: Snapshot lock for 0058 contextual diagnostics
**Status:** Complete (pre-merge)  
0058 contextual diagnostics are snapshot-locked via dedicated full-rendered snapshots for call-site argument mismatch, let-annotation dual-span, and function-return dual-span; `cargo test --test snapshot_diagnostics` is green.

### R4-T05: Expand 0059 parser contextual coverage
**Status:** Complete (post-0.0.4 follow-up landed)  
Contextual coverage for `perform`/`handle` (including arm) and `module` delimiter failures is present in `parser_tests`, `parser_recovery`, and `snapshot_parser` using the 0059 fixtures, and validation is green (`cargo test --test parser_tests`, `cargo test --test parser_recovery`, `cargo test --test snapshot_parser`).

### R4-T06: Add parser-message regression guard
**Status:** Complete (pre-merge)  
Exact contextual `E034` message/hint regression guards are in place for perform/handle/handle-arm/module paths in parser + recovery tests, and `cargo test --test parser_tests` plus `cargo test --test parser_recovery` are green.

### R4-T07: Materialize 0032 deferred tracks into execution tasks
**Priority:** Medium | **Target:** post-0.0.4 planning  
**Status:** Materialized (planning)  
These entries are a post-0.0.4 execution queue. They are explicitly out of v0.0.4 scope and exist to prepare the next planning cycle.

#### Post-0.0.4 Execution Queue (Not in v0.0.4)

What to do now:
1. Keep v0.0.4 focused on hardening and release sign-off.
2. After v0.0.4, start `R4-T07A`, then `R4-T07B`, then `R4-T07C`.
3. Use each entry's checklist and validation gates as the execution contract.
4. Record owner/status/target-release consolidation in `R4-T08`.

##### R4-T07A — Traits/Typeclasses (`0053`)
**Proposal:** [0053_traits_and_typeclasses.md](../proposals/0053_traits_and_typeclasses.md)

**Objective:** Stage an executable plan for nominal/coherent trait/typeclass delivery.

**In Scope:**
- Trait/impl syntax and environment tables.
- Coherence/orphan-rule diagnostics.
- Constraint-carrying HM integration (bounded scope).
- Deterministic diagnostics coverage.

**Out of Scope:**
- Advanced typeclass ecosystem beyond stated baseline.
- GADT/higher-rank/dependent typing.

**Execution Checklist:**
- A1 syntax + parser acceptance matrix.
- A2 trait/impl table build + coherence enforcement.
- A3 constrained-function checks + obligation diagnostics.
- A4 fixtures (`examples/type_system/` + `failing/`) and README command entries.
- A5 snapshot/parity review for new diagnostics.

**Validation Gates:**
- `cargo test --test parser_tests`
- `cargo test --test type_inference_tests`
- `cargo test --test compiler_rules_tests`
- `cargo test --test snapshot_diagnostics`

**Exit Criteria:**
- Coherence/orphan diagnostics stable.
- Constraint mismatch diagnostics deterministic (`code/title/primary label`).
- Fixture and README coverage landed.

**Dependencies / Sequencing:**
- Runs as a post-0.0.4 stage-2 feature track under roadmap/proposal sequencing.
- Coordinate with `0052` (currying/partials) only where trait ergonomics overlap.

**Owner Track:**
- Proposal `0053` owner track.

##### R4-T07B — JIT Type-Specialization/Perf (`0062`)
**Proposal:** [0062_performance_stabilization_program.md](../proposals/0062_performance_stabilization_program.md)

**Objective:** Create a stabilization-first execution lane for perf without semantic expansion.

**In Scope:**
- Baseline corpus and reproducible command packs.
- Perf artifact schema + ownership.
- Low-risk stabilization tasks with parity protection.

**Out of Scope:**
- New language/runtime semantics.
- Broad architecture rewrites under this track.

**Execution Checklist:**
- B1 baseline freeze + command inventory.
- B2 artifact contract (`perf_logs/...` context + command logs).
- B3 lane gates (compiler throughput, runtime parity, cache/harness determinism).
- B4 low-risk optimization candidate queue with evidence requirements.

**Validation Gates:**
- `cargo check --all --all-features`
- `cargo test --all --all-features purity_vm_jit_parity_snapshots`
- Bench command pack documented and reproducible.

**Exit Criteria:**
- No un-attributed perf diffs.
- No parity/diagnostic tuple regressions.
- Evidence logs attached to each stabilization item.

**Dependencies / Sequencing:**
- Sequence after v0.0.4 hardening lock and coordinate with `044/055/056` architecture/perf tracks.
- Block semantic optimizations that exceed stabilization-first scope.

**Owner Track:**
- Proposal `0062` owner track.

##### R4-T07C — Effect-Handler Compilation Strategy (`0063`)
**Proposal:** [0063_true_fp_completion_program.md](../proposals/0063_true_fp_completion_program.md)

**Objective:** Materialize effect-handler strategy work as an executable lane with deterministic semantics gates.

**In Scope:**
- Lane-A effect/principal row completion tasks linked from `0063`.
- Deterministic handler diagnostics behavior.
- Sequencing with `049/042` closure dependencies.

**Out of Scope:**
- Perf-only work (`0062` lane).
- Tooling/editor/package-manager expansions.

**Execution Checklist:**
- C1 readiness matrix row for handler strategy.
- C2 explicit subtask list for compilation/lowering strategy decisions.
- C3 fixture expansion for handler edge paths (`examples/type_system` + `failing`).
- C4 diagnostics + parser/recovery guards for handler-context errors.

**Validation Gates:**
- `cargo test --test type_inference_tests`
- `cargo test --test compiler_rules_tests`
- `cargo test --test parser_tests`
- `cargo test --test parser_recovery`
- `cargo test --all --all-features purity_vm_jit_parity_snapshots`

**Exit Criteria:**
- Handler strategy tasks are fully enumerated and sequenced.
- Deterministic diagnostics and recovery coverage for targeted handler paths.
- Clear dependency mapping to `049/042` and `0063` milestone lane.

**Dependencies / Sequencing:**
- Depends on principal effect-row closure milestones from `042` baseline and `049` completion.
- Coordinate with `0063` lane sequencing and post-0.0.4 FP completion readiness.

**Owner Track:**
- Proposal `0063` owner track.

R4-T08 consolidates these scoped entries into the deferred-policy owner/status/target-release matrix.

### R4-T08: Add deferred-policy tracker section
**Priority:** Medium | **Target:** pre-merge  
**Status:** Complete (pre-merge)  
The deferred-policy matrix below is the single tracking surface for deferred items from `0032`, `0058`, and `0059`.

#### Deferred-Policy Tracker (0032/0058/0059)

| Item | Source Proposal | Owner | Status | Target Release | Tracker |
|---|---|---|---|---|---|
| Traits/typeclasses (`0053`) | `0032` | Proposal `0053` track | Planned (post-0.0.4) | Post-0.0.4 (stage-2) | [0053_traits_and_typeclasses.md](../proposals/0053_traits_and_typeclasses.md) |
| Effect-handler compilation strategy (`0063`) | `0032` | Proposal `0063` track | Planned (post-0.0.4) | Post-0.0.4 | [0063_true_fp_completion_program.md](../proposals/0063_true_fp_completion_program.md) |
| JIT type-specialization/perf (`0062`) | `0032` | Proposal `0062` track | Planned (post-0.0.4) | Post-0.0.4 | [0062_performance_stabilization_program.md](../proposals/0062_performance_stabilization_program.md) |
| Contextual boundary/effect mismatch expansion | `0058` | Proposal `0058` follow-up track | Complete (pre-merge + follow-up landed) | v0.0.4 hardening follow-up lane | [R4-T03](#r4-t03-add-contextual-boundaryeffect-mismatch-fixtures-0058-follow-up), [R4-T04](#r4-t04-snapshot-lock-for-0058-contextual-diagnostics) |
| Parser contextual coverage + regression guard hardening | `0059` | Proposal `0059` follow-up track | Complete (pre-merge + follow-up landed) | v0.0.4 hardening follow-up lane | [R4-T05](#r4-t05-expand-0059-parser-contextual-coverage), [R4-T06](#r4-t06-add-parser-message-regression-guard) |

### R4-T09: Record dated merge-gate evidence
**Priority:** High | **Target:** pre-merge  
**Status:** Complete (pre-merge)  
**Run date:** 2026-03-01 (EET)

Recorded command evidence:
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --test type_inference_tests`
- `cargo test --test compiler_rules_tests`
- `cargo test --test parser_tests`
- `cargo test --test parser_recovery`
- `cargo test --test snapshot_parser`
- `cargo test --test snapshot_diagnostics`
- `cargo test --all --all-features purity_vm_jit_parity_snapshots`

Merge-gate results:
- `cargo fmt --all -- --check`: pass
- `cargo clippy --all-targets --all-features -- -D warnings`: pass
- `cargo test --test type_inference_tests`: pass (73/73)
- `cargo test --test compiler_rules_tests`: pass (132/132)
- `cargo test --test parser_tests`: pass (115/115)
- `cargo test --test parser_recovery`: pass (11/11)
- `cargo test --test snapshot_parser`: pass (16/16)
- `cargo test --test snapshot_diagnostics`: pass (6/6)
- `cargo test --all --all-features purity_vm_jit_parity_snapshots`: pass

Tooling guide presence:
- `docs/tooling/parser_benchmarking.md`: present
- `docs/tooling/compiler_benchmarking.md`: present

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
   - `docs/proposals/0043_pure_flux_checklist.md`
   - `docs/proposals/0000_index.md`
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
