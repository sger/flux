- Feature Name: Traits and Typeclasses (Eq/Ord/Show/Functor)
- Start Date: 2026-02-26
- Status: **Superseded by Proposal 0145 (Type Classes)**
- Superseded Date: 2026-04-07
- Proposal PR: 
- Flux Issue: 

> **Note:** This proposal has been superseded by [Proposal 0145](../0145_type_classes.md),
> which implements Haskell-style type classes with `class`/`instance` syntax, ClassEnv
> validation, and runtime dispatch. Proposal 0145 covers the same goals (Eq, Ord, Show,
> constrained polymorphism) with a concrete implementation that is partially landed.
> See also [Proposal 0123](../0123_full_static_typing.md) for the broader static typing
> roadmap that 0145 is part of.

# Proposal 0053: Traits and Typeclasses (Eq/Ord/Show/Functor)

## Summary
[summary]: #summary

Add principled ad-hoc polymorphism to Flux using traits/typeclasses, with a staged but execution-grade design: Add principled ad-hoc polymorphism to Flux using traits/typeclasses, with a staged but execution-grade design:

## Motivation
[motivation]: #motivation

Flux currently lacks trait constraints, so reusable polymorphic APIs are either: Flux currently lacks trait constraints, so reusable polymorphic APIs are either:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Introduce trait declarations, implementations, and constrained generic functions.
2. Keep diagnostics deterministic and actionable.
3. Integrate trait constraints with HM inference without regressing effect/type stability.
4. Preserve VM/JIT compile-time diagnostics parity.
5. Provide migration path from untyped helper patterns to trait-based APIs.

### 4. Non-Goals

1. Full Haskell-style advanced typeclass ecosystem in one release.
2. GADTs, higher-rank polymorphism, or dependent typing.
3. Rewrite of runtime value model.
4. Trait solver completeness beyond declared scope.

### 6.4 Phase B syntax (Functor)

```flux
trait Functor<F<_>> {
  fn fmap<A, B>(f: (A) -> B, fa: F<A>) -> F<B>
}
```

Minimal kind `* -> *` support is introduced only for trait constructor parameters in this phase.

### Phase A1: Syntax + environments

1. parse trait/impl,
2. build trait and impl tables,
3. enforce orphan/coherence checks.

### 3. Goals

### 4. Non-Goals

### 6.4 Phase B syntax (Functor)

### Phase A1: Syntax + environments

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **6.1 Trait declaration:** trait Ord<T>: Eq<T> { fn compare(a: T, b: T) -> Ordering } - **6.2 Impl declaration:** impl Show<Int> for Int { fn show(x: Int) -> String { to_strin...
- **6.1 Trait declaration:** trait Ord<T>: Eq<T> { fn compare(a: T, b: T) -> Ordering }
- **6.2 Impl declaration:** impl Show<Int> for Int { fn show(x: Int) -> String { to_string(x) } } ```
- **6.3 Constrained generic function:** ```flux fn dedup<T: Eq<T>>(xs: List<T>) -> List<T> { ... } fn sort_default<T: Ord<T>>(xs: List<T>) -> List<T> { ... } fn print_all<T: Show<T>>(xs: List<T>) -> Unit with IO { ......
- **7.1 Constraint-carrying schemes:** HM schemes are extended with trait predicates: - `forall T. Eq<T> => (List<T>) -> Bool`
- **7.2 Inference flow:** 1. Infer base type as today. 2. Collect trait obligations from constrained signatures and method usage. 3. Solve obligations via impl environment. 4. Emit trait diagnostics when...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. Full Haskell-style advanced typeclass ecosystem in one release.
2. GADTs, higher-rank polymorphism, or dependent typing.
3. Rewrite of runtime value model.
4. Trait solver completeness beyond declared scope.

### 4. Non-Goals

### 14. Risks and Mitigations

1. Risk: solver complexity/regressions.
   - Mitigation: staged constraint solver with explicit non-goals and bounded search.
2. Risk: coherence/orphan UX confusion.
   - Mitigation: clear diagnostics with concrete import/module guidance.
3. Risk: HM + effects interaction drift.
   - Mitigation: dedicated fixtures combining constraints and `with` effects.
4. Risk: Phase B (Functor) kinding scope creep.
   - Mitigation: isolate minimal kind support to trait constructor parameters only.

### 4. Non-Goals

### 14. Risks and Mitigations

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### 5. Locked Language Decisions

1. Traits are nominal and globally coherent per compilation unit.
2. One impl per `(Trait, ConcreteType)` pair is allowed in resolution scope.
3. Orphan rule is enforced from day one:
   - an impl is legal only if trait or target type is defined in the current module/package root.
4. Method dispatch is static dictionary passing (no runtime reflection).
5. Phase A ships `Eq`, `Ord`, `Show`; Phase B adds `Functor` with minimal kind layer.

### 5. Locked Language Decisions

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
