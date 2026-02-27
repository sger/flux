# Proposal 053: Traits and Typeclasses (Eq/Ord/Show/Functor)

**Status:** Draft  
**Date:** 2026-02-26  
**Depends on:** `032_type_system_with_effects.md`, `043_pure_flux_checklist.md`, `047_adt_semantics_deepening.md`, `052_auto_currying_and_partial_application.md`, `027_language_syntax_specification.md`

---

## 1. Summary

Add principled ad-hoc polymorphism to Flux using traits/typeclasses, with a staged but execution-grade design:

1. Phase A: `Eq`, `Ord`, `Show` for principled constrained polymorphism.
2. Phase B: `Functor` with minimal kind support for unary type constructors.

This proposal replaces untyped duck-typing patterns in generic library APIs with compile-time checked constraints.
Roadmap note: this is a stage-2 (post-0.0.4) track under
`054_0_0_4_hm_adt_exhaustiveness_critical_path.md`.

---

## 2. Motivation and Current Gap

Flux currently lacks trait constraints, so reusable polymorphic APIs are either:

1. specialized per concrete type, or
2. dynamic/`Any`-leaning and unsound at boundaries.

Current pain points:

1. No canonical `Eq`/`Ord`/`Show` abstraction.
2. `sort`/`sort_by` cannot be constrained by principled ordering laws.
3. Container-generic mapping abstractions are not expressible (`Functor`).
4. Module APIs leak duck-typed behavior instead of explicit contracts.

---

## 3. Goals

1. Introduce trait declarations, implementations, and constrained generic functions.
2. Keep diagnostics deterministic and actionable.
3. Integrate trait constraints with HM inference without regressing effect/type stability.
4. Preserve VM/JIT compile-time diagnostics parity.
5. Provide migration path from untyped helper patterns to trait-based APIs.

---

## 4. Non-Goals

1. Full Haskell-style advanced typeclass ecosystem in one release.
2. GADTs, higher-rank polymorphism, or dependent typing.
3. Rewrite of runtime value model.
4. Trait solver completeness beyond declared scope.

---

## 5. Locked Language Decisions

1. Traits are nominal and globally coherent per compilation unit.
2. One impl per `(Trait, ConcreteType)` pair is allowed in resolution scope.
3. Orphan rule is enforced from day one:
   - an impl is legal only if trait or target type is defined in the current module/package root.
4. Method dispatch is static dictionary passing (no runtime reflection).
5. Phase A ships `Eq`, `Ord`, `Show`; Phase B adds `Functor` with minimal kind layer.

---

## 6. Surface Syntax (Normative)

### 6.1 Trait declaration

```flux
trait Eq<T> {
  fn eq(a: T, b: T) -> Bool
}

trait Ord<T>: Eq<T> {
  fn compare(a: T, b: T) -> Ordering
}

trait Show<T> {
  fn show(x: T) -> String
}
```

### 6.2 Impl declaration

```flux
impl Eq<Int> for Int {
  fn eq(a: Int, b: Int) -> Bool { a == b }
}

impl Show<Int> for Int {
  fn show(x: Int) -> String { to_string(x) }
}
```

### 6.3 Constrained generic function

```flux
fn dedup<T: Eq<T>>(xs: List<T>) -> List<T> { ... }
fn sort_default<T: Ord<T>>(xs: List<T>) -> List<T> { ... }
fn print_all<T: Show<T>>(xs: List<T>) -> Unit with IO { ... }
```

### 6.4 Phase B syntax (Functor)

```flux
trait Functor<F<_>> {
  fn fmap<A, B>(f: (A) -> B, fa: F<A>) -> F<B>
}
```

Minimal kind `* -> *` support is introduced only for trait constructor parameters in this phase.

---

## 7. Type System/HM Integration

### 7.1 Constraint-carrying schemes

HM schemes are extended with trait predicates:

- `forall T. Eq<T> => (List<T>) -> Bool`

### 7.2 Inference flow

1. Infer base type as today.
2. Collect trait obligations from constrained signatures and method usage.
3. Solve obligations via impl environment.
4. Emit trait diagnostics when unsatisfied/ambiguous/conflicting.

### 7.3 Constraint solving bounds

1. No backtracking search explosion in Phase A.
2. Resolution uses direct impl lookup + superclass expansion.
3. Ambiguous unresolved trait var emits compile-time error (no `Any` fallback).

---

## 8. Effects Integration

1. Trait methods may carry effects:

```flux
trait Loggable<T> {
  fn log(x: T) -> Unit with IO
}
```

2. Calling constrained methods participates in normal effect propagation (`E400` family).
3. No special effect exceptions for traits.

---

## 9. Compiler and Runtime Model

### 9.1 Frontend additions

1. Parse `trait` and `impl` statements.
2. Track trait environment and impl environment per module + import scope.
3. Validate orphan/coherence rules pre-HM finalize.

### 9.2 Lowering strategy

Use dictionary passing:

