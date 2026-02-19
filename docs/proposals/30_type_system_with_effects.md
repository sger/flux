# Proposal: Type System with Algebraic Effects for Flux

**Status:** Draft
**Date:** 2026-02-17

---

## 1. Motivation

Flux is currently a dynamically typed language. All type errors are caught at runtime, which means:

- Bugs surface late (at execution, not compilation)
- Refactoring is risky without exhaustive test coverage
- Functions have no documented contracts — callers must read implementations to know what types are expected
- Side effects (IO, state, failure) are invisible in function signatures

Adding a **gradual type system with algebraic effects** would give Flux:

1. **Compile-time safety** — catch type mismatches, missing pattern arms, and arity errors before running
2. **Self-documenting code** — types serve as machine-checked documentation
3. **Effect tracking** — distinguish pure functions from those that do IO, mutate state, or may fail
4. **Incremental adoption** — existing untyped code continues to work

---

## 2. Design Principles

| Principle | Rationale |
|---|---|
| **Gradual** | Types are optional. Untyped code infers as `Any`. No breaking changes. |
| **Inferred** | Local types are inferred. Annotations required only at module boundaries. |
| **Functional-first** | Function types are first-class. Effects compose naturally. |
| **Minimal syntax** | Reuse existing keywords. Avoid ceremony. |
| **Effect-aware** | Effects are part of the type system, not bolted on. |

---

## 3. Type Syntax

### 3.1 Primitive Types

```
Int       // i64
Float     // f64
Bool      // true | false
String    // Rc<str>
Unit      // the empty tuple (), absence of meaningful value
Never     // bottom type, inhabited by no values (divergent computations)
```

### 3.2 Composite Types

```
Option<T>          // Some(T) | None
Either<L, R>       // Left(L) | Right(R)
List<T>            // Persistent cons list
Array<T>           // Dense array
Map<K, V>          // Persistent HAMT map
(A, B)             // Tuple of two elements
(A, B, C)          // Tuple of three elements
```

### 3.3 Function Types

```
(Int, Int) -> Int                   // pure function
(String) -> Unit with IO            // effectful function
(List<T>, (T) -> Bool) -> List<T>   // higher-order, polymorphic
```

`->` is used consistently for all function-like things (named functions, lambdas, function types). It binds tighter than `with`:

```
fn f(x: Int) -> Int with IO    // means: returns Int, has effect IO
                                // NOT: returns (Int with IO)
```

### 3.4 Generics

Generic type parameters use angle brackets on type and function declarations:

```flux
fn identity<T>(x: T) -> T { x }

fn pair<A, B>(a: A, b: B) -> (A, B) { (a, b) }

fn map<T, U>(xs: List<T>, f: (T) -> U) -> List<U> {
    match xs {
        [h | t] -> [f(h) | map(t, f)],
        [] -> []
    }
}
```

Constraints can restrict type parameters (future extension):

```flux
fn sum<T: Numeric>(xs: List<T>) -> T {
    fold(xs, 0, \(acc, x) -> acc + x)
}
```

---

## 4. Tuples

Tuples are fixed-size, heterogeneous collections. Unlike arrays (homogeneous, variable-length), each position in a tuple can hold a different type.

**Runtime representation:** `Value::Tuple(Rc<Vec<Value>>)` — a distinct variant from `Value::Array`. This avoids conflating "variable-length homogeneous array" with "fixed-length heterogeneous tuple" and enables better JIT specialization.

### 4.1 Tuple Syntax

```flux
// Construction
let point = (10, 20)
let record = ("Alice", 42, true)

// Type annotation
let point: (Int, Int) = (10, 20)
let entry: (String, Int) = ("score", 100)
```

### 4.2 Destructuring

```flux
let (x, y) = point
let (name, age, active) = record

// In function parameters
fn distance((x1, y1): (Int, Int), (x2, y2): (Int, Int)) -> Float {
    let dx = x2 - x1
    let dy = y2 - y1
    sqrt(to_float(dx * dx + dy * dy))
}
```

### 4.3 Pattern Matching

```flux
fn describe(pair: (String, Int)) -> String {
    match pair {
        (name, score) if score >= 90 -> name ++ " got an A",
        (name, score) if score >= 80 -> name ++ " got a B",
        (name, _) -> name ++ " needs improvement"
    }
}
```

