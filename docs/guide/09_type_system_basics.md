# Chapter 9 — Type System Basics

> Examples: [`examples/guide_type_system/01_typed_let_and_fn.flx`](../../examples/guide_type_system/01_typed_let_and_fn.flx), [`02_annotations_vs_inference.flx`](../../examples/guide_type_system/02_annotations_vs_inference.flx), [`03_pure_function_boundaries.flx`](../../examples/guide_type_system/03_pure_function_boundaries.flx)

## Learning Goals

- Read and write typed `let` bindings and function signatures.
- Understand what Hindley-Milner inference does and when it kicks in.
- Understand `Any` as the gradual fallback type.
- Recognize compile-time type mismatches (`E300`) and when they occur.

## Overview

Flux has a **gradual type system**: type annotations are optional, and unannotated code participates in Hindley-Milner (HM) inference. The key properties are:

- **Annotated paths** — if you write a type, it is statically validated. Mismatches are compile-time errors (`E300`).
- **Inferred paths** — HM fills types for unannotated local expressions. The compiler reports concrete mismatches without false positives.
- **`Any` as escape hatch** — when inference can't determine a concrete type (heterogeneous branches, unresolvable generics), the type degrades to `Any`. `Any` unifies with everything, suppressing errors.
- **Gradual adoption** — you can type some functions and leave others untyped. The two coexist safely.

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
| `Any` | Gradual type — unifies with everything | — |

---

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
3. Unify constrained variables — if two types conflict, emit `E300` only when both sides are concrete (non-`Any`).
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

## The `Any` Type

`Any` is the gradual escape hatch. When inference cannot determine a concrete type, the value is treated as `Any`.

`Any` unifies with everything, which means:
- No error is emitted when `Any` meets a concrete type.
- Typed checks are bypassed for that expression.

`Any` occurs naturally in:
- Unannotated heterogeneous `if`/`match` branches.
- Calls to functions whose result type can't be resolved.
- Runtime values that arrive from dynamic dispatch.

```flux
// This is fine — the branch types are String and Int,
// but without a typed let, the result is Any
let x = if true { "hello" } else { 42 }
```

```flux
// This fails — typed let expects Int, but branch is Any → Int mismatch
let y: Int = if true { "hello" } else { 42 }
// E300: type mismatch
```

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

## Next

Continue to [Chapter 10 — Effects and Purity](10_effects_and_purity.md).
