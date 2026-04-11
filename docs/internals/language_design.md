# Flux Language Design

> A pure functional programming language with algebraic effects, Hindley-Milner type inference, and familiar syntax

## Identity

**Flux** is a strict, pure functional language that combines:
- **Haskell's purity** — referential transparency, immutability, no side effects without explicit annotation
- **Koka's algebraic effects** — first-class effect handlers for IO, state, exceptions, and custom effects
- **Rust/TypeScript syntax** — braces, type annotations with `:`, generics with `<T>`, familiar operators

The name reflects data flowing through pipelines — the core metaphor of functional programming.

**File extension:** `.flx`

> Canonical type system and effects semantics: `docs/internals/type_system_effects.md`

## Design Principles

1. **Pure by default** — functions are referentially transparent unless annotated with effects
2. **Effects are explicit** — `with IO` annotations track what a function can do
3. **Expression-everything** — `if`, `match`, blocks are all expressions that produce values
4. **Familiar syntax** — brace-style blocks, no whitespace sensitivity, approachable for JS/Rust developers
5. **Data flows through pipes** — `|>` is the primary composition operator
6. **No null** — `Option` types (`Some`/`None`) replace null entirely
7. **Human-friendly errors** — Elm-style diagnostics with source snippets, hints, and suggestions
8. **Three execution backends** — bytecode VM (fast startup), Cranelift JIT (fast execution), LLVM (optimized native code)

## What Makes Flux Unique

| | Haskell | Koka | Elm | OCaml | **Flux** |
|---|---|---|---|---|---|
| Syntax | Indentation-based | ML-style | Elm-specific | ML-style | **Brace-based (Rust/TS)** |
| Evaluation | Lazy | Strict | Strict | Strict | **Strict** |
| Effects | IO monad | Algebraic | Cmd/Sub | None | **Algebraic** |
| Purity | Enforced | Enforced | Enforced | Not enforced | **Enforced** |
| Type inference | HM + type classes | HM + rows | HM | HM | **HM + effect rows + type classes** |
| Target audience | FP experts | Researchers | Web devs | Systems/FP | **JS/Rust developers** |
| Backends | NCG/LLVM | C codegen | JS | Native | **VM + JIT + LLVM** |

The gap Flux fills: **no existing language combines type classes with algebraic effects in a way that allows per-instance effect rows**. Haskell has type classes but uses monads for effects (no per-instance effect signatures). Koka has algebraic effects but no type classes (uses implicit parameters instead). Flux integrates both: a class method call dispatches to the right instance by type, and the resolved instance's effect row flows to the caller. This is unique to Flux.

---

## Syntax Overview

### Functions and Effects

```flux
// Pure function — no effects, guaranteed referentially transparent
fn add(a: Int, b: Int) -> Int {
    a + b
}

// Effectful function — must declare effects with `with`
fn greet(name: String) with IO {
    print("Hello, " + name)
}

// Multiple effects
fn fetch_data(url: String) -> String with IO, Fail {
    let response = http_get(url)
    parse(response)
}

// Effects propagate — callers must have the same effects
fn main() with IO {
    greet("Alice")        // OK — main has IO
    let x = add(1, 2)    // OK — add is pure, works anywhere
}
```

### Algebraic Effects and Handlers

```flux
// Define a custom effect
effect Console {
    print: String -> ()
    read: () -> String
}

// Use effects with `perform`
fn program() with Console {
    perform Console.print("What is your name?")
    let name = perform Console.read()
    perform Console.print("Hello, " + name)
}

// Handle effects — intercept and control behavior
program() handle Console {
    print(resume, msg) -> {
        log_to_file(msg)
        resume(())
    }
    read(resume) -> resume("Alice")
}
```

### Algebraic Data Types

```flux
// Sum type (enum with data)
data Shape {
    Circle(Float),
    Rectangle(Float, Float),
    Point,
}

// Pattern matching with exhaustiveness checking
fn area(s: Shape) -> Float {
    match s {
        Circle(r) -> 3.14159 * r * r,
        Rectangle(w, h) -> w * h,
        Point -> 0.0,
    }
}

// Built-in: Option (no null) and Either (error handling)
fn safe_divide(a: Int, b: Int) -> Option {
    if b == 0 { None } else { Some(a / b) }
}
```