### 4.4 Tuple Access

```flux
let t = (10, "hello", true)
t.0   // 10
t.1   // "hello"
t.2   // true
```

### 4.5 Unit Type

The empty tuple `()` is the `Unit` type — used when a function returns nothing meaningful:

```flux
fn greet(name: String) -> Unit with IO {
    print("Hello, " ++ name)
}
```

---

## 5. Annotating Declarations

### 5.1 Let Bindings

Type annotations use `:` after the binding name. They are **optional** — the compiler infers types when omitted.

```flux
let x: Int = 42
let name: String = "hello"
let xs: List<Int> = list(1, 2, 3)

// Inferred — no annotation needed
let y = x + 1          // inferred as Int
let greeting = "hi"    // inferred as String
```

### 5.2 Function Declarations

Parameters and return types annotated with `:` and `->`:

```flux
fn add(a: Int, b: Int) -> Int {
    a + b
}

fn greet(name: String) -> Unit with IO {
    print("Hello, " ++ name)
}

// Fully inferred — still valid
fn double(x) { x * 2 }
```

**Return type rules:**

- `-> T` is required when the return type is not `Unit`
- When the return type is `Unit`, `->` can be omitted:
  - `fn log(x: String) with IO { print(x) }` — implied `-> Unit`
  - `fn id<T>(x: T) -> T { x }` — explicit, required
- All functions must have a block body `{ ... }`. No expression-bodied `fn`.

### 5.3 Lambda Expressions

```flux
let inc: (Int) -> Int = \x -> x + 1

// Or annotate inline
let inc = \(x: Int) -> x + 1

// Block lambda
let log_and_double = \(x: Int) -> {
    print(to_string(x))
    x * 2
}

// Type inferred from context
map(list(1, 2, 3), \x -> x + 1)   // x inferred as Int
```

### 5.4 Module-Level Type Signatures

For public module functions, top-level signatures act as documentation and API contracts:

```flux
module Math {
    fn abs(x: Int) -> Int {
        if x < 0 { -x } else { x }
    }

    fn max(a: Int, b: Int) -> Int {
        if a > b { a } else { b }
    }

    fn sum(xs: List<Int>) -> Int {
        fold(xs, 0, \(acc, x) -> acc + x)
    }
}
```

---

## 6. Type Aliases and Custom Types

### 6.1 Type Aliases

```flux
type Name = String
type Score = Int
type Predicate<T> = (T) -> Bool
type Point = (Int, Int)
```

### 6.2 Algebraic Data Types (ADTs)

Introduce `data` declarations for sum types:

```flux
data Shape {
    Circle(Float),
    Rectangle(Float, Float),
    Triangle(Float, Float, Float),
}

fn area(s: Shape) -> Float {
    match s {
        Circle(r) -> 3.14159 * r * r,
        Rectangle(w, h) -> w * h,
        Triangle(a, b, c) -> {
            let s = (a + b + c) / 2.0
            sqrt(s * (s - a) * (s - b) * (s - c))
        }
    }
}
```

### 6.3 Generic ADTs

```flux
data Tree<T> {
    Leaf(T),
    Node(Tree<T>, T, Tree<T>),
}

fn tree_sum(t: Tree<Int>) -> Int {
    match t {
        Leaf(v) -> v,
        Node(left, v, right) -> tree_sum(left) + v + tree_sum(right),
    }
}

data Result<T, E> {
    Ok(T),
    Err(E),
}
```

### 6.4 Relationship to Existing Types

`Option<T>` and `Either<L, R>` become built-in ADTs:

```flux
// These are defined by the language, not user code:
data Option<T> { Some(T), None }
data Either<L, R> { Left(L), Right(R) }
```

Existing `Some(x)`, `None`, `Left(x)`, `Right(x)` syntax continues to work unchanged.

---

## 7. The `Any` Type: Semantics

`Any` is the **dynamic type** — it is not a supertype of everything, but rather a boundary between typed and untyped code. Values crossing this boundary undergo runtime checks.

### 7.1 Flow Rules

| Direction | Behavior | Cost |
|---|---|---|
| `T -> Any` | Always succeeds, free (erase type info) | Zero-cost |
| `Any -> T` | Inserts a **runtime cast/check** at the boundary | Runtime check |
| `Any -> Any` | No check needed | Zero-cost |

