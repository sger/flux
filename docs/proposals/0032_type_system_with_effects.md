- Feature Name: Type System with Algebraic Effects for Flux
- Start Date: 2026-02-17
- Proposal PR: 
- Flux Issue: 

# Proposal 0032: Type System with Algebraic Effects for Flux

## Summary
[summary]: #summary

// Generics
fn f<T>(x: T) -> T { ... }
fn f<T, U>(x: T, y: U) -> (T, U) { ... }

## Motivation
[motivation]: #motivation

Flux is currently a dynamically typed language. All type errors are caught at runtime, which means: Flux is currently a dynamically typed language. All type errors are caught at runtime, which means:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 2. Design Principles

| Principle | Rationale |
|---|---|
| **Gradual** | Types are optional. Untyped code infers as `Any`. No breaking changes. |
| **Inferred** | Local types are inferred. Annotations required only at module boundaries. |
| **Functional-first** | Function types are first-class. Effects compose naturally. |
| **Minimal syntax** | Reuse existing keywords. Avoid ceremony. |
| **Effect-aware** | Effects are part of the type system, not bolted on. |

### 4.1 Tuple Syntax

```flux
// Construction
let point = (10, 20)
let record = ("Alice", 42, true)

// Type annotation
let point: (Int, Int) = (10, 20)
let entry: (String, Int) = ("score", 100)
```

### 7. The `Any` Type: Semantics

`Any` is the **dynamic type** — it is not a supertype of everything, but rather a boundary between typed and untyped code. Values crossing this boundary undergo runtime checks.

### Phase 1: Type Syntax (Parser)

Status: implemented.
- Add type annotation parsing to `let`, `fn`, and lambda expressions
- Parse generic parameters `<T, U>` on `fn` and `data` declarations
- Parse tuple types `(A, B)` and tuple expressions `(a, b)`
- Parse `type` alias declarations
- Parse `data` ADT declarations
- Parse `effect` declarations
- Parse `with` effect clauses on function signatures
- Parse `... handle Effect { ... }` expressions
- Parse `fn main` as program entry point
- Parse `[]` as empty list literal (distinct from `None`)
- **No semantic checking** — just parse and store in AST
- All existing programs continue to work

### 18. Syntax Summary

```
// Type annotations
let x: T = expr
fn f(a: T, b: U) -> V { ... }
fn f(a: T) -> V with E1, E2 { ... }
fn f(a: T) with E { ... }              // return type omitted (Unit)
\(x: T) -> expr

// Generics
fn f<T>(x: T) -> T { ... }
fn f<T, U>(x: T, y: U) -> (T, U) { ... }

// Tuples
let pair: (Int, String) = (42, "hello")
let (a, b) = pair
pair.0                                  // 42

// Empty list vs None
[]                                      // empty List<T>
None                                    // empty Option<T>

// Type aliases
type Name = Type
type Pred<T> = (T) -> Bool

// Algebraic data types
data TypeName<T> {
    Variant1(T),
    Variant2(T, T),
    Variant3,
}

// Effect declarations
effect EffectName<T> {
    operation: ArgType -> ReturnType
}

// Effect handlers
expr handle EffectName {
    operation(resume, args...) -> resume(value)
}

// Effect polymorphism
fn hof<T, U>(f: (T) -> U with e) -> U with e { ... }
fn hof_io<T, U>(f: (T) -> U with e) -> U with e + IO { ... }
fn hof_discharge<T, U>(f: (T) -> U with e + Console) -> U with e { ... }

// Entry point
fn main() with IO {
    ...
}
```

### 21. Non-Goals (Explicitly Out of Scope)

- **Dependent types** — too complex for a first type system
- **Linear/affine types** — Rc-based runtime doesn't benefit
- **Type classes / traits** — deferred to a separate proposal
- **Mutable references** — Flux is immutable-first; `State` effect covers mutation
- **Async/await** — could be modeled as an effect later, but not in this proposal
- **Records / named fields** — structs could be a future extension, tuples cover positional data for now
- **Advanced row-polymorphism ergonomics** — core row constraints are implemented; deeper/generalized row features remain future work (see 042)
- **Guard exhaustiveness reasoning** — guards are always treated as "may fail" in v1

