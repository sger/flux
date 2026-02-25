# Proposal 042: Effect Rows and Constraint Solving for `with e`

**Status:** Draft  
**Date:** 2026-02-25  
**Depends on:** 032_type_system_with_effects.md

---

## 1. Motivation

Proposal 032 introduced algebraic effects and a practical `with e` model for higher-order functions.
Flux now supports useful effect-variable propagation, but it is still not full row polymorphism.

Current limitations:

- No first-class row terms (for example "IO plus e")
- No row subtraction/absence constraints at type level
- No principal row unification across all call sites
- Limited diagnostics when multiple `with e` constraints interact

This proposal defines a future, solver-based effect-row system.

---

## 2. Goals

1. Make `with e` a true row-polymorphic mechanism.
2. Support row extension and normalization (order-insensitive sets).
3. Support row constraints needed for handlers and effect-safe APIs.
4. Preserve Flux ergonomics and compatibility with existing `with` syntax.
5. Improve diagnostics for conflicting or unsatisfied effect constraints.

---

## 3. Non-Goals (for this proposal)

1. Full capability/security effect systems.
2. Changes to runtime effect representation.
3. Mandatory `fn main` policy changes (covered by Phase 4 hybrid policy already chosen).

---

## 4. Surface Model

Flux keeps user-facing effect clauses in `with ...` form, but gains row semantics:

```flux
fn map<T, U>(xs: List<T>, f: (T) -> U with e) -> List<U> with e

fn log_map<T, U>(xs: List<T>, f: (T) -> U with e) -> List<U> with IO, e
```

Interpretation:

- `with IO, e` means row extension (`IO` union `e`).
- `with e` remains an open effect row variable.
- Duplicate effects are idempotent (`IO, IO, e == IO, e`).

---

## 5. Type-System Model

Represent effects as rows:

- `Empty` row
- `Label(name, tail)` row nodes
- `Var(e)` row variables

Examples:

- `with IO` => `Label(IO, Empty)`
- `with IO, Time` => `Label(IO, Label(Time, Empty))`
- `with IO, e` => `Label(IO, Var(e))`

Normalization rules:

1. Effect labels are unique.
2. Label order does not matter semantically.
3. Equivalent rows normalize to a canonical form for comparison.

---

## 6. Constraints

Solver constraints introduced by typing:

1. **Row equality**: `r1 == r2`
2. **Row contains**: `label in r`
3. **Row extension**: `r_out = label + r_in`
4. **Handled subtraction (derived)**: handling effect `E` in expression `x` yields `row(x) - E`

Absence constraints are optional in v1 of this proposal. They can be added later if needed.

---

## 7. Solving Strategy

Constraint solving proceeds in phases:

1. Collect effect constraints during type/effect checking.
2. Unify row variables with occurs checks.
3. Normalize row terms after each unification step.
4. Resolve required ambient effects from solved rows.
5. Report unsatisfied constraints with source spans.

Expected properties:

- Principal effect rows for polymorphic functions.
- Deterministic diagnostics.
- No dependence on runtime fallback for static row obligations.

---

## 8. Examples

### 8.1 Row Extension

```flux
fn with_logging<T>(f: () -> T with e) -> T with IO, e {
    print("start")
    let x = f()
    print("done")
    x
}
```

### 8.2 Handler Subtraction

```flux
fn run_console() -> Int with Console {
    perform Console.print("hi")
    1
}

fn pure_run() -> Int {
    run_console() handle Console {
        print(resume, _msg) -> resume(())
    }
}
```

`pure_run` is accepted because `Console` is discharged by the handler.

### 8.3 Polymorphic Chain

```flux
fn apply_once(f: (Int) -> Int with e, x: Int) -> Int with e { f(x) }
fn wrap(f: (Int) -> Int with e, x: Int) -> Int with e { apply_once(f, x) }
```

If `f` resolves to `with IO`, `wrap` resolves to `with IO`.

---

## 9. Diagnostics

New/updated diagnostics should include:

1. Missing required effect after row solving (existing `E400` can be reused with better explanation).
2. Unsolved effect variable due to ambiguous constraints.
3. Incompatible row constraints (for example impossible row equalities).

Diagnostic messages should show:

- the effect variable (`e`)
- inferred concrete effects
- where each constraint originated (call site and signature span)

---

## 10. Migration Plan

### Phase A: Internal Row IR

- Introduce row data structure and normalizer.
- Keep current surface syntax unchanged.

### Phase B: Equality + Extension Solver

- Solve `with e`, `with IO, e`, and chained propagation end-to-end.

### Phase C: Handler-Aware Solving

- Integrate `handle` subtraction directly in row constraints.

### Phase D: Diagnostics + Strict Mode Integration

- Improve messages and optionally tighten behavior under `--strict`.

---

## 11. Compatibility

This proposal is backward-compatible with existing `with` syntax.
Existing programs continue to parse unchanged.
Behavioral changes are limited to stricter and more precise compile-time effect reasoning.

---

## 12. Open Questions

1. Should Flux expose explicit row-tail syntax in user code, or keep only `with ...` sugar?
2. Should absence constraints be part of v1, or deferred?
3. How much row detail should appear in user diagnostics by default?
4. Should `--strict` require explicit effect annotations for public higher-order APIs?