```flux
// T -> Any: free, always works
fn old_add(x, y) { x + y }    // x and y are Any
let a: Any = 42               // Int -> Any, free

// Any -> T: runtime check inserted at boundary
fn typed_add(a: Int, b: Int) -> Int { a + b }
let x = old_add(1, 2)         // x: Any
typed_add(x, 3)               // Any -> Int, runtime check on x
```

### 7.2 Runtime Cast Failure

When `Any -> T` fails, it produces a runtime type error:

```flux
fn double(x: Int) -> Int { x * 2 }

let val: Any = "hello"
double(val)
// Runtime error: expected Int, got String at <call site>
```

### 7.3 Soundness Guarantee

This approach ensures that **typed code is never unsound**:

- If all annotations say `Int`, you will only ever see `Int` values
- Runtime checks happen at the `Any -> T` boundary, never inside fully typed code
- Fully typed programs have zero runtime cast overhead

### 7.4 Equivalence

```flux
// These are equivalent — unannotated code is implicitly Any
fn old_style(x) { x + 1 }
fn old_style(x: Any) -> Any { x + 1 }
```

### 7.5 Mixing Typed and Untyped Code

```flux
// Typed module
module Math {
    fn add(a: Int, b: Int) -> Int { a + b }
}

// Untyped script using it — works fine
let result = Math.add(1, 2)         // OK, Int values pass the check
let bad = Math.add("x", 2)         // Compile error! String != Int
```

---

## 8. The `Never` Type

`Never` is the **bottom type** — it has no values. It represents computations that never return (divergence, exceptions, infinite loops).

### 8.1 Use in Effects

```flux
effect Error<E> {
    fn raise(e: E) -> Never
}
```

`raise` returns `Never` because it never produces a value — it transfers control to a handler. This makes conditional code type-check naturally:

```flux
fn parse_or_fail(s: String) -> Int with Error<String> {
    match parse_int(s) {
        Some(n) -> n,          // n: Int
        None -> raise("bad")   // raise: Never, coerces to Int
    }
}
```

Without `Never`, the `raise` branch would need a dummy return value or a special type hack.

### 8.2 Type Rules

- `Never` is a subtype of every type: `Never <: T` for all `T`
- An expression of type `Never` can appear anywhere any type is expected
- `Never` is not exposed in surface syntax initially — users write `raise(...)` and the checker assigns `Never` internally

### 8.3 Other Uses

```flux
// Infinite loop
fn forever<T>(f: () -> Unit with e) -> Never with e {
    f()
    forever(f)
}

// Unreachable (compiler can verify dead code)
fn unwrap_or_panic<T>(opt: Option<T>) -> T {
    match opt {
        Some(v) -> v,
        None -> panic("unwrap failed")  // panic: -> Never
    }
}
```

---

## 9. Effect System

### 9.1 What Are Effects?

Effects describe *what a function does* beyond computing a return value. They make side effects explicit and trackable in the type system.

```
Pure function:     (Int, Int) -> Int              // no effects
IO function:       (String) -> Unit with IO       // reads/writes external world
Fallible function: (String) -> Int with Error      // may fail
Stateful function: () -> Int with State<Int>       // reads/modifies state
```

### 9.2 Declaring Effects

Effects are declared with the `effect` keyword, listing their operations:

```flux
effect IO {
    fn print(s: String) -> Unit
    fn read_line() -> String
    fn read_file(path: String) -> String
    fn read_lines(path: String) -> Array<String>
}

effect Error<E> {
    fn raise(e: E) -> Never
}

effect State<S> {
    fn get() -> S
    fn put(s: S) -> Unit
}

effect Random {
    fn random_int(min: Int, max: Int) -> Int
}

effect Time {
    fn now_ms() -> Int
}
```

### 9.3 Built-in Effects and Typed Builtins

The standard library pre-declares commonly used effects. Existing builtins get typed signatures:

**IO effect operations (already exist as builtins):**

```
print       : (String) -> Unit          with IO
read_file   : (String) -> String        with IO
read_lines  : (String) -> Array<String> with IO
read_stdin  : () -> String              with IO
```

**Pure builtins (no effect required):**

