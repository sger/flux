- Feature Name: Any Fallback Reduction and Typed-Path Soundness
- Start Date: 2026-02-26
- Proposal PR: —
- Flux Issue: —
- Status: Implemented
- Completion Date: 2026-03-03 (Stage 1); Stage 2: 2026-03-03

# Proposal 0051: Any Fallback Reduction and Typed-Path Soundness

## Summary
[summary]: #summary

Reduce accidental unsoundness by replacing silent `Any` degradation with concrete type constraints or explicit unresolved diagnostics in high-value typed paths. Stage 1 targets strict mode and HM-known contexts. Stage 2 (non-strict module-qualified generic paths) is deferred.

## Motivation
[motivation]: #motivation

`Any` is a deliberate gradual escape hatch, but current behavior contains fallback sites that are effectively accidental: HM has concrete type evidence at a call site, yet the constraint is silently widened to `Any` rather than flagging a mismatch. Stage 1 eliminates this in the highest-value paths (strict mode and typed-known contexts) without touching intentional gradual behavior.

---

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Goals

1. Inventory and classify all `Any` fallback hotspots.
2. Define clear allow/disallow policy for fallback.
3. Tighten typed/HM-known contexts first.
4. Keep intentional gradual behavior explicit and documented.
5. Improve deterministic diagnostics where fallback is blocked.

### Non-Goals

1. Eliminate `Any` from Flux entirely.
2. Force fully static typing for all programs.
3. Introduce new syntax for gradual boundaries.
4. Redesign runtime boundary checking model.

---

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### 5.1 Allowed fallback policy

Fallback to `Any` is **permitted** when:

1. Source value is explicitly dynamic/unknown — e.g. an unannotated binding whose HM type is `Any` by construction.
2. Type information is truly unavailable after HM + contract resolution — free type variables remain after inference (i.e. `is_hm_type_resolved()` returns `false`).
3. Context is not strict mode — `--strict` is not active, so unresolved boundaries are silently widened.
4. `join_types()` branch — when two arms produce incompatible but non-concrete types (free vars or `Any` on either side), widening to `Any` avoids false positives on partially-typed code.

### 5.2 Disallowed fallback policy

Fallback to `Any` is **blocked** (Stage 1) when:

1. HM has concrete type evidence at the expression site — `hm_expr_type_strict_path()` returns `Known(InferType)` and the type satisfies `is_hm_type_resolved()`.
2. Both sides of a unification are fully concrete and `Any`-free — `unify_with_context()` emits `E300` instead of widening.
3. Strict boundary is hit but the boundary type is unresolved — `maybe_unresolved()` emits `E425` in strict mode rather than silently accepting `Any`.

### 5.3 When disallowed fallback is hit

1. Emit concrete mismatch diagnostics (`E300`) when types conflict and both sides are concrete.
2. Emit unresolved-boundary diagnostics (`E425`) only when type is genuinely unresolved after all evidence paths are exhausted (strict mode only).

### 6. Hotspot Inventory Matrix (Stage 1)

| Hotspot | Location | Current (pre-0051) | Stage 1 behavior | Severity | Status |
|---------|----------|-------------------|------------------|----------|--------|
| HM concrete mismatch widened to Any | `src/types/unification.rs` `unify_with_context()` | Silent `Any` widening when one side is `Any` | Emits E300 when both sides concrete and `Any`-free | High | ✅ Done |
| Strict path unresolved boundary | `src/bytecode/compiler/hm_expr_typer.rs` `maybe_unresolved()` | Silent acceptance | Emits E425 in `--strict` mode | High | ✅ Done |
| Typed-path zero-fallback gate | `src/bytecode/compiler/hm_expr_typer.rs` `hm_expr_type_strict_path()` | Any propagation through typed path | Returns `Unresolved` unless `is_hm_type_resolved()` passes | High | ✅ Done |
| `join_types()` branch mismatch | `src/types/unification.rs` `join_types()` | Widening to `Any` for mixed branches | Retained: widening still occurs when at least one side is non-concrete | Medium | ✅ Retained (intentional) |
| Non-strict module-qualified generic calls | `src/bytecode/compiler/expression.rs` `check_static_contract_call()` line 748 | Unchecked propagation (silent `continue`) when `convert_type_expr` returns `None` | HM-instantiated function type used as fallback; emits E300 via `call_arg_type_mismatch` when resolved params disagree with resolved argument | Medium | ✅ Done (Stage 2) |
| HOF element types (`map`/`filter` etc.) | `src/runtime/base/helpers.rs` HOF signatures | `Any` element types in signatures | `Any` retained pending proposal 0053 (traits) | Low | 📋 Deferred (0053) |
| Overloaded builtins (`len`/`abs`/`min`/`max`) | `src/runtime/base/helpers.rs` | `Any -> Any` signatures | `Any` retained pending proposal 0053 (type classes) | Low | 📋 Deferred (0053) |

