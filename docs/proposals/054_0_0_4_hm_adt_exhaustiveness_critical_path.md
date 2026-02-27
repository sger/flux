# Proposal 054: 0.0.4 HM/ADT/Exhaustiveness Critical Path + Post-0.0.4 Hardening Roadmap

**Status:** Draft  
**Date:** 2026-02-26  
**Depends on:** `043_pure_flux_checklist.md`, `047_adt_semantics_deepening.md`, `050_totality_and_exhaustiveness_hardening.md`, `051_any_fallback_reduction.md`, `052_auto_currying_and_partial_application.md`, `053_traits_and_typeclasses.md`, `048_typed_record_types.md`, `044_compiler_phase_pipeline_refactor.md`, `055_lexer_performance_and_architecture.md`

---

## 1. Summary

This proposal is the execution orchestrator for the full hardening roadmap:

1. Stage 1 (0.0.4 release blocker): HM zero-fallback + ADT semantics + strong exhaustiveness + parity/sign-off.
2. Stage 2 (post-0.0.4): currying (`052`), traits (`053`), typed records (`048`), architecture/perf starter (`044`).

It does not redefine language semantics already specified in the owning proposals; it fixes sequencing, gates, and ownership.

---

## 2. Scope Lock

### In scope

1. Compiler semantic hardening and diagnostics stability.
2. Fixture/snapshot parity governance.
3. Feature rollout sequencing for `052`, `053`, `048`.
4. Internal architecture/perf follow-up sequencing for `044`.

### Out of scope

1. Concurrency (`026`) as release blocker.
2. GC (`045`) as release blocker.
3. Higher-rank polymorphism and theorem-proving style exhaustiveness.

---

## 3. Locked Global Diagnostics Contract

Preserve existing classes where semantics match:

1. `E300` HM/type mismatch
2. `E400` effect boundary family
3. `E055` runtime boundary mismatch
4. `E015` general non-exhaustive match
5. `E083` ADT constructor-space non-exhaustive match
6. `E425` strict unresolved typed boundary

Any intentional class/title/primary-label change must be snapshot-reviewed and noted.

---

## 4. Stage 1 (Weeks 1-4): 0.0.4 Release Blockers

### Week 1: HM Zero-Fallback Completion (owner: 051 track)

Objective:
- typed/inferred validation does not depend on runtime-compat rescue.

File focus:
- `src/bytecode/compiler/hm_expr_typer.rs`
- `src/ast/type_infer.rs`
- `src/bytecode/compiler/statement.rs`
- `src/bytecode/compiler/expression.rs`
- `src/bytecode/compiler/mod.rs`

Gates:
- typed let mismatch checks (identifier + typed call return)
- module/member/generic inference edge cases
- index/tuple-field known-shape typing
- strict unresolved focused fixtures

### Week 2: ADT Semantics Hardening (owner: 047 track)

Objective:
- deterministic constructor typing and module policy diagnostics.

File focus:
- compiler constructor typing and match-check paths
- module constructor boundary diagnostics

Gates:
- constructor arity/type determinism
- generic constructor field mismatch
- module constructor misuse diagnostics
- nested ADT pass/fail consistency

### Week 3: Strong Exhaustiveness Completion (owner: 050 track)

Objective:
- deterministic compile-time totality over supported domains.

File focus:
- general coverage analyzer + ADT integration in match checking/compilation.

Gates:
- Bool/list/sum-like pass/fail
- guarded wildcard fail behavior
- tuple conservative behavior with and without fallback
- nested ADT constructor-space non-exhaustive regressions

### Week 4: Parity + Sign-off

Objective:
- freeze diagnostics contract and publish release evidence.

Required commands:

```bash
cargo fmt --all -- --check
cargo check --all --all-features
cargo test --all --all-features purity_vm_jit_parity_snapshots
```

Required docs updates:
- `docs/internals/type_system_effects.md`
- `docs/proposals/043_pure_flux_checklist.md`
- `docs/proposals/000_index.md`

Stage-1 acceptance:
1. HM typed-path zero-fallback achieved.
2. ADT semantics deterministic for supported contract.
3. Exhaustiveness guarantees stable in supported domains.
4. VM/JIT parity green on curated matrix.
5. Docs aligned with implementation truth.

---

## 5. Stage 2 (Weeks 5-16): Post-0.0.4 Full Hardening

### Track A (Weeks 5-8): Currying + Placeholder (`052`)

Deliver:
1. parser/AST placeholder call-arg form
2. call normalization template/hole model
3. runtime partial callable representation
4. HM residual function typing + effect-row preservation
5. currying-specific diagnostics + parity fixtures

### Track B (Weeks 9-12): Traits/Typeclasses (`053`)

Deliver:
1. `trait`/`impl` syntax
2. coherence/orphan checks
3. HM predicate-carrying schemes
4. dictionary-passing lowering
5. baseline traits (`Eq`, `Ord`, `Show`) then minimal `Functor`

### Track C (Weeks 13-14): Typed Records (`048`)

Deliver:
1. typed record declarations/shape typing
2. field access/update/spread checks
3. diagnostics for unknown/missing/mismatched fields

### Track D (Weeks 15-16): Architecture/Perf Starter (`044`)

Deliver:
1. minimal pass-pipeline shell with behavior-preserving delegation
2. phase timing counters and baseline artifacts
3. first measurable low-risk compile-time optimization

Constraint:
- no semantic/diagnostic drift versus established suites.

### Track E (post-0.0.4): Lexer Performance/Architecture (`055`)

Deliver:
1. benchmark-first lexer optimization workflow
2. hot-path dispatch and allocation efficiency improvements
3. reader/state invariant hardening
4. parser-contract regression guardrails for token stream compatibility

---

## 6. Proposal Ownership Map

1. `051` owns HM fallback reduction and typed-path soundness.
2. `047` owns ADT semantic hardening.
3. `050` owns totality/exhaustiveness hardening.
4. `052`, `053`, `048` are post-0.0.4 feature tracks.
5. `044` is post-semantic-stability architecture/perf track.

---

## 7. Governance and Rollout Controls

1. `054` is the canonical sequencing doc for this roadmap.
2. Each stage transition requires:
   - green required gates,
   - intentional-only snapshot diffs,
   - updated evidence notes in internals/checklist/index docs.
3. `000_index.md` should update per stage (`gap -> partial -> have`) with evidence pointers.

---

## 8. Risks and Mitigations

1. Risk: diagnostics churn across tracks.  
   Mitigation: lock class boundaries and enforce snapshot review.

2. Risk: scope creep in post-0.0.4 features.  
   Mitigation: maintain stage boundaries and non-goals explicitly.

3. Risk: parity regressions while adding features.  
   Mitigation: parity fixture expansion per track before merge.

---

## 9. Explicit Assumptions and Defaults

1. 0.0.4 focuses on hardening, not broad syntax expansion.
2. Currying/typeclasses/records are critical but intentionally post-0.0.4.
3. VM/JIT parity snapshots are canonical regression guardrails.
4. Strict mode remains strongest unresolved-type enforcement boundary.
5. Architecture/perf refactor starts only after semantic tracks stabilize.