```
len         : <T>(Array<T>) -> Int
first       : <T>(Array<T>) -> Option<T>
last        : <T>(Array<T>) -> Option<T>
rest        : <T>(Array<T>) -> Array<T>
push        : <T>(Array<T>, T) -> Array<T>
concat      : <T>(Array<T>, Array<T>) -> Array<T>
reverse     : <T>(Array<T>) -> Array<T>
contains    : <T>(Array<T>, T) -> Bool
slice       : <T>(Array<T>, Int, Int) -> Array<T>
sort        : <T>(Array<T>) -> Array<T>
range       : (Int, Int) -> Array<Int>
to_string   : <T>(T) -> String
split       : (String, String) -> Array<String>
join        : (Array<String>, String) -> String
trim        : (String) -> String
upper       : (String) -> String
lower       : (String) -> String
substring   : (String, Int, Int) -> String
parse_int   : (String) -> Option<Int>
type_of     : <T>(T) -> String
abs         : (Int) -> Int
min         : (Int, Int) -> Int
max         : (Int, Int) -> Int
hd          : <T>(List<T>) -> T
tl          : <T>(List<T>) -> List<T>
list        : <T>(T...) -> List<T>
to_list     : <T>(Array<T>) -> List<T>
to_array    : <T>(List<T>) -> Array<T>
map         : <T, U>(List<T>, (T) -> U with e) -> List<U> with e
filter      : <T>(List<T>, (T) -> Bool with e) -> List<T> with e
fold        : <T, U>(List<T>, U, (U, T) -> U with e) -> U with e
put         : <K, V>(Map<K, V>, K, V) -> Map<K, V>
get         : <K, V>(Map<K, V>, K) -> Option<V>
keys        : <K, V>(Map<K, V>) -> Array<K>
values      : <K, V>(Map<K, V>) -> Array<V>
```

**Time effect:**

```
now_ms      : () -> Int                 with Time
```

**Rule:** Effect operations are in scope like normal functions when the effect is in the ambient effect set. Calling `print(...)` inside a `with IO` function works directly — no `perform` keyword needed.

In untyped code (no annotations), all builtins remain callable as before. The effect system only constrains when the caller has explicit type annotations.

### 9.4 The `fn main` Entry Point

Effects require an explicit program entry point. `fn main` is the **root effect handler** — it implicitly handles all its declared effects by connecting to the real world.

```flux
fn main() with IO {
    let lines = read_lines("input.txt")
    print(len(lines))
}
```

**Rules:**

- Top-level code is treated as an **implicit pure module initializer**
- Any effectful operation at top-level is a **compile error** once effects are enabled
- Pure top-level `let` bindings and pure function definitions remain valid without `fn main`
- `fn main` can declare any combination of effects: `fn main() with IO, Error<String> { ... }`
- The return type of `fn main` is implicitly `Unit`
- A program must have exactly one `fn main` if it performs effects

```flux
// Pure program — no main needed
let x = 42
let y = x + 1

// Effectful program — requires main
fn main() with IO {
    print(to_string(x))    // OK — IO is handled by main
}
```

```flux
// ERROR: top-level effectful operation
let lines = read_lines("input.txt")  // Compile error: `read_lines` requires IO
                                      // hint: move this into `fn main() with IO { ... }`
```

### 9.5 Annotating Effects on Functions

Use `with` after the return type (or after parameters when return type is omitted) to declare effects:

```flux
// Single effect
fn greet(name: String) -> Unit with IO {
    print("Hello, " ++ name)
}

// Multiple effects
fn logged_increment() -> Int with IO, State<Int> {
    let current = get()
    print("Current value: " ++ to_string(current))
    put(current + 1)
    current
}

// No annotation = pure
fn add(a: Int, b: Int) -> Int {
    a + b
}

// Shorthand: omit return type, just declare effects (implied -> Unit)
fn say_hello() with IO {
    print("hello")
}
```

### 9.6 Effect Inference

When type annotations are omitted, effects are **inferred** from the function body (Option B):

```flux
// Effect inferred as IO because print is an IO operation
fn say_hello() {
    print("hello")
}
// Inferred signature: fn say_hello() -> Unit with IO

// Inferred as pure — no effects
fn square(x) { x * x }
// Inferred signature: fn square(x: Any) -> Any
```

When you **explicitly annotate** a function as pure but use effects, you get a compile error:

```flux
fn add(a: Int, b: Int) -> Int {
    print("debug")  // ERROR: fn `add` is pure but `print` requires IO
    a + b            // hint: add `with IO` to the function signature
}
```

### 9.7 Effect Set Rule

An expression is well-typed if the **set of effects it performs** is a subset of the **ambient effect set** of the enclosing function:

```
performed_effects(expr) ⊆ declared_effects(enclosing_fn)
```

- `handle` **removes** an effect from the ambient set (the handler provides the implementation)
- `resume` reinstates the continuation with a value
- Pure (empty effect set) is a subset of every effect set — pure functions can be called anywhere

### 9.8 Effect Polymorphism (v1 scope)

Higher-order functions can use a **single effect variable** `e` to be polymorphic over effects:

```flux
fn map<T, U>(xs: List<T>, f: (T) -> U with e) -> List<U> with e {
    match xs {
        [h | t] -> [f(h) | map(t, f)],
        [] -> []
    }
}
```

This means `map` preserves whatever effects `f` has:

```flux
// Pure — no effects
map(list(1, 2, 3), \x -> x * 2)

// With IO — because the lambda prints
map(list(1, 2, 3), \x -> { print(x); x * 2 })
```

**v1 constraints:** Effect variables (`e`) are treated as **opaque effect sets**. No row polymorphism, effect subtraction, or presence constraints. A single `with e` means "whatever effects the callback needs." This keeps the checker simple while preserving the right surface syntax for future extension.

### 9.9 Effect Handlers

Handlers give concrete implementations to abstract effects. The *caller* decides how effects are interpreted, not the callee.

```flux
// Basic handler syntax
handle expr with {
    EffectName {
        operation(args...) -> resume(value),
    }
}
```

#### Example: Error Handling

```flux
fn parse_config(path: String) -> Config with IO, Error<String> {
    let content = read_file(path)
    if content == "" {
        raise("empty config file")
    }
    parse(content)
}

// Handle the Error effect — convert to Option
fn main() with IO {
    let result = handle parse_config("app.conf") with {
        Error {
            raise(e) -> {
                print("Warning: " ++ e)
                None
            }
        }
    }
    // result: Option<Config>
    // The IO effect is still ambient (handled by main)
    // The Error effect was handled by the handle block
}
```

#### Example: Testing with Mocked Effects

```flux
fn fetch_user(id: Int) -> String with IO {
    let data = read_file("/users/" ++ to_string(id))
    data
}

// In tests — mock the IO effect
let user = handle fetch_user(42) with {
    IO {
        read_file(_) -> resume("{\"name\": \"test\"}"),
        print(_)     -> resume(()),
    }
}
```

### 9.10 Effect Subtyping

A pure function can be used where an effectful one is expected (effects are covariant):

```flux
fn apply(f: (Int) -> Int with IO) -> Int with IO {
    f(42)
}

// Pure function works here — pure ⊆ {IO}
apply(\x -> x + 1)  // OK
```

---

## 10. Empty List: `[]` vs `None`

In current Flux, `None` serves double duty as both "no value" (Option) and "empty list" (List). With a type system, this ambiguity must be resolved.

### 10.1 The Problem

```flux
// Is this Option<T> or List<T>?
fn bad_example() { None }

// These return List<T> but use None
fn map<T, U>(xs: List<T>, f: (T) -> U) -> List<U> {
    match xs {
        [h | t] -> [f(h) | map(t, f)],
        _ -> None    // confusing: None here means "empty list"
    }
}
```

### 10.2 The Rule

- **`[]`** is the empty list literal, with type `List<T>`
- **`None`** is the absent value, with type `Option<T>`
- The two are **distinct types** and not interchangeable in typed code

```flux
// Correct
fn map<T, U>(xs: List<T>, f: (T) -> U) -> List<U> {
    match xs {
        [h | t] -> [f(h) | map(t, f)],
        [] -> []
    }
}

fn find<T>(xs: List<T>, pred: (T) -> Bool) -> Option<T> {
    match xs {
        [h | t] -> if pred(h) { Some(h) } else { find(t, pred) },
        [] -> None    // None here means Option::None, not empty list
    }
}
```

### 10.3 Pattern Matching