1. Constrained function gets implicit dictionary parameters.
2. Call sites pass resolved dictionaries.
3. Method calls lower to dictionary field calls.

### 9.3 Runtime representation

A dictionary is a runtime value/constant containing callable entries for required methods.

No dynamic trait lookup by string; all dictionary wiring is compile-time determined.

---

## 10. Diagnostics Contract

Existing diagnostics remain unchanged for unrelated categories (`E300`, `E400`, `E055`).

Add trait-specific diagnostics (new registry codes):

1. missing impl for required trait bound,
2. conflicting impls in scope,
3. orphan impl violation,
4. ambiguous trait obligation,
5. illegal impl method signature mismatch vs trait declaration.

Each diagnostic must include:

1. code + stable title,
2. primary label on offending declaration/call,
3. clear fix hint (`add impl`, `import correct module`, `remove conflicting impl`, etc.).

---

## 11. Flux Examples (Pass/Fail)

### 11.1 Pass

```flux
trait Eq<T> {
  fn eq(a: T, b: T) -> Bool
}

impl Eq<Int> for Int {
  fn eq(a: Int, b: Int) -> Bool { a == b }
}

fn contains<T: Eq<T>>(xs: List<T>, x: T) -> Bool {
  match xs {
    [h | t] -> if eq(h, x) { true } else { contains(t, x) },
    _ -> false,
  }
}
```

```flux
trait Show<T> {
  fn show(x: T) -> String
}

impl Show<Int> for Int {
  fn show(x: Int) -> String { to_string(x) }
}

fn main() with IO {
  print(show(42))
}
```

```flux
trait Ord<T>: Eq<T> {
  fn compare(a: T, b: T) -> Ordering
}
```

### 11.2 Fail

```flux
fn f<T: Show<T>>(x: T) -> String { show(x) }
let out = f(1) // missing Show<Int> impl
```

```flux
impl Eq<Int> for Int { ... }
impl Eq<Int> for Int { ... } // conflicting impl
```

```flux
impl Eq<ExternalType> for ExternalType { ... } // orphan violation
```

---

## 12. Test and Fixture Plan

### 12.1 New fixtures

Add dedicated trait fixtures:

1. pass: constrained generics (`Eq`, `Ord`, `Show`),
2. fail: missing impl, conflicting impl, orphan violation, ambiguous bound,
3. Phase B pass/fail for `Functor` constructor constraints.

### 12.2 Compiler tests

1. parser tests for `trait`/`impl` syntax,
2. HM tests with predicate-carrying schemes,
3. dictionary-lowering tests,
4. diagnostics snapshots for trait errors.

### 12.3 Parity

Add representative failing trait fixtures to VM/JIT parity suite and snapshot tuple invariants (`code/title/primary label`).

---

## 13. Rollout Plan (Phased)

### Phase A1: Syntax + environments

1. parse trait/impl,
2. build trait and impl tables,
3. enforce orphan/coherence checks.

### Phase A2: HM + dictionary lowering

1. attach constraints to schemes,
2. resolve obligations,
3. lower constrained calls via dictionaries.

### Phase A3: Standard trait baseline

1. land `Eq`, `Ord`, `Show` examples and fixture set,
2. migrate selected stdlib-style APIs to constrained forms.

### Phase B: Functor

1. introduce minimal kind support `* -> *` for trait constructor parameter,
2. add `Functor` trait and `fmap` examples,
3. extend solver for constructor-level obligations in bounded scope.

### Phase C: Hardening

1. parity snapshots,
2. docs and guide updates,
3. regressions and diagnostics stability pass.

---

## 14. Risks and Mitigations

1. Risk: solver complexity/regressions.
   - Mitigation: staged constraint solver with explicit non-goals and bounded search.
2. Risk: coherence/orphan UX confusion.
   - Mitigation: clear diagnostics with concrete import/module guidance.
3. Risk: HM + effects interaction drift.
   - Mitigation: dedicated fixtures combining constraints and `with` effects.
4. Risk: Phase B (Functor) kinding scope creep.
   - Mitigation: isolate minimal kind support to trait constructor parameters only.

---

## 15. Acceptance Criteria

1. `Eq`, `Ord`, `Show` trait declarations and impls compile and dispatch correctly.
2. Constrained generic functions are type-checked and lowered deterministically.
3. Missing/conflicting/orphan/ambiguous trait cases produce stable diagnostics.
4. VM/JIT parity suite includes trait failure cases and remains green.
5. Existing type/effect strict suites continue passing.
6. Phase B `Functor` ships only with minimal kind support and explicit fixture coverage.

---

## 16. Explicit Assumptions and Defaults

1. Trait system is compile-time dictionary-based, not runtime reflection.
2. Coherence + orphan checks are enforced from initial rollout.
3. `Eq`/`Ord`/`Show` are mandatory Phase A baseline.
4. `Functor` requires minimal kind support and is Phase B in same proposal track.
5. No higher-rank polymorphism or full typeclass feature parity is targeted in this proposal.