### Pipelines

```flux
// Data flows left-to-right through transformations
let result = data
    |> parse
    |> validate
    |> transform
    |> format

// With inline lambdas
numbers
    |> map(fn(x) { x * 2 })
    |> filter(fn(x) { x > 10 })
    |> fold(0, fn(acc, x) { acc + x })
```

### Collections

```flux
// Arrays
let numbers = [1, 2, 3, 4, 5]

// Tuples
let point = (10, 20)
let person = ("Alice", 30, true)

// Hash maps (persistent HAMT)
let user = { "name": "Alice", "age": 30 }

// Cons lists (persistent, O(1) prepend)
let list = 1 :: 2 :: 3 :: []
```

### Modules and Imports

```flux
// Define a module
module Math {
    public fn square(x: Int) -> Int { x * x }
    fn _helper(x: Int) -> Int { x + 1 }  // private
}

// Import and use
import Math
let result = Math.square(5)

// Aliased import
import Long.Module.Name as M
let x = M.function()

// Selective import — unqualified access
import Flow.Foldable exposing (fold, length)
let total = fold([1, 2, 3], 0, fn(acc, x) { acc + x })
```

### Type Classes (Proposal 0145, 0151)

```flux
// Define a class inside a module
module Flow.Comparable {
    public class Comparable<a> {
        fn same(x: a, y: a) -> Bool
    }

    public instance Comparable<Int> {
        fn same(x, y) { x == y }
    }
}

// Use class methods through module alias
import Flow.Comparable as Comparable

let equal = Comparable.same(1, 2)
```

Type classes use GHC-style dictionary passing under the hood. The compiler resolves monomorphic calls directly to mangled instance functions at compile time.

#### Effectful Type Class Methods

Class methods can declare an effect floor. Different instances can have different effect rows:

```flux
module Flow.Comparable {
    public class Comparable<a> {
        fn same(x: a, y: a) -> Bool    // no floor — pure by default
    }

    // Pure instance
    public instance Comparable<Int> {
        fn same(x, y) { x == y }
    }
}

// In another module — effectful instance
module App.Users {
    public instance Comparable<UserId> {
        fn same(a, b) with AuditLog {
            perform AuditLog.record("comparing users")
            match (a, b) { (Id(x), Id(y)) -> x == y }
        }
    }
}
```

The compiler propagates the resolved instance's effect row to callers: `Comparable.same(1, 2)` is pure, `Comparable.same(user1, user2)` requires `AuditLog`.

### Pattern Matching

```flux
// Value matching
match status {
    "active" -> handle_active(),
    "pending" -> handle_pending(),
    _ -> handle_unknown(),
}

// Destructuring
match result {
    Some(value) -> use(value),
    None -> default_value,
}

// Nested patterns
match shape {
    Circle(r) if r > 10.0 -> "large circle",
    Circle(r) -> "small circle",
    Rectangle(w, h) -> to_string(w) + "x" + to_string(h),
    Point -> "point",
}

// List patterns
match list {
    [] -> "empty",
    [x] -> "single: " + to_string(x),
    [x, ...rest] -> "head: " + to_string(x),
}
```

### List Comprehensions

```flux
let squares = for x in numbers { x * x }
let evens = for x in numbers, x % 2 == 0 { x }
let pairs = for x in [1, 2], y in ["a", "b"] { (x, y) }
```

---

## Type System

### Hindley-Milner Inference

Types are inferred automatically. Annotations are optional but encouraged for public APIs.

```flux
// Inferred as: Int -> Int -> Int
fn add(a, b) { a + b }

// Explicit annotation
fn add(a: Int, b: Int) -> Int { a + b }

// Strict mode (--strict): public functions must have annotations
public fn api_function(x: Int) -> String { to_string(x) }
```

### Gradual Typing

Unannotated code infers as `Any` when types can't be determined. This enables incremental adoption of types.

```flux
fn identity(x) { x }    // x: Any → Any (gradual)
fn double(x: Int) -> Int { x * 2 }  // fully typed
```

### Effect Rows

Effects use row-polymorphic types. Functions can be polymorphic over effects:

```flux
// This function works in any effect context
fn apply(f, x) { f(x) }

// The effect of `apply` is inferred from `f`
fn main() with IO {
    apply(print, "hello")   // IO effect flows through
    apply(add, 1)           // no effect
}
```