```flux
match my_list {
    [h | t] -> ...,    // cons pattern
    [] -> ...,          // empty list pattern (replaces _ -> None for lists)
}

match my_option {
    Some(v) -> ...,
    None -> ...,        // option-none pattern
}
```

### 10.4 Migration

In untyped code, `None` and `[]` remain interchangeable at runtime (backward compat). The distinction is enforced only when type annotations are present.

---

## 11. Error Model

### 11.1 Compile-Time Errors

| Error | When |
|---|---|
| Type mismatch | `fn f(x: Int) ...` called with `f("hi")` |
| Missing effect | Pure function calls effectful operation |
| Arity mismatch | Wrong number of arguments |
| Unknown type | Annotation references undefined type |
| ADT constructor arity | `Circle(1, 2)` when `Circle` takes one arg |
| Non-exhaustive match (typed) | Missing constructor in `match` over ADT |
| Top-level effect | Effectful operation outside `fn main` |

### 11.2 Runtime Errors

| Error | When |
|---|---|
| `Any -> T` cast failure | Dynamic value doesn't match expected type at boundary |
| Pattern match failure (untyped) | No arm matches at runtime in untyped code |

### 11.3 Policy

- **Typed code:** Non-exhaustive `match` on an ADT is a **compile error**
- **Untyped code:** Non-exhaustive `match` remains a **runtime error** (current behavior)
- **Guards:** `if` guards in match arms are treated as "may fail" — they do not contribute to exhaustiveness (a `_` or unconditional arm is still required)

---

## 12. Exhaustiveness Checking (v1 Scope)

v1 exhaustiveness checking is deliberately scoped to be useful without becoming a rabbit hole.

### 12.1 What v1 Checks

- **ADT constructors:** All variants must be covered (or a wildcard `_` must be present)
- **Bool:** `true` and `false` must both be covered (or wildcard)
- **Option:** `Some(_)` and `None` must both be covered (or wildcard)
- **Either:** `Left(_)` and `Right(_)` must both be covered (or wildcard)

### 12.2 What v1 Does NOT Check

- **Guard reasoning:** Guards (`if expr`) are treated as "may fail" — they never satisfy exhaustiveness on their own
- **Nested patterns:** Only top-level constructor is checked. Nested patterns are not analyzed for exhaustiveness.
- **Tuple exhaustiveness:** Tuples are structural, not checked for exhaustiveness in v1. Use `_` as a catch-all.
- **Integer/String ranges:** Not checked. Always require a wildcard arm.

### 12.3 Example

```flux
data Color { Red, Green, Blue }

fn name(c: Color) -> String {
    match c {
        Red -> "red",
        Green -> "green",
        // ERROR: non-exhaustive match — missing Blue
    }
}

fn name(c: Color) -> String {
    match c {
        Red -> "red",
        _ -> "other",    // OK — wildcard covers Green, Blue
    }
}
```

---

## 13. Full Example: AoC-style Program

```flux
fn line_at(lines: Array<String>, idx: Int) -> String {
    match lines[idx] {
        Some(v) -> v,
        None -> ""
    }
}

fn char_at(line: String, col: Int) -> String {
    substring(line, col, col + 1)
}

fn is_roll(lines: Array<String>, rows: Int, cols: Int, r: Int, c: Int) -> Bool {
    if r < 0 || c < 0 || r >= rows || c >= cols {
        false
    } else {
        char_at(line_at(lines, r), c) == "@"
    }
}

fn count_all(lines: Array<String>, rows: Int, cols: Int) -> Int {
    // ... recursive counting logic
    0
}

fn main() with IO {
    let lines = read_lines("examples/io/aoc_day4.txt")
    let rows = len(lines)
    let cols = if rows == 0 { 0 } else { len(line_at(lines, 0)) }
    print(count_all(lines, rows, cols))
}
```

---

## 14. Full Example: Effect Handlers