### 2. Design Principles

### 4.1 Tuple Syntax

### 7. The `Any` Type: Semantics

### Phase 1: Type Syntax (Parser)

### 18. Syntax Summary

### 21. Non-Goals (Explicitly Out of Scope)

### 21. Non-Goals (Explicitly Out of Scope)

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Implementation Status:** This proposal is now implemented in Flux and serves as the canonical semantics narrative for the type/effect system. - **3.1 Primitive Types:** ```...
- **Implementation Status:** This proposal is now implemented in Flux and serves as the canonical semantics narrative for the type/effect system.
- **3.1 Primitive Types:** ``` Int // i64 Float // f64 Bool // true | false String // Rc<str> Unit // the empty tuple (), absence of meaningful value Never // bottom type, inhabited by no values (divergen...
- **3.2 Composite Types:** ``` Option<T> // Some(T) | None Either<L, R> // Left(L) | Right(R) List<T> // Persistent cons list Array<T> // Dense array Map<K, V> // Persistent HAMT map (A, B) // Tuple of tw...
- **3.3 Function Types:** `->` is used consistently for all function-like things (named functions, lambdas, function types). It binds tighter than `with`: ``` fn f(x: Int) -> Int with IO // means: return...
- **3.4 Generics:** Generic type parameters use angle brackets on type and function declarations: ```flux fn identity<T>(x: T) -> T { x }

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 21. Non-Goals (Explicitly Out of Scope)

- **Dependent types** — too complex for a first type system
- **Linear/affine types** — Rc-based runtime doesn't benefit
- **Type classes / traits** — deferred to a separate proposal
- **Mutable references** — Flux is immutable-first; `State` effect covers mutation
- **Async/await** — could be modeled as an effect later, but not in this proposal
- **Records / named fields** — structs could be a future extension, tuples cover positional data for now
- **Advanced row-polymorphism ergonomics** — core row constraints are implemented; deeper/generalized row features remain future work (see 042)
- **Guard exhaustiveness reasoning** — guards are always treated as "may fail" in v1

### 21. Non-Goals (Explicitly Out of Scope)

### 21. Non-Goals (Explicitly Out of Scope)

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### 20. Prior Art

| Language | Approach | What Flux borrows |
|---|---|---|
| **Koka** | Algebraic effects + Hindley-Milner | Effect syntax, handler semantics, `Never` for divergence |
| **OCaml 5** | Algebraic effects (runtime) | Practical handler design, performance model |
| **Eff** | First-class effects | Clean effect declarations |
| **TypeScript** | Gradual typing | `Any` with runtime boundary checks, migration strategy |
| **Elm** | ML-style + effects as architecture | Effect as explicit boundary, `fn main` as entry point |
| **Unison** | Algebraic effects + content-addressed | Ability syntax inspiration |
| **Rust** | Generics + traits + `!` (never) | Generic syntax `<T>`, `Never` type, monomorphization |

### 20. Prior Art

## Unresolved questions
[unresolved-questions]: #unresolved-questions

### 19. Open Questions

1. **Structural vs. nominal ADTs?** — Nominal (as proposed) is simpler and matches Rust/Haskell conventions. Structural typing could be added later for records.

2. **Type classes / traits?** — Not in this proposal. A future proposal could add `trait Eq { fn eq(a, b) -> Bool }` for ad-hoc polymorphism. For now, operator overloading remains dynamic.

3. **Recursive types?** — Needed for tree-like ADTs (`Tree<T>`). Requires careful handling in the type checker to avoid infinite loops.

4. **Effect handler compilation strategy?** — Options include CPS transform, multi-prompt delimited continuations, or evidence passing. Each has different performance tradeoffs. The VM may need new opcodes (`OpResume`, `OpHandle`, `OpPerform`).

5. **Interaction with JIT?** — Type information could feed into JIT specialization. Monomorphized hot paths could skip type checks entirely.

6. **`[]` runtime representation?** — Should `[]` be `Value::EmptyList` (current) or a new sentinel? Current `EmptyList` works fine if we just change the syntax from `None` to `[]` in list contexts.

### 19. Open Questions

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
