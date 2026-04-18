# Chapter 9 — Type System Basics

> Examples: [`examples/guide_type_system/01_typed_let_and_fn.flx`](../../examples/guide_type_system/01_typed_let_and_fn.flx), [`02_annotations_vs_inference.flx`](../../examples/guide_type_system/02_annotations_vs_inference.flx), [`03_pure_function_boundaries.flx`](../../examples/guide_type_system/03_pure_function_boundaries.flx), [`13_any_is_rejected.flx`](../../examples/guide_type_system/13_any_is_rejected.flx)

## Learning Goals

- Read and write typed `let` bindings and function signatures.
- Understand what Hindley-Milner inference does and when it kicks in.
- Recognize compile-time type mismatches (`E300`) and when they occur.

## Overview

Flux uses Hindley-Milner (HM) type inference: type annotations are optional, and unannotated code is inferred where possible. The key properties are:

- **Annotated paths** — if you write a type, it is statically validated. Mismatches are compile-time errors (`E300`).
- **Inferred paths** — HM fills types for unannotated local expressions. The compiler reports concrete mismatches without false positives.
- **Strict typing direction** — the maintained language direction is that ordinary source programs should resolve to concrete static types rather than relying on fallback typing.
- **Incremental adoption** — you can still add annotations gradually, but the long-term model is static typing, not `Any` as a normal escape hatch.

Flux is a statically typed language:

- accepted programs are type-checked before execution
- source-language typing is decided in HM/Core, not deferred to backend runtime representation
- backend generic values such as tagged runtime values are representation details, not dynamic source typing

---

## Built-in Types

| Annotation | Description | Example literal |
|------------|-------------|-----------------|
| `Int` | 64-bit signed integer | `42`, `-7` |
| `Float` | 64-bit float | `3.14`, `-0.5` |
| `Bool` | Boolean | `true`, `false` |
| `String` | Immutable string | `"hello"` |
| `Unit` | No meaningful value (used for effectful functions) | — |
| `Option<T>` | Optional value | `Some(x)`, `None` |
| `Either<L, R>` | Two-variant sum | `Left(x)`, `Right(y)` |
| `List<T>` | Persistent cons list | `[1, 2, 3]` |
| `Array<T>` | Rc-backed mutable array | `[|1, 2, 3|]` |
| `Map<K, V>` | HAMT persistent hash map | `{"a": 1}` |
| `(A, B)` | Tuple (any arity) | `(1, "hi")` |
| `A -> B` | Function type | `\x -> x + 1` |
## Typed `let` Bindings

```flux
let total: Int = 40 + 2
let name: String = "Alice"
let flag: Bool = true
```

If the inferred type of the right-hand side doesn't match the annotation, Flux emits `E300`:

```flux
let n: Int = "oops"
// E300: type mismatch — expected Int, got String
```

---

## Typed Function Signatures

```flux
fn add(x: Int, y: Int) -> Int {
    x + y
}
```

- Parameter types are written after the parameter name with `:`.
- Return type is written after `->`.
- Both are optional — omit either and the HM engine fills them in.

```flux
// Fully typed
fn multiply(x: Int, y: Int) -> Int { x * y }

// Partially typed — return type inferred
fn greet(name: String) { "Hello, #{name}!" }

// Unannotated — all types inferred
fn double(x) { x * 2 }
```

---

## Hindley-Milner Inference

HM inference runs between PASS 1 (predeclaration) and PASS 2 (code generation). It uses Algorithm W:

1. Assign fresh type variables to unannotated parameters and `let` bindings.
2. Propagate constraints from how values are used.
3. Unify constrained variables — if two concrete types conflict, emit `E300`.
4. Generalize unconstrained variables into polymorphic `Scheme`s for `let`-bound functions.
5. Instantiate `Scheme`s fresh at each use site (the identity function can be used at both `Int` and `String` in the same scope).

### Let-polymorphism example

```flux
fn id(x) { x }

fn main() -> Unit {
    let n = id(1)       // id : Int -> Int at this use
    let s = id("hi")    // id : String -> String at this use
    print((n, s))
}
```

`id` is generalized to `forall t. t -> t`. Each call instantiates a fresh copy of `t`, so both usages type-check independently.

---

## `Any` and Legacy Surfaces

`Any` still exists in some internal and compatibility-oriented parts of the repo, but it is not part of the intended normal source-language model.

For ordinary Flux code, the goal is:

- expressions resolve to concrete static types
- mismatches are reported as compile-time diagnostics
- user-facing docs and examples do not treat `Any` as the normal typing story

If you encounter `Any` in diagnostics, internals docs, or older examples, treat it as legacy or migration residue rather than the recommended way to write Flux.