```flux
effect Logger {
    fn log(level: String, msg: String) -> Unit
}

effect Config {
    fn get_config(key: String) -> Option<String>
}

fn start_server(port: Int) -> Unit with IO, Logger, Config {
    let host = match get_config("host") {
        Some(h) -> h,
        None -> "localhost"
    }
    log("info", "Starting server on " ++ host ++ ":" ++ to_string(port))
    print("Server running")
}

// Production
fn main() with IO {
    handle start_server(8080) with {
        Logger {
            log(level, msg) -> {
                print("[" ++ upper(level) ++ "] " ++ msg)
                resume(())
            }
        },
        Config {
            get_config(key) -> {
                let env = read_file(".env")
                resume(lookup(env, key))
            }
        }
    }
}

// Testing — mock all effects
let result = handle start_server(3000) with {
    Logger {
        log(_, _) -> resume(()),
    },
    Config {
        get_config(_) -> resume(Some("test-host")),
    },
    IO {
        print(_) -> resume(()),
    }
}
```

---

## 15. Full Example: Generics and Tuples

```flux
fn zip<A, B>(xs: List<A>, ys: List<B>) -> List<(A, B)> {
    match (xs, ys) {
        ([x | xt], [y | yt]) -> [(x, y) | zip(xt, yt)],
        _ -> []
    }
}

fn unzip<A, B>(pairs: List<(A, B)>) -> (List<A>, List<B>) {
    match pairs {
        [(a, b) | rest] -> {
            let (as_, bs) = unzip(rest)
            ([a | as_], [b | bs])
        },
        [] -> ([], [])
    }
}

fn find<T>(xs: List<T>, pred: (T) -> Bool) -> Option<T> {
    match xs {
        [h | t] -> if pred(h) { Some(h) } else { find(t, pred) },
        [] -> None
    }
}

fn group_by<T, K>(xs: List<T>, key_fn: (T) -> K) -> Map<K, List<T>> {
    fold(xs, {}, \(acc, x) -> {
        let k = key_fn(x)
        let group = match get(acc, k) {
            Some(g) -> g,
            None -> []
        }
        put(acc, k, [x | group])
    })
}

fn main() with IO {
    let names = list("Alice", "Bob", "Charlie")
    let scores = list(95, 87, 92)
    let pairs = zip(names, scores)

    let (ns, ss) = unzip(pairs)
    print(ns)
    print(ss)

    let high = find(pairs, \(_, score) -> score >= 90)
    print(high)
}
```

---

## 16. Interaction with Existing Features

| Feature | Impact |
|---|---|
| **Pattern matching** | Exhaustiveness checking becomes type-aware for ADTs/Option/Either/Bool. Tuples add `(a, b)` patterns. |
| **Pipe operator** | Works unchanged. Types flow through the pipe. |
| **Modules** | Module signatures can specify types. Public functions require annotations in `--strict`. |
| **Builtins** | Get typed signatures with effect annotations. Pure builtins callable anywhere, IO builtins require `with IO`. |
| **Closures** | Capture types inferred from context. Effect variables propagate through closures. |
| **Cons lists** | `List<T>` is the typed cons list. `[]` is the empty list, `None` is `Option::None`. |
| **HAMT maps** | `Map<K, V>` with `K: Hashable` constraint (future). |
| **GC heap** | No changes — GC manages `List` and `Map` objects as before. |
| **JIT** | Type information enables monomorphization and unboxed specialization. |

---

## 17. Implementation Phases

### Phase 1: Type Syntax (Parser)
- Add type annotation parsing to `let`, `fn`, and lambda expressions
- Parse generic parameters `<T, U>` on `fn` and `data` declarations
- Parse tuple types `(A, B)` and tuple expressions `(a, b)`
- Parse `type` alias declarations
- Parse `data` ADT declarations
- Parse `effect` declarations
- Parse `with` effect clauses on function signatures
- Parse `handle ... with` expressions
- Parse `fn main` as program entry point
- Parse `[]` as empty list literal (distinct from `None`)
- **No semantic checking** — just parse and store in AST
- All existing programs continue to work

### Phase 2: Type Representation (AST + Compiler)
- Add `Type` enum to AST:
  - Primitives: `TInt`, `TFloat`, `TBool`, `TString`, `TUnit`, `TNever`
  - Containers: `TOption(Box<Type>)`, `TEither(Box<Type>, Box<Type>)`, `TList(Box<Type>)`, `TArray(Box<Type>)`, `TMap(Box<Type>, Box<Type>)`
  - Tuples: `TTuple(Vec<Type>)`
  - Functions: `TFun(Vec<Type>, Box<Type>, Vec<Effect>)`
  - Generics: `TVar(Symbol)`, `TApply(Symbol, Vec<Type>)`
  - Escape: `TAny`
