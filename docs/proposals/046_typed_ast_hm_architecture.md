# Proposal 046: Typed AST + TypeEnv Architecture (Post-0.0.4 HM Evolution)

**Status:** Draft  
**Date:** 2026-02-26  
**Depends on:** `032_type_system_with_effects.md`, `043_pure_flux_checklist.md`, `044_compiler_phase_pipeline_refactor.md`

---

## 1. Summary

Move Flux HM typing from the current dual artifact model:
- `TypeEnv` (`name -> scheme`)
- `ExprTypeMap` (internal expression-id -> `InferType`)

to a canonical **Typed AST + TypeEnv** model.

The goal is one semantic typing source of truth with no expression pointer-key lookup layer and no duplicate expression typing logic in compiler validation callsites.

---

## 2. Motivation

Current 0.0.4 HM architecture is intentionally pragmatic and low-risk, but has structural limits:

1. Expression typing is carried in side maps, not on AST nodes.
2. Pointer-based expression identity introduces a compile-invocation invariant.
3. Future AST transform phases require careful ordering to preserve map validity.
4. Compiler validations still need map lookups instead of typed-node access.

A typed AST resolves these by attaching inferred type information directly to each expression node (or typed parallel node), allowing compiler phases to consume typed structure directly.

---

## 3. Goals

1. Make expression typing first-class and structural (typed node), not indirect map lookup.
2. Preserve HM behavior and diagnostics policy already stabilized in 0.0.4.
3. Remove pointer-identity dependence for typed validation paths.
4. Provide a clean foundation for later HM improvements (module/generic/member edge cases).
5. Align with 044 phase-pipeline modularization so typed data flows explicitly between passes.

---

## 4. Non-Goals

1. New language syntax or runtime features.
2. Higher-rank polymorphism.
3. Trait/typeclass system.
4. Effect-system semantic redesign.

---

## 5. Proposed Architecture

### 5.1 Typed Program Artifacts

Introduce a typed-pass output:

```rust
struct TypedProgram {
    program: Program,
    type_env: TypeEnv,
    typed_exprs: TypedExprIndex,
}

struct TypedExpr {
    ty: InferType,
    span: Span,
}
```

`TypedExprIndex` key can be:
- stable AST `NodeId` (preferred), or
- typed parallel tree references if AST id-plumbing is deferred.

### 5.2 Canonical data flow

Pipeline shape:
1. Parse/transform (desugar/fold/rename) to final compile AST.
2. HM inference on final AST.
3. Produce `TypedProgram` (`TypeEnv + typed expressions`).
4. Validation/codegen consume typed expressions directly.

No typed-path expression re-inference and no pointer-key dependency.

### 5.3 Callsite contract

Typed validators must accept typed node info from `TypedExprIndex`:
- let initializer checks
- return-tail checks
- condition/guard checks
- operator/index/member checks
- contract argument checks

Existing diagnostic boundaries remain:
- `E300` for HM mismatch class
- `E425` for strict unresolved where policy applies
- `E055` runtime boundary mismatch only

---

## 6. Migration Plan

### Phase A: Typed artifact introduction (behavior-preserving)

1. Add `TypedProgram` and typed-expression index types.
2. Keep current HM map internals as adapter while exposing typed artifact API.
3. Keep all existing diagnostics and parity snapshots stable.

### Phase B: Validation callsite cutover

1. Replace direct map/pointer lookups in compiler validation code with typed artifact access.
2. Ensure unresolved handling flows through existing strict policy.
3. Remove fallback adapter usage from typed validators.

### Phase C: Identity stabilization

1. Introduce explicit `NodeId` plumbing in AST nodes (if not already complete in this track).
2. Replace pointer-derived ids with stable AST ids.
3. Update transforms to preserve/allocate `NodeId` deterministically.

### Phase D: Cleanup

1. Remove transitional pointer-identity code paths.
2. Remove obsolete typed lookup wrappers.
3. Keep runtime boundary typing paths isolated.

---

## 7. Compiler/Interface Changes

### Internal APIs

New/changed expected APIs:
- `infer_program(...) -> TypedProgram` (or equivalent result struct)
- `Compiler` stores typed artifact for current compile invocation
- `validate_expr_expected_type(...)` consumes typed-expression types, not ad hoc inference

### External APIs

No CLI/user-facing language changes.

---

## 8. Diagnostics Compatibility Contract

This proposal must preserve:
1. error code
2. title
3. primary label text
4. deterministic ordering (especially entry + strict diagnostics)

Reference guardrail suites:
- `tests/purity_vm_jit_parity_snapshots.rs`
- existing HM/type fixtures and snapshots.

---

## 9. Test and Validation Plan

Required gates:

```bash
cargo fmt --all -- --check
cargo check --all --all-features
cargo test --all --all-features --lib
cargo test --all --all-features purity_vm_jit_parity_snapshots
```

Additional targeted suites:
- `tests/type_inference_tests.rs`
- `tests/compiler_rules_tests.rs`
- `tests/primop_compiler_lowering_tests.rs`
- `tests/primop_effect_summary_tests.rs`

New tests to add in this track:
1. Typed artifact completeness: every expression reachable in compile pass has typed entry.
2. Transform stability: typed ids survive parse->transform->infer->validate pipeline.
3. Strict unresolved path still emits `E425` where policy requires.

---

## 10. Rollout and Risk Control

1. Land typed artifact API first with adapter compatibility.
2. Migrate callsites incrementally behind stable tests.
3. Cut pointer identity only after parity suite is green.
4. Snapshot updates allowed only for intentional policy changes.

Risks:
- large AST plumbing diff
- subtle diagnostics ordering drift
- transform/id consistency bugs

Mitigations:
- phase-by-phase gating
- parity suite lock
- invariant tests for typed artifact completeness.

---

## 11. Relationship to Existing Proposals

- `032`: semantic baseline remains authoritative.
- `043`: parity and policy gates remain release-quality contract.
- `044`: this proposal should be executed within/alongside the phase-pipeline refactor track.

This proposal is the HM architecture deepening step after 0.0.4 stabilization.

---

## 12. Acceptance Criteria

1. Typed validation paths no longer rely on pointer-key expression maps.
2. Typed expression data is delivered as canonical compiler artifact.
3. Existing diagnostics policy (`E300/E425/E055`) remains stable.
4. VM/JIT parity suite remains green.
5. Transitional compatibility code is removed or explicitly isolated.

---

## 13. Explicit Assumptions and Defaults

1. 0.0.4 keeps current HM single-source map model; this proposal is post-0.0.4 execution.
2. Typed AST migration prioritizes correctness/diagnostic stability over broad feature expansion.
3. No runtime semantic changes are included.
4. `NodeId` stabilization is required before final pointer-path deletion.