If you try to use `Any` as a source annotation, Flux rejects it:

```flux
fn bad(x: Any) -> Int {
    x
}
```

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/13_any_is_rejected.flx
```

Expected result:

- compile failure with `E423`
- `Any` is treated as an invalid source annotation, not as a dynamic top type

---

## Example 1: Typed `let` + typed function

```flux
fn add(x: Int, y: Int) -> Int {
    x + y
}

let total: Int = add(40, 2)
```

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/01_typed_let_and_fn.flx
cargo run --features jit -- --no-cache examples/guide_type_system/01_typed_let_and_fn.flx --jit
```

---

## Example 2: Annotations vs inference

`02_annotations_vs_inference.flx` shows inferred and explicit values used together.

`03_pure_function_boundaries.flx` shows typed tuple and list manipulation without effects.

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/02_annotations_vs_inference.flx
cargo run -- --no-cache examples/guide_type_system/03_pure_function_boundaries.flx
```

---

## Generics

Generic functions use `<T>` type parameters. Only `fn f<T>(x: T)` syntax triggers let-polymorphism for explicitly-parameterized forms:

```flux
fn wrap<T>(x: T) -> Option<T> { Some(x) }

let a = wrap(42)     // Option<Int>
let b = wrap("hi")   // Option<String>
```

User-defined generic ADTs use the same syntax (see [Chapter 13](13_match_exhaustiveness_and_adts.md)).

---

## Failure Patterns

| Situation | Error | Hint |
|-----------|-------|------|
| Annotated binding type doesn't match inferred RHS | `E300` | Check type annotation and value |
| Annotated return type doesn't match function body | `E300` | Align return annotation with body |
| Calling a typed function with wrong argument type | `E300` | Fix argument type at call site |
| Infinite recursive type detected | `E301` | Break the cycle with an explicit ADT |

---

## Static Typing Guarantees

Flux's static-typing story is closed. The language makes the following guarantees about any program the compiler accepts:

### Before execution

- **No unresolved types reach runtime.** Every let binding, function return, and sub-expression either resolves to a concrete type or is rejected with `E430` (expression residue) or `E004` (binding residue). `Any` is not a source-level type.
- **Annotations are contracts, not hints.** Declared parameter types, return types, and let-binding types are checked against the body. Mismatches are `E300`. Quantified variables in signatures are rigid (skolemized) — solving them with a concrete type raises `E305`.
- **Numeric ambiguity resolves deterministically.** A binding with only `Num a` obligations (no other constraints, not an explicit bound, not appearing in the public type) defaults to `Int`. This applies at every generalization site, top-level and local.
- **Type-class method calls monomorphize or dictionary-pass explicitly.** No silent fallback; a missing instance is `E441`. Effect floors on class methods are enforced with `E452`.
- **Recursive signatures enable polymorphic recursion.** An annotated recursive function can call itself at a polymorphic instantiation; unannotated recursion stays monomorphic.
- **Effect rows are inferred and checked.** Row-polymorphic effects unify like types; row conflicts raise `E304`, missing effects at call sites raise `E400`.

### At the runtime boundary

- **Typed public APIs check incoming and outgoing values.** Arguments and returns are matched against the declared type, including ADT constructor shape, list/array element types, tuple arities, and function arity + effects.
- **Closures without stored contracts are rejected at typed function boundaries.** There is no auto-accept for opaque callables.
- **Module interfaces preserve full runtime contracts.** Cross-module calls get the same shape checks as intra-module ones, including for user-defined ADTs.
- **Strict mode rejects unresolvable boundary types.** Generic-only public types emit `E425` so the boundary is never silently un-enforced.

### Internally

- **Core IR is validated after every pass.** A `core_lint` verifier checks binder scope, parameter-arity metadata, case/handler shape, and recursive-group invariants. Violations are fatal (`E998`).
- **Inferred schemes render deterministically.** `forall a, b. Eq<a>, Num<a> => (a, b) -> a` — canonical names, sorted constraints, regardless of inference-internal id allocation. The same formatter drives diagnostics, `--dump-core`, caches, and inspection surfaces.

### What this means for writing Flux

If the compiler accepts your program, you do not need a runtime `typeof` check, a defensive `match` for an unexpected shape, or a cast at a module boundary. The type you see in an annotation or a `--dump-core` rendering is the type the runtime will see.

If you want a guarantee the compiler does not yet give — higher-rank polymorphism without explicit signatures, multi-parameter type classes, record row polymorphism — that is outside the current static-typing closure. Open a proposal rather than reaching for `Any`.

---

## Next

Continue to [Chapter 10 — Effects and Purity](10_effects_and_purity.md).