Effect rows interact with type classes: different instances of the same class can carry different effect rows, and the compiler propagates the correct row through type-directed dispatch. See the Type Classes section above for examples.

---

## Execution Backends

| Backend | Flag | Speed | Use case |
|---------|------|-------|----------|
| **Bytecode VM** | (default) | Interpreted | Development, debugging, fast startup |
| **Cranelift JIT** | `--jit` | Native (fast compile) | Interactive development with speed |
| **LLVM** | `--llvm` | Native (optimized) | Release builds, AOT compilation |

```bash
flux run program.flx              # VM
flux run program.flx --jit        # Cranelift JIT
flux run program.flx --llvm       # LLVM JIT
flux run program.flx --llvm --emit-obj -O   # AOT object file
```

All three backends produce identical output (enforced by parity testing).

---

## Error Messages

Flux follows Elm's philosophy: errors teach, not frustrate.

```
• 1 error • src/main.flx
error[E1009]: Invalid Operation

Cannot add Int and None values.

  src/main.flx:3:3
  |
3 |   1 + None;
  |   ^^^^^^^^

Stack trace:
  at boom (src/main.flx:3:3)
  at <main> (src/main.flx:6:1)
```

```
error[E400]: Missing Effect

requires effect `IO` but the enclosing context does not provide it

  src/main.flx:3:5
  |
3 |     print("hello")
  |     ^^^^^^^^^^^^^^
  |     requires `IO`

Hint:
  annotate the enclosing function with `with IO`
```

---

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Paradigm | Pure functional | Referential transparency, composability |
| Mutability | Immutable only | Eliminates state bugs, enables optimization |
| Null | None/Option only | Billion-dollar mistake avoided |
| Evaluation | Strict | Predictable performance, no thunk overhead |
| Syntax | Brace-style | Familiar to JS/Rust developers |
| Type inference | HM + gradual | Safety with incremental adoption |
| Effects | Algebraic (row-polymorphic) | Composable, interceptable, testable |
| Ad-hoc polymorphism | Type classes (Haskell-style, dictionary passing) | Familiar, expressive, integrates with effect rows |
| Semicolons | Optional | Expression-based, newlines sufficient |
| Backend | VM + JIT + LLVM | Development speed + release performance |
| Collections | Persistent (HAMT, cons lists) | Immutable with structural sharing |
| File extension | `.flx` | Short, unique |
| Function keyword | `fn` | Concise, Rust-familiar |
| Lambda syntax | `fn(x) { expr }` | Consistent with named functions |
| Pipe operator | `\|>` | F#/Elm/Elixir convention |
| Match arms | `->` | Pattern → result |
| Type annotations | `x: Type` | Rust/TypeScript convention |
| Module access | `Module.function` | Dot notation, familiar |
| Error style | Elm-inspired | Human-friendly, educational |

---

## Roadmap

### Implemented (v0.0.4)
- Hindley-Milner type inference with effect rows
- Algebraic effects with `perform`/`handle`
- Type classes with GHC-style dictionary passing (Proposal 0145)
- Module-scoped type classes with ClassId-keyed storage, orphan rule, coherence (Proposal 0151, Phases 1–4)
- Effectful type class methods — per-instance effect rows with type-directed propagation
- Row-polymorphic class methods (`with |e`) end-to-end
- ADTs with exhaustiveness checking
- Pattern matching (nested, guards, wildcards)
- Persistent collections (HAMT maps, cons lists)
- Three execution backends (VM, JIT, LLVM)
- 77 built-in base functions
- Elm-style diagnostics (E001–E458)
- Bytecode cache (.fxc files)
- Module system with imports, aliases, visibility, `exposing`
- Tail call optimization
- Built-in test runner

### Planned (v0.0.5+)
- `Any` elimination (proposal 0099) — full static typing once traits cover ad-hoc polymorphism
- Standalone binary emission (proposal 0106) — `flux build program.flx -o program`
- Perceus reference counting (proposals 0068-0070) — in-place mutation when refcount=1
- Package system (proposal 0015) — module distribution and dependency management
- Stdlib migration to module-scoped classes (Proposal 0151, Phase 6)