### Core implementation anchors

| Gate | File | Function | Semantics |
|------|------|----------|-----------|
| Zero-fallback gate | `src/bytecode/compiler/hm_expr_typer.rs` | `is_hm_type_resolved(ty)` | `free_vars().is_empty() && !contains_any()` — both conditions required |
| Strict-path query | `src/bytecode/compiler/hm_expr_typer.rs` | `hm_expr_type_strict_path(expr, env)` | Returns `Known(ty)` only when resolved; `Unresolved` otherwise |
| Concrete mismatch | `src/types/unification.rs` | `unify_with_context(t1, t2, ctx)` | E300 emitted only when both types satisfy `is_hm_type_resolved()` |
| Strict unresolved | `src/bytecode/compiler/hm_expr_typer.rs` | `maybe_unresolved(ty, span, strict)` | E425 emitted in strict mode when `!is_hm_type_resolved(ty)` |
| Stage 2 HM fallback | `src/bytecode/compiler/expression.rs` | `check_static_contract_call()` | When `convert_type_expr` returns `None`, extracts HM-instantiated `Fun(params, ..)` type of the function; emits `call_arg_type_mismatch` (E300) when resolved param and resolved argument types disagree |
| Stage 2 deduplication | `src/bytecode/compiler/hm_expr_typer.rs` | `has_concrete_e300_for_expression(expr)` (now `pub(super)`) | Guards Stage 2 — skips if HM already emitted E300 for the argument span |

### Test coverage

- 40+ E425 cases in `tests/compiler_rules_tests.rs` validating strict-path behavior
- E300 concrete-only guard tests in `tests/type_inference_tests.rs`
- `examples/type_system/96_hm_stage2_generic_module_call_ok.flx` — passing: consistent generic calls, no error
- `examples/type_system/failing/206_hm_stage2_generic_module_arg_mismatch.flx` — failing: `prepend<T>("hello", [|1,2,3|])` → E300 arg #2 `Array<Int>` vs `Array<String>`
- VM/JIT diagnostic parity verified for both fixtures

---

## Drawbacks
[drawbacks]: #drawbacks

### Risks and Mitigations

1. Risk: breaking existing gradual code unexpectedly.
   - Mitigation: staged rollout; allowed-fallback table preserved and documented. Non-strict mode unaffected.
2. Risk: increased diagnostic volume.
   - Mitigation: E300/E425 split keeps mismatch vs unresolved diagnostics distinct and deterministic.
3. Risk: implementation drift across HM and compiler validators.
   - Mitigation: `is_hm_type_resolved()` is the single canonical gate used by all three sites above.

---

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. Using `is_hm_type_resolved()` as a single gate keeps the allow/disallow boundary mechanically verifiable.
2. Splitting E300 (concrete mismatch) from E425 (unresolved boundary) avoids conflating two distinct failure modes.
3. Deferring Stage 2 (non-strict paths) to a follow-up avoids touching intentional gradual code before proposal 0053 provides type classes for overloaded builtins.

---

## Prior art
[prior-art]: #prior-art

- TypeScript's `strict` mode progressively tightens inference without removing gradual escape hatches.
- Typed Racket's blame system distinguishes typed/untyped boundary failures from internal mismatches.

---

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- HOF element type tightening — tracked in proposal 0053 (traits/type classes).

---

## Future possibilities
[future-possibilities]: #future-possibilities

- Stage 2: block Any-fallback in non-strict module-qualified generic call paths once 0053 provides overloaded builtin signatures.
- Hotspot matrix ownership: add regression fixture for each new blocked site as it is hardened.

---

## Completion Notes

**Stage 1 deliverable:** Disallowed Any fallback blocked in strict mode and HM-known typed contexts. Three implementation anchors ship together as an atomic gate: `is_hm_type_resolved()`, `hm_expr_type_strict_path()`, and the `unify_with_context()` concrete-only E300 guard.

**Stage 2 deliverable:** Non-strict module-qualified generic call paths now use HM's call-site-instantiated function type as a fallback when `convert_type_expr` returns `None` (e.g. generic param `T`, user ADT types, `Array<T>` with generic element). When HM resolves the function type and argument type concretely and they conflict, E300 is emitted via `call_arg_type_mismatch`. Deduplication via `has_concrete_e300_for_expression` prevents double-emitting when HM already flagged the same argument span.

**Deferred to 0053:** HOF element types (`map`/`filter`/`fold` etc.) and overloaded builtins (`len`, `abs`, `min`, `max`, `sum`, `product`, `contains`, `concat`, `reverse`) remain `Any`-typed in HOF signatures until type classes land.

**Test evidence:** 40+ E425 strict-path cases in `compiler_rules_tests.rs`; E300 concrete guard in `type_inference_tests.rs`; fixture matrix in `examples/type_system/failing/`.
