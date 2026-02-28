# Proposal 063: True FP Completion Program (Feature Delivery)

**Status:** Draft  
**Date:** 2026-02-28  
**Depends on:** `032_type_system_with_effects.md`, `042_effect_rows_and_constraints.md`, `043_pure_flux_checklist.md`, `047_adt_semantics_deepening.md`, `049_effect_rows_completeness.md`, `050_totality_and_exhaustiveness_hardening.md`, `051_any_fallback_reduction.md`, `052_auto_currying_and_partial_application.md`, `053_traits_and_typeclasses.md`, `048_typed_record_types.md`

---

## 1. Summary

This proposal defines the remaining feature work required for Flux to be considered a true functional programming language in practice, not only by vision.

Focus is feature completion and semantic guarantees, not performance tuning and not editor tooling.

Delivery is organized into four execution lanes:
1. principal effect semantics,
2. total and deterministic typing/exhaustiveness,
3. immutable typed data modeling,
4. core FP abstraction ergonomics.

---

## 2. Intent and Non-Goals

### 2.1 Intent

1. Close core semantic gaps that still allow non-principal or ambiguous type/effect behavior.
2. Remove remaining “gradual escape hatches” from strict typed paths.
3. Deliver the missing immutable data and polymorphism primitives needed for day-to-day FP code.
4. Freeze a release-grade FP readiness contract with auditable gates.

### 2.2 Non-Goals

1. No runtime representation redesign unless required by one of the scoped feature tracks.
2. No macro system, package manager, or concurrency expansion in this proposal.
3. No parser-DX/performance-only tasks.
4. No LSP/editor integration work in this proposal.

---

## 3. Current Grounded State

1. Pure-by-default baseline is implemented and parity-guarded (`043`).
2. Effects exist and row constraints are partially solved (`032`, `042`), but principal row completeness is still tracked as gap (`049`).
3. ADT semantics are implemented and hardened (`047`), but totality and Any-fallback reductions are still draft tracks (`050`, `051`).
4. Typed records, currying/partial application, and traits/typeclasses are defined but not implemented (`048`, `052`, `053`).
5. Language vision (`025`) is largely directional; execution-grade closure requires feature owners and release gates.

---

## 4. Delivery Lanes

### 4.1 Lane A — Principal Effect System Completion

Owner proposals: `049` over `042` baseline.

Required outcomes:
1. principal row solving for supported `with` forms,
2. deterministic normalization/equivalence for extension and subtraction,
3. deterministic diagnostics for unresolved and ambiguous row constraints,
4. consistent strict-path behavior across HM and compiler validation boundaries.

Exit condition:
1. no `partial` entries remain in the supported row completeness matrix.

### 4.2 Lane B — Typed Determinism and Totality

Owner proposals: `050`, `051` with `047` dependencies.

Required outcomes:
1. strict-first elimination of disallowed `Any` fallback at typed boundaries,
2. deterministic concrete-conflict surfacing (`E300`) vs truly unresolved strict failures (`E425`),
3. stable conservative exhaustiveness where full theorem proving is out of scope,
4. no backend parity drift in compile-time diagnostics for curated fixtures.

Exit condition:
1. draft-gap rows in `050` and `051` are closed with fixture-backed evidence.

### 4.3 Lane C — Immutable Typed Data Modeling

Owner proposal: `048`.

Required outcomes:
1. typed immutable records (declaration, construction, access, update),
2. compile-time field checking and deterministic diagnostics,
3. HM/compiler integration for record field typing without `Any` leakage,
4. module-boundary behavior aligned with existing strict policy.

Exit condition:
1. records become first-class typed product modeling, removing reliance on untyped hashes for structured domain data.

### 4.4 Lane D — Core FP Abstractions

Owner proposals: `052`, `053`.

Required outcomes:
1. auto-currying and placeholder partial application with deterministic typing/effects,
2. trait/typeclass baseline (`Eq`, `Ord`, `Show`) and constrained generics,
3. dictionary-passing lowering with stable compile-time diagnostics,
4. coherence/orphan rules with deterministic enforcement.

Exit condition:
1. users can write idiomatic compositional FP APIs without dynamic/duck-typed fallbacks.

---

## 5. Milestones

### M0 — Readiness Freeze