- Add `Effect` enum: `EIO`, `EError(Box<Type>)`, `EState(Box<Type>)`, `ECustom(Symbol, Vec<Type>)`, `EVar(Symbol)`
- Add `Value::Tuple(Rc<Vec<Value>>)` runtime variant
- Store type annotations in AST nodes

### Phase 3: Type Checking
- Implement Hindley-Milner type inference with extensions for effects
- Bidirectional type checking: annotations checked top-down, expressions inferred bottom-up
- Generic instantiation and unification
- `Any -> T` boundary checks (insert runtime casts)
- `Never <: T` subtyping rule
- Report type errors through existing diagnostics system
- New error codes: E300-E399 for type errors

### Phase 4: Effect Checking
- Track effects through function calls (effect set rule)
- Verify `handle` blocks cover all required effects
- Effect inference for unannotated functions (Option B)
- Effect polymorphism: single effect variable `e` for HOFs
- Validate `fn main` as root effect handler
- Compile error for effectful top-level code

### Phase 5: ADTs and Exhaustiveness
- Compile `data` declarations to tagged values
- Exhaustive match checking for ADTs, Bool, Option, Either (v1 scope)
- Constructor arity validation
- Guards treated as non-exhaustive

### Phase 6: Strict Mode
- `--strict` flag requires all public functions to be annotated
- Warn on `Any` types in strict mode
- Full effect tracking enforcement
- Require `fn main` for all programs

---

## 18. Syntax Summary

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
    fn operation(args...) -> ReturnType
}

// Effect handlers
handle expr with {
    EffectName {
        operation(args...) -> resume(value),
    }
}

// Effect polymorphism
fn hof<T, U>(f: (T) -> U with e) -> U with e { ... }

// Entry point
fn main() with IO {
    ...
}
```

---

## 19. Open Questions

1. **Structural vs. nominal ADTs?** — Nominal (as proposed) is simpler and matches Rust/Haskell conventions. Structural typing could be added later for records.

2. **Type classes / traits?** — Not in this proposal. A future proposal could add `trait Eq { fn eq(a, b) -> Bool }` for ad-hoc polymorphism. For now, operator overloading remains dynamic.

3. **Recursive types?** — Needed for tree-like ADTs (`Tree<T>`). Requires careful handling in the type checker to avoid infinite loops.

4. **Effect handler compilation strategy?** — Options include CPS transform, multi-prompt delimited continuations, or evidence passing. Each has different performance tradeoffs. The VM may need new opcodes (`OpResume`, `OpHandle`, `OpPerform`).

5. **Interaction with JIT?** — Type information could feed into JIT specialization. Monomorphized hot paths could skip type checks entirely.

6. **`[]` runtime representation?** — Should `[]` be `Value::EmptyList` (current) or a new sentinel? Current `EmptyList` works fine if we just change the syntax from `None` to `[]` in list contexts.

---

## 20. Prior Art

| Language | Approach | What Flux borrows |
|---|---|---|
| **Koka** | Algebraic effects + Hindley-Milner | Effect syntax, handler semantics, `Never` for divergence |
| **OCaml 5** | Algebraic effects (runtime) | Practical handler design, performance model |
| **Eff** | First-class effects | Clean effect declarations |
| **TypeScript** | Gradual typing | `Any` with runtime boundary checks, migration strategy |
| **Elm** | ML-style + effects as architecture | Effect as explicit boundary, `fn main` as entry point |
| **Unison** | Algebraic effects + content-addressed | Ability syntax inspiration |
| **Rust** | Generics + traits + `!` (never) | Generic syntax `<T>`, `Never` type, monomorphization |

---

## 21. Non-Goals (Explicitly Out of Scope)

- **Dependent types** — too complex for a first type system
- **Linear/affine types** — Rc-based runtime doesn't benefit
- **Type classes / traits** — deferred to a separate proposal
- **Mutable references** — Flux is immutable-first; `State` effect covers mutation
- **Async/await** — could be modeled as an effect later, but not in this proposal
- **Records / named fields** — structs could be a future extension, tuples cover positional data for now
- **Full row polymorphism** — start with single effect variable `e`, extend later
- **Guard exhaustiveness reasoning** — guards are always treated as "may fail" in v1
