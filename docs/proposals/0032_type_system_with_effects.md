- Feature Name: Type System with Algebraic Effects for Flux
- Start Date: 2026-02-17
- Proposal PR: pending (feature/type-system merge PR)
- Flux Issue: pending (type-system merge-readiness tracker, March 1, 2026)

# Proposal 0032: Type System with Algebraic Effects for Flux

## Summary
[summary]: #summary

Define Flux's typed core (HM-style inference + boundary annotations + algebraic effects) while preserving gradual migration from untyped code.

## Motivation
[motivation]: #motivation

Flux began as dynamically typed, so many type/effect errors surfaced at runtime. This proposal defines compile-time semantics that improve correctness and diagnostics without breaking gradual adoption.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 2. Design Principles

| Principle | Rationale |
|---|---|
| Gradual | Untyped code can continue to run during migration. |
| Inferred | Local inference minimizes annotation burden. |
| Effect-aware | Effects are modeled in function types, not as runtime-only checks. |
| Boundary-first | Public contracts and typed boundaries carry stronger guarantees. |
| Deterministic diagnostics | Stable code/title/primary-label shape for regressions. |

### 4.1 Tuple Syntax

```flux
let point: (Int, Int) = (10, 20)
let entry: (String, Int) = ("score", 100)
```

### 7. The `Any` Type: Semantics

`Any` is the gradual boundary type. It is not a semantic supertype; crossings between typed and untyped paths may require runtime checks.

### Phase 1: Type Syntax (Parser)

Status: implemented.

- Type annotations on `let`, `fn`, lambda params.
- Generic parameters on functions and data declarations.
- Function effect clauses (`with ...`).
- Effect declarations and `handle` syntax.
- Entry-point parsing and typed forms used in type-system fixtures.

### 18. Syntax Summary

```flux
let x: T = expr
fn f(a: T) -> U with IO { ... }
fn id<T>(x: T) -> T { x }
\(x: T) -> x

effect Console {
  print: String -> Unit
}

expr handle Console {
  print(resume, msg) -> resume(())
}
```

### 21. Non-Goals (Explicitly Out of Scope)

- Dependent types.
- Trait/typeclass system (tracked separately).
- New runtime representation for effect rows.
- Advanced guard-exhaustiveness proof beyond conservative policy.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Consolidated technical points

- Implementation status: landed and used by the current type/effect diagnostics pipeline.
- HM inference + compiler validation boundaries are explicit and test-backed.
- Effect checking is compile-time first, with runtime boundary checks where `Any` is involved.

### Detailed specification (migrated legacy content)

Normative behavior is captured by this proposal plus fixtures in `examples/type_system/` and `examples/type_system/failing/`.

### Historical notes

- Legacy content was normalized into template form on this branch.

## Drawbacks
[drawbacks]: #drawbacks

- Migration from legacy dynamic code may surface more compile-time errors initially.
- Additional policy/docs maintenance is required to keep semantics and diagnostics aligned.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- Template normalization improves reviewability.
- Deterministic diagnostics + fixtures are preferred over ad hoc behavior.

## Prior art
[prior-art]: #prior-art

| Language | Approach | Flux takeaway |
|---|---|---|
| Koka | Algebraic effects + HM | Effect-aware typing model |
| OCaml 5 | Practical effect runtime | Handler ergonomics |
| TypeScript | Gradual typing | Boundary discipline |
| Rust | Generic ergonomics | Type parameter syntax |

## Unresolved questions
[unresolved-questions]: #unresolved-questions

### 19. Resolution Log (March 1, 2026)

1. **Structural vs nominal ADTs**
   - Outcome: **Rejected for v0.0.4**.
   - Decision: ADTs remain nominal in this milestone.
2. **Type classes / traits in this proposal**
   - Outcome: **Deferred (linked follow-up proposal)**.
   - Follow-up: `docs/proposals/0053_traits_and_typeclasses.md`.
3. **Recursive types support**
   - Outcome: **Accepted now**.
   - Decision: keep named recursive ADT support with existing occurs-check protections.
4. **Effect-handler compilation strategy expansion**
   - Outcome: **Deferred (linked follow-up proposal)**.
   - Follow-up: `docs/proposals/0063_true_fp_completion_program.md`.
5. **JIT specialization from inferred types**
   - Outcome: **Deferred (linked follow-up proposal)**.
   - Follow-up: `docs/proposals/0062_performance_stabilization_program.md`.
6. **`[]` runtime representation**
   - Outcome: **Accepted now**.
   - Decision: keep `Value::EmptyList` representation for this milestone.

## Future possibilities
[future-possibilities]: #future-possibilities

- Extend trait/typeclass capabilities in dedicated follow-up work.
- Expand handler compilation strategy only with VM/JIT parity safeguards.