1. normalize proposal contracts for `049`, `050`, `051`, `048`, `052`, `053`,
2. publish a single FP readiness matrix with owner and gate mapping.

### M1 — Semantic Core Closure

1. complete Lane A and Lane B,
2. freeze deterministic diagnostics contracts and strict-path behavior.

### M2 — Data and Abstraction Closure

1. complete Lane C and Lane D,
2. expand fixture and parity suites for new typed features.

### M3 — FP Readiness Sign-Off

1. all lane exit conditions satisfied,
2. publish final evidence table and unresolved-churn ledger,
3. mark 063 as implemented and move remaining deferred items to explicit post-063 backlog.

---

## 6. Validation Pack

### 6.1 Blocking

1. `cargo check --all --all-features`
2. `cargo test --test type_inference_tests`
3. `cargo test --test compiler_rules_tests`
4. `cargo test --test pattern_validation`
5. `cargo test --test snapshot_diagnostics`
6. `cargo test --all --all-features purity_vm_jit_parity_snapshots`
7. additional lane-specific suites introduced by `048/052/053` implementations.

### 6.2 Informational

1. `cargo test --test examples_fixtures_snapshots`
2. informational failures are allowed only with explicit attribution:
   1. path,
   2. reason,
   3. owner proposal/task.

---

## 7. Evidence Contract

Each 063 task PR must include:
1. lane and milestone tag,
2. commands run with PASS or FAIL classification,
3. fixture and test mapping to acceptance requirements,
4. diagnostic/snapshot drift rationale,
5. VM/JIT parity status when applicable,
6. explicit statement of non-goal compliance.

---

## 8. Initial Backlog

### T0 — Unified FP Readiness Matrix

1. consolidate unresolved requirements from `049/050/051/048/052/053`,
2. classify each as `must-have for 063` or `deferred`.

### T1 — Effect Rows Principal Completion

1. execute `049` strict-first closure,
2. remove remaining partial row-solving pockets.

### T2 — Any-Fallback and Exhaustiveness Closure

1. execute remaining `051` strict-first tasks,
2. execute `050` conservative totality closure.

### T3 — Typed Records Delivery

1. implement `048` MVP scope,
2. add fixture and parity locks for record typing behavior.

### T4 — Currying and Traits Baseline

1. implement phase-A `052` currying/placeholder scope,
2. implement phase-A `053` traits scope (`Eq`, `Ord`, `Show`).

### T5 — Final Sign-Off

1. run full validation pack,
2. publish lane-by-lane closure evidence and unresolved external churn ledger.

---

## 9. Risks and Mitigations

1. Risk: semantic churn across multiple tracks causes diagnostic instability.
   Mitigation: lane-gated rollout and deterministic snapshot contracts.
2. Risk: feature interactions between HM, rows, and traits create regressions.
   Mitigation: strict-first staging and mandatory compiler/parity gates.
3. Risk: scope creep into non-essential advanced FP features.
   Mitigation: must-have matrix and explicit defer list at M0.
4. Risk: hidden fallback behavior remains in non-obvious paths.
   Mitigation: fixture matrix expansion and strict-path negative assertions.

---

## 10. Acceptance Criteria

1. Lane A-D exit conditions are all satisfied.
2. Principal effect semantics and strict typed determinism are auditable and stable.
3. Typed immutable records are production-ready in scoped MVP form.
4. Currying and baseline traits enable idiomatic functional APIs without dynamic fallback.
5. VM/JIT diagnostic parity remains green for curated compile-time and runtime-critical matrices.
6. No out-of-scope feature expansions are introduced under 063.

---

## 11. Important API / Interface / Type Changes

1. Public language feature additions are expected through owned tracks:
   1. typed records (`048`),
   2. currying/partial application (`052`),
   3. traits/typeclasses (`053`).
2. No compatibility-breaking runtime semantic redesign is allowed by default.
3. Diagnostics families should reuse existing codes where semantics match; new codes only when materially distinct behavior is introduced.

---

## 12. Explicit Assumptions and Defaults

1. `043` remains the pure-by-default baseline and is not reopened.
2. `063` is the execution umbrella for true-FP completion, while individual tracks remain code owners.
3. Strict-first policy remains canonical for determinism-hardening tasks (`049/050/051`).
4. Proposal closure requires auditable evidence, not intent-level status updates.
