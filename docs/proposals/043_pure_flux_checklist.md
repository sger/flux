# 043 — Pure Flux Checklist

## 1. Goal

Define a practical checklist to make Flux **pure-by-default** while preserving explicit, typed effects at boundaries.

This document is an execution checklist mapped to:
- `032_type_system_with_effects.md`
- `042_effect_rows_and_constraints.md`

It is intentionally implementation-focused (pass/fail criteria), not a new language design.

---

## 2. Definition of "Pure" (for Flux)

A Flux program is considered "pure by default" when:
- A function without effect requirements cannot perform side effects.
- Side effects are only allowed through explicit effect annotations/handling.
- Effect handling coverage is guaranteed statically for supported cases.
- VM and JIT enforce identical effect semantics and diagnostics.

---

## 3. Priority Order

1. Effect soundness and static enforcement (Phase 4 hardening from 032)
2. Effect polymorphism completion (`with e`) and row constraints (042)
3. Strict boundary discipline for APIs (`--strict`)
4. Backend parity and regression matrix

---

## 4. Checklist

### A. Effect Soundness (032 / Phase 4)

- [x] A1. All direct effectful builtins/primops are statically effect-checked in function bodies.
- [x] A2. Effect propagation through call chains is complete (typed, inferred, generic, module-qualified calls).
- [x] A3. Pure contexts reject effectful operations consistently.
- [x] A4. Top-level effectful execution is always rejected outside policy-approved entry boundary.

Pass criteria:
- Every `examples/type_system/failing/*effect*` fixture fails with compile-time diagnostics (not runtime fallback).
- No known path where `print/read_file/now/...` executes in a pure function without error.

A4 verification matrix:

| Context | Expected |
|---|---|
| Pure top-level declarations/expressions only (no `main`) | Allow |
| Effectful top-level expression | Reject (`E413` + `E414`) |
| Effectful expression inside `fn main() with ...` | Allow |

Status:
- Completed with dedicated regression fixtures covering direct builtin checks, module-qualified/generic propagation, alias edge cases (`let p = print` / `let n = now_ms`), pure-context rejection matrix, and top-level policy matrix.

---

### B. Handle/Perform Static Correctness (032 / Phase 4)

- [x] B1. `perform Effect.op(...)` validates declared effect and operation at compile time.
- [x] B2. `handle Effect { ... }` validates unknown/missing operations statically.
- [x] B3. Handlers statically discharge required effects in call chains where modeled.
- [x] B4. Runtime unhandled-effect error remains fallback only.

Pass criteria:
- Existing handle/perform failing fixtures fail at compile-time.
- Valid handler fixtures compile and execute on VM and JIT with matching behavior.

B verification matrix:

| Context | Expected |
|---|---|
| `perform` unknown effect/op | Reject (`E403`/`E404`) |
| `handle` unknown effect | Reject (`E405`) |
| `handle` unknown/missing operations | Reject (`E401`/`E402`) |
| Custom effect reaches `main` undischarged | Reject (`E406`) |
| Custom effect discharged by `handle` before root return | Allow |

Status:
- Completed with compile-time checks for unknown handle effects and root-boundary undischarged custom effects, plus VM/JIT fixture parity.

---

### C. Effect Polymorphism Completion (042)

- [x] C1. `with e` resolution is solver-level (not just syntax + ad hoc propagation).
- [x] C2. Higher-order chains preserve effect variables across wrappers/composition.
- [x] C3. Row operations are supported for constrained polymorphism:
  - extension (`e + IO`)
  - subtraction/discharge (`e - Console`)
  - subset constraints (`e1 ⊆ e2`) or equivalent internal model
- [x] C4. Diagnostics explain unresolved/ambiguous effect variables clearly.

Pass criteria:
- Add fixture matrix for nested HOF + partial handling + mixed effects.
- No false-positive "missing effect" in valid polymorphic programs from 042 examples.

Status:
- Completed with row-constraint solving for call-site effect-variable resolution, nested HOF propagation fixtures, partial-handle discharge coverage, mixed `IO`/`Time` row-extension coverage, explicit row surface syntax (`with ... + ... - ...`), and row-specific diagnostics for unresolved/ambiguous/constraint failures.

---

### D. Entry-Point Policy and Purity Boundary (032 + team decision)

- [x] D1. Enforce chosen `main` policy exactly (hybrid policy currently selected by team).
- [x] D2. Validate `main` signature/effect-root behavior consistently.
- [x] D3. Keep diagnostic messages and hints stable across VM/JIT.

Pass criteria:
- Programs violating chosen policy always fail with deterministic diagnostics.
- Programs following policy succeed without requiring runtime effect fallback.

Status:
- Completed with dedicated entry-point fixtures for duplicate/invalid `main` signatures, top-level effect rejection with and without `main`, strict missing-main enforcement, and root-discharge gating to avoid redundant `E406` cascades when `main` signature is invalid.

D verification matrix:

| Context | Expected |
|---|---|
| Duplicate top-level `fn main` | Reject (`E410`) |
| `fn main` with parameters | Reject (`E411`) |
| `fn main` with non-`Unit` return | Reject (`E412`) |
| Effectful top-level, no `main` | Reject (`E413`, `E414`) |
| Effectful top-level, valid `main` present | Reject (`E413` only) |
| Custom effect escapes valid `main` boundary | Reject (`E406`) |
| Strict mode without `main` | Reject (`E415`) |

---

### E. Strict Mode as Purity Profile (032 / Phase 6)

- [ ] E1. `--strict` enforces annotation discipline for exported/public APIs.
- [ ] E2. `Any` usage under `--strict` follows explicit policy:
  - warning-only (current), or
  - error in pure profile (future tightening)
- [ ] E3. Strict-mode cache identity is isolated from non-strict builds.
- [ ] E4. Strict checks apply uniformly to run/test/bytecode entry paths.

Pass criteria:
- Strict fixtures pass/fail exactly as documented.
- Same file compiled in strict vs non-strict never reuses incompatible cache artifacts.

---

### F. Public API Boundary Semantics (follow-up after strict baseline)

- [ ] F1. Replace naming-convention heuristics with explicit visibility (`pub`) or equivalent.
- [ ] F2. Strict checks target real exported surface only.

Pass criteria:
- `pub` boundary fixtures exist for pass/fail cases.
- Non-exported helper functions are not over-constrained by strict API rules.

---

### G. Backend Parity and Regression Coverage

- [ ] G1. VM and JIT produce equivalent compile-time diagnostics for effect/type boundary failures.
- [ ] G2. Shared fixture matrix covers:
  - direct effects
  - call propagation
  - handle discharge
  - effect polymorphism
  - strict policy
- [ ] G3. Snapshot tests pin diagnostic code + title + primary label for key cases.

Pass criteria:
- No known VM/JIT discrepancy in effect diagnostics for fixture suite.
- CI runs both backends on purity-critical fixtures.

---

## 5. Exit Criteria ("Flux is pure-by-default enough")

Flux reaches this milestone when:
- Sections A/B are complete.
- Section C has solver-level `with e` plus minimal row constraints from 042.
- Section D policy is finalized and enforced.
- Section E strict mode is stable for API boundaries.
- Section G parity suite is green on both backends.

At that point, Flux is pure-by-default with explicit effect boundaries and predictable enforcement.

---

## 6. Out of Scope

- Full theorem-prover-level effect reasoning.
- Advanced capability security model.
- Concurrency effect system design details (covered by separate concurrency proposals).
