# Proposal 062: Performance Stabilization Program (No New Features)

**Status:** Draft  
**Date:** 2026-02-28  
**Depends on:** `043_pure_flux_checklist.md`, `044_compiler_phase_pipeline_refactor.md`, `055_lexer_performance_and_architecture.md`, `056_parser_performance_and_architecture.md`

---

## 1. Summary

This proposal defines a stabilization-first performance program with no language/runtime semantics expansion.

The scope is intentionally limited to three lanes:
1. Compiler throughput stability.
2. Runtime VM/JIT stability and parity for hot paths and runtime-error paths.
3. Cache/harness determinism and evidence governance.

This proposal is an execution and governance layer that consolidates and operationalizes existing performance tracks (`044/055/056`) and partial implementation tracks (`019/033`), while preserving current compiler behavior.

---

## 2. Intent and Non-Goals

### 2.1 Intent

1. Make performance regression detection deterministic and auditable.
2. Normalize blocking vs informational gate policy for perf and snapshot churn.
3. Ensure VM/JIT runtime parity remains stable while compile-time diagnostics evolve quickly.
4. Reduce team time spent triaging unrelated drift by enforcing explicit ownership attribution.

### 2.2 Non-Goals

1. No new language syntax/features.
2. No runtime semantic redesign.
3. No broad architecture rewrites under this proposal.
4. No speculative optimization work without baseline and evidence.

---

## 3. Current Grounded State

1. Performance architecture tracks exist but are draft/gap (`044`, `055`, `056`) with baseline artifacts and partial command packs.
2. Runtime and diagnostics parity suites exist and are used as release-quality guards (`043`, `purity_vm_jit_parity_snapshots`, `runtime_vm_jit_parity_release`).
3. Recent diagnostics/typechecker hardening changed error surfaces quickly (`057..061` and follow-ups), increasing snapshot and evidence churn.
4. Friction points persist:
   1. module-root sensitivity in generic examples snapshot harnesses,
   2. inconsistent command/evidence reporting style across tasks,
   3. inconsistent blocking vs informational gate interpretation.

---

## 4. Stabilization Lanes

### 4.1 Lane A: Compiler Throughput Stability

Goals:
1. Lock a canonical baseline corpus for lexer/parser/compiler throughput.
2. Require reproducible perf command packs and artifact schemas.
3. Add deterministic regression policy based on repeated runs and medians.

Policy:
1. Use curated corpora from existing perf tracks (`055`, `056`).
2. Capture run metadata (toolchain/features/host/time) with results.
3. Treat sustained slowdown beyond threshold (defined in milestone tasks) as blocking.

### 4.2 Lane B: Runtime Stability and VM/JIT Outcome Parity

Goals:
1. Keep VM/JIT behavior aligned for representative success and failure paths.
2. Expand parity coverage for runtime-type-error boundary paths (`E1004`) and selected hot paths.
3. Keep parity assertions deterministic (exit code + normalized signature/value contracts).

Policy:
1. Use fixture-backed assertions where possible.
2. Permit backend text-format differences only where explicitly normalized.
3. Runtime parity failures are blocking unless explicitly classified as known infrastructure instability with owner and remediation task.

### 4.3 Lane C: Cache and Harness Determinism

Goals:
1. Stabilize strict/non-strict and backend cache-key validation expectations.
2. Document module-root-dependent fixture behavior and canonical focused command paths.
3. Normalize snapshot churn classification:
   1. intentional (accepted),
   2. unrelated external churn (informational with explicit attribution),
   3. unexpected drift (blocking).

Policy:
1. No harness redesign in initial 062 tranche.
2. Harness limitations must be documented with path + reason + owner in proposal/task closure notes.

---

## 5. Milestones

### M0: Baseline Freeze

1. Consolidate performance command inventory from `055/056/043/060`.
2. Define artifact layout:
   - `perf_logs/062_<timestamp>/context.txt`
   - `perf_logs/062_<timestamp>/commands.list`
   - `perf_logs/062_<timestamp>/commands.log`
   - `perf_logs/062_<timestamp>/results.tsv`
   - `perf_logs/062_<timestamp>/cmd_<n>.log`
3. Publish baseline medians and variance for tracked corpora.

### M1: Gate Normalization

1. Standardize blocking and informational gate classes.
2. Define mandatory evidence outcome labels:
   - `PASS`
   - `FAIL (blocking)`
   - `FAIL (non-blocking, unrelated)`
3. Define canonical known-churn schema:
   - snapshot/test path,
   - reason,
   - owner/task.

### M2: Low-Risk Stabilizations

1. Land only low-risk hot-path cleanups with before/after perf evidence.
2. Keep semantics unchanged.
3. Require parser/diagnostic and VM/JIT parity gates for every stabilization PR.

### M3: Closure

1. Re-run full validation pack.
2. Publish final baseline-vs-current table per lane.
3. Publish unresolved churn ledger with ownership and next action.

---

## 6. Validation Pack

### 6.1 Blocking

1. `cargo check --all --all-features`
2. Parser/diagnostics stability suites used in active tracks.
3. `cargo test --all --all-features --test runtime_vm_jit_parity_release`
4. Codified performance command pack from `055/056` (as finalized under M0/M1).

### 6.2 Informational

1. `cargo test --test examples_fixtures_snapshots`
2. Informational failures are acceptable only with explicit unrelated attribution:
   - path + reason + owner/task.

---

## 7. Evidence Contract for 062 Task PRs

Each PR under 062 must include:
1. commands run with PASS/FAIL classification,
2. before/after metric table (median and run count),
3. snapshot diff rationale,
4. VM/JIT parity status summary,
5. known external churn list (path + owner + reason),
6. explicit statement of semantic non-change.

---

## 8. Initial Task Backlog

### T0 — Baseline Inventory and Command Canonicalization

1. Consolidate commands from `055/056/043/060`.
2. Define and document the 062 artifact schema in `perf_logs/`.

### T1 — Gate Policy Unification

1. Standardize blocking/informational gate rules.
2. Add canonical known-external-churn reporting format.

### T2 — Compiler Performance Stability Locks

1. Add trend-based (not single-sample) benchmark assertions.
2. Ensure parser diagnostics hardening does not produce material throughput regression.

### T3 — Runtime E1004 and Hotpath Stability Locks

1. Ensure runtime parity suite includes representative boundary runtime-error and success hotpath cases.
2. Lock expected signature/value parity semantics.

### T4 — Cache/Harness Determinism Lock

1. Document root-dependent fixture behavior and focused command canonical checks.
2. Keep harness behavior unchanged; only policy/evidence normalization in this phase.

### T5 — Closure Evidence

1. Publish final lane-by-lane table:
   - baseline vs current,
   - gate outcomes,
   - remaining churn with owners.

---

## 9. Risks and Mitigations

1. Benchmark noise causes false regressions.
   - Mitigation: repeated runs, median reporting, fixed command packs.
2. Local-machine tuning overfits results.
   - Mitigation: multi-run policy, environment capture, conservative thresholds.
3. “Optimization” changes semantics.
   - Mitigation: strict semantic non-change policy + parity/diagnostic gates.
4. Snapshot churn hides real regressions.
   - Mitigation: mandatory path-level attribution + ownership.

---

## 10. Acceptance Criteria

1. Performance stabilization process is codified with deterministic commands and artifact schema.
2. Blocking vs informational gate policy is unified and enforced.
3. Compiler throughput and runtime parity lanes each have auditable baseline and current measurements.
4. Cache/harness determinism policy is explicit and consistently applied.
5. No new language/runtime semantics introduced under 062.

---

## 11. Important API / Interface / Type Changes

1. No public language API changes.
2. No runtime semantic API changes.
3. CI/test governance additions are allowed where needed for stabilization gates.
4. Proposal/docs/evidence process changes are the primary output of 062.

