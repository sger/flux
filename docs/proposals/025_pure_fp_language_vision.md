# Proposal 025: Pure Functional Language Vision

**Status:** Proposed
**Priority:** High
**Created:** 2026-02-12
**Related:** Proposal 004 (Language Features), Proposal 017 (Persistent Collections & GC), Proposal 009 (Macro System)

## Vision

Flux is a **pure functional programming language** with syntax that feels familiar to developers coming from JavaScript, TypeScript, and Rust. The goal is to make pure FP approachable — no academic jargon, no intimidating syntax, just clean functional code that reads naturally.

**Tagline:** _"Pure FP that doesn't feel alien."_

### Guiding Principles

1. **Pure by default** — Functions have no side effects unless explicitly marked. Referential transparency is the norm.
2. **Expressions everywhere** — Everything returns a value. No statements, no `void`, no `null` surprises.
3. **Immutability is the only option** — No mutable bindings, no mutable data structures. Updates produce new values.
4. **Types work for you** — Hindley-Milner inference means you rarely write type annotations, but the compiler catches errors at compile time.
5. **Familiar syntax** — Braces, arrows, `let`/`fun`, `match` with `=>` — if you know JS or Rust, you can read Flux.
6. **Composition over configuration** — Pipe operator, currying, and small functions compose into complex behavior.

### What "Pure" Means for Flux

| Aspect | Pure FP Rule | Flux Approach |
|--------|-------------|---------------|
| Side effects | Tracked in type system | `with IO` annotation on effectful functions; pure functions are the default |
| Mutability | No mutable state | All `let` bindings are immutable; no `var`, no `mut`, no reassignment |
| Data structures | Persistent/immutable | Structural sharing via `Rc`; update syntax creates new values |
| Null/undefined | No null | `Option` type (`Some`/`None`); no implicit nullability |
| Exceptions | No thrown exceptions | `Result` type (`Ok`/`Err`); effects for recoverable errors |
| Referential transparency | Always | `f(x)` always returns the same value for the same `x` (when pure) |

---

## Syntax Philosophy

### Keep It Familiar

Flux syntax should feel like a blend of **Rust** (types, match, traits) and **JavaScript/TypeScript** (braces, arrows, object literals) with FP ergonomics from **Elm** and **F#** (pipes, inference, no semicolons).

```flux
// This should feel natural to a JS/Rust developer
fun fibonacci(n) {
  match n {
    0 => 0,
    1 => 1,
    n => fibonacci(n - 1) + fibonacci(n - 2),
  }
}

// Pipes read left-to-right like method chains in JS
let result = [1, 2, 3, 4, 5]
  |> filter(\x -> x > 2)
  |> map(\x -> x * x)
  |> fold(0, \acc, x -> acc + x)
```

### Syntax Decisions

| Choice | Flux | Rationale |
|--------|------|-----------|
| Blocks | `{ }` | Familiar to JS/Rust/C developers |
| Lambdas | `\x -> expr` | Short, unambiguous, visually evokes lambda |
| Match arms | `pattern => expr` | Rust-like (currently uses `->`, consider migrating to `=>`) |
| Type annotations | `x: Int` | Rust/TS style, not `x :: Int` (Haskell) |
| Generics | `<T>` | Rust/TS style, not `[T]` or `'a` |
| Pipe | `\|>` | F#/Elm/Elixir standard |
| Function keyword | `fun` | Short, clear (not `fn`, `func`, `def`, or `function`) |
| Comments | `//` and `/* */` | C-family standard |
| String interpolation | `"Hello, #{name}"` | Ruby-style, already implemented |
| No semicolons | Expression-based | Modern language trend, reduces noise |

---

## Phase 1: Type Foundation

The type system is the backbone of a pure FP language. Without it, purity cannot be enforced at compile time.

### 1.1 Algebraic Data Types (ADTs)

Replace the hardcoded `Some`/`None`/`Left`/`Right` with user-defined sum types.

```flux
// Sum types (tagged unions)
type Option<T> = Some(T) | None

type Result<T, E> = Ok(T) | Err(E)

type List<T> = Cons(T, List<T>) | Nil

type Shape
  = Circle(Float)
  | Rectangle(Float, Float)
  | Triangle(Float, Float, Float)

// Construction — just function calls
let shape = Circle(5.0)
let list = Cons(1, Cons(2, Cons(3, Nil)))

// Pattern matching — already works, just needs ADT awareness
fun area(shape) {
  match shape {
    Circle(r) => 3.14159 * r * r,
    Rectangle(w, h) => w * h,
    Triangle(a, b, c) => {
      let s = (a + b + c) / 2.0
      sqrt(s * (s - a) * (s - b) * (s - c))
    },
  }
}
```

**Implementation notes:**
- ADT variants compile to tagged values (tag byte + payload)
- Exhaustiveness checking extends naturally from current pattern matching
- Existing `Some`/`None`/`Left`/`Right` become sugar for built-in ADTs
- Recursive types (like `List<T>`) require `Rc` wrapping internally (already the model)

### 1.2 Record Types

Typed, immutable records to replace untyped hashes for structured data.

```flux
// Record declaration
type User = {
  name: String,
  age: Int,
  active: Bool,
}

// Construction
let user = User { name: "Alice", age: 30, active: true }

// Shorthand when variable names match fields
let name = "Alice"
let age = 30
let user = User { name, age, active: true }

// Access
let n = user.name

// Functional update (creates a new record)
let older = { ...user, age: user.age + 1 }

// Destructuring in let
let { name, age } = user

// Destructuring in function params
fun greet({ name, age }) {
  "Hello #{name}, you are #{age}"
}

// Destructuring in match
match user {
  { active: true, name } => "Active: #{name}",
  { active: false, name } => "Inactive: #{name}",
}
```

**Implementation notes:**
- Records compile to a fixed-layout value with field indices resolved at compile time
- Field access compiles to index lookup (no hash table overhead)
- Update syntax (`...spread`) compiles to a shallow copy with field override

### 1.3 Hindley-Milner Type Inference

The compiler infers types — annotations are optional documentation.

```flux
// No annotations needed — compiler infers everything
let add = \x, y -> x + y          // inferred: (Int, Int) -> Int
let greet = \name -> "Hi #{name}" // inferred: (String) -> String
let id = \x -> x                  // inferred: <T>(T) -> T (polymorphic)

// Optional annotations for documentation or disambiguation
fun factorial(n: Int): Int {
  match n {
    0 => 1,
    n => n * factorial(n - 1),
  }
}

// Type aliases
type UserId = Int
type Predicate<T> = (T) -> Bool
type Transform<A, B> = (A) -> B
```

**Implementation strategy:**
- Algorithm W for type inference (standard HM)
- Unification-based constraint solving
- Let-polymorphism (generalize at `let` bindings)
- Error messages that point to the conflict, not just the usage site
- Gradual rollout: start with inference for expressions, extend to full programs

### 1.4 Traits (Typeclasses)

Polymorphism without OOP — define shared behavior across types.

```flux
// Trait declaration
trait Show {
  fun show(self): String
}

trait Eq {
  fun eq(self, other: Self): Bool
}

trait Ord: Eq {
  fun compare(self, other: Self): Ordering
}

// Implementations
impl Show for User {
  fun show(self) {
    "User(#{self.name}, #{self.age})"
  }
}

impl Eq for User {
  fun eq(self, other) {
    self.name == other.name && self.age == other.age
  }
}

// Trait bounds in functions
fun print_all<T: Show>(items: List<T>) {
  items |> each(\item -> print(show(item)))
}

// Deriving common traits (future)
type Point = { x: Float, y: Float } deriving (Show, Eq)
```

---

## Phase 2: FP Ergonomics

### 2.1 Auto-Currying and Partial Application

Every function is automatically curried. Partial application is natural.

```flux
// All functions are curried
fun add(x, y) { x + y }

let inc = add(1)       // partial application — returns (Int) -> Int
let result = inc(5)    // 6

// Natural with pipes
[1, 2, 3]
  |> map(add(10))      // [11, 12, 13]
  |> filter(gt(11))    // [12, 13]

// Placeholder syntax for non-first-argument partial application
let divide_by_2 = div(_, 2)     // (Int) -> Int
let is_adult = \user -> user.age >= 18

// Currying makes point-free style possible
let process = filter(is_even) >> map(double) >> fold(0, add)
// equivalent to: \xs -> fold(0, add, map(double, filter(is_even, xs)))
```

**Implementation notes:**
- Each function of arity N compiles to a chain of closures
- Optimization: when all arguments are supplied at once, call directly (no intermediate closures)
- The `>>` operator is function composition: `(f >> g)(x) = g(f(x))`

### 2.2 Enhanced Pattern Matching

Build on the existing pattern matching with destructuring and or-patterns.

```flux
// Array destructuring
let [first, second, ...rest] = [1, 2, 3, 4, 5]

// Nested patterns
match data {
  { users: [first, ...rest] } => process(first),
  { users: [] } => "no users",
}

// Or-patterns
match day {
  "Saturday" | "Sunday" => "weekend",
  _ => "weekday",
}

// If-let for single-pattern checks
if let Some(user) = find_user(id) {
  greet(user)
} else {
  "not found"
}

// Let-else for early returns
fun process(data) {
  let Ok(valid) = validate(data) else {
    return Err("invalid")
  }
  transform(valid)
}
```

### 2.3 List Comprehensions

Concise syntax for transforming collections.

```flux
// Basic
let doubled = [x * 2 for x in numbers]

// With filter
let adults = [user for user in users if user.age >= 18]

// Multiple generators (cartesian product)
let pairs = [(x, y) for x in xs for y in ys]

// With pattern matching
let names = [name for { name, active: true } in users]

// Hash comprehension
let lookup = { user.id: user for user in users }
```

### 2.4 Where Clauses

Let complex expressions read top-down.

```flux
fun bmi_category(weight, height) {
  classify(index)
  where index = weight / (height * height)
  where classify = \i -> match i {
    i if i < 18.5 => "underweight",
    i if i < 25.0 => "normal",
    _ => "overweight",
  }
}

// Multiple where bindings
fun distance(p1, p2) {
  sqrt(dx * dx + dy * dy)
  where dx = p2.x - p1.x
  where dy = p2.y - p1.y
}
```

---

## Phase 3: Purity and Effects

### 3.1 Effect System

Pure functions are the default. Side effects are tracked in the type signature.

```flux
// Pure function — no annotation needed
fun add(x, y) { x + y }

// Effectful function — must declare effects
fun greet(name) with IO {
  print("Hello, #{name}!")
}

// Multiple effects
fun fetch_data(url) with IO, Fail<HttpError> {
  let response = http_get(url)
  match response.status {
    200 => Ok(response.body),
    code => fail(HttpError { code, message: response.body }),
  }
}

// Effects are inferred — you can omit the annotation
// and the compiler will tell you what effects a function has
fun process(data) {       // compiler infers: with IO
  print("Processing...")
  transform(data)
}

// Pure functions cannot call effectful functions
fun pure_calc(x) {
  print(x)  // Compile error: `print` requires IO effect,
             // but `pure_calc` is pure
}
```

**Built-in effects:**

| Effect | Purpose | Example operations |
|--------|---------|-------------------|
| `IO` | Console, file, network | `print`, `read_file`, `http_get` |
| `Fail<E>` | Recoverable errors | `fail(err)`, `try { ... }` |
| `Async` | Asynchronous operations | `await`, `spawn` |
| `Random` | Non-determinism | `random_int`, `random_float` |

**Effect handlers** (advanced, later phase):

```flux
// Handle an effect to provide an implementation
let result = handle fetch_data("http://example.com") {
  IO.http_get(url) => resume({ status: 200, body: "mock data" }),
}

// Great for testing — mock effects without dependency injection
fun test_fetch() {
  let result = handle fetch_data("http://api.com") {
    IO.http_get(_) => resume({ status: 200, body: "{\"name\": \"test\"}" }),
  }
  assert(result == Ok("{\"name\": \"test\"}"))
}
```

### 3.2 Result-Based Error Handling

No exceptions. Errors are values.

```flux
// The ? operator propagates errors (Rust-style)
fun load_config(path) with Fail<ConfigError> {
  let content = read_file(path)?
  let parsed = parse_toml(content)?
  validate_config(parsed)?
}

// Match on results
match load_config("app.toml") {
  Ok(config) => start_app(config),
  Err(FileNotFound(path)) => print("Missing: #{path}"),
  Err(ParseError(line, msg)) => print("Parse error at #{line}: #{msg}"),
  Err(ValidationError(field)) => print("Invalid field: #{field}"),
}

// Pipeline-style error handling
load_config("app.toml")
  |> map_err(\e -> "Config failed: #{show(e)}")
  |> unwrap_or(default_config)
  |> start_app
```

---

## Phase 4: Data Structures and Performance

### 4.1 Persistent Data Structures

Immutable data structures with structural sharing for efficient updates.

```flux
// Persistent vector — O(log32 N) access and update
let v = [1, 2, 3, 4, 5]
let v2 = set(v, 2, 99)    // [1, 2, 99, 4, 5] — v is unchanged

// Persistent hash map — O(log32 N) operations
let m = { "a": 1, "b": 2 }
let m2 = put(m, "c", 3)   // m is unchanged

// These are already the semantics (Rc-based), but internal representation
// should move to HAMTs or RRB trees for better performance on large collections
```

### 4.2 Lazy Evaluation (Opt-in)

Strict by default, lazy when you ask for it.

```flux
// Lazy sequences for infinite data
let naturals = lazy_seq(0, \n -> n + 1)
let evens = naturals |> filter(is_even) |> take(10)

// Lazy values
let expensive = lazy { heavy_computation() }
let value = force(expensive)  // computed on first access, cached

// Ranges are lazy by default
let r = 1..1000000  // no allocation
r |> filter(is_prime) |> take(10)  // only computes what's needed
```

---

## Implementation Roadmap

### Stage 1: Type Foundation (Critical Path)

| Step | Feature | Depends On | Effort |
|------|---------|-----------|--------|
| 1.1 | ADTs (user-defined sum types) | — | Large |
| 1.2 | Record types | — | Large |
| 1.3 | Type inference (HM Algorithm W) | ADTs, Records | Very Large |
| 1.4 | Migrate `Some`/`None`/`Left`/`Right` to ADTs | ADTs | Medium |

**Milestone:** Flux programs are statically typed with full inference.

### Stage 2: FP Ergonomics

| Step | Feature | Depends On | Effort |
|------|---------|-----------|--------|
| 2.1 | Auto-currying | Type inference | Medium |
| 2.2 | Function composition `>>` | — | Small |
| 2.3 | Enhanced destructuring (`...rest`, nested) | Pattern matching | Medium |
| 2.4 | Or-patterns | Pattern matching | Medium |
| 2.5 | If-let / let-else | Pattern matching | Medium |
| 2.6 | List comprehensions | — | Medium |
| 2.7 | Where clauses | — | Small |

**Milestone:** Flux feels ergonomic and expressive for everyday FP.

### Stage 3: Purity Enforcement

| Step | Feature | Depends On | Effort |
|------|---------|-----------|--------|
| 3.1 | Effect annotations (`with IO`) | Type inference | Large |
| 3.2 | Effect inference | Effect annotations | Large |
| 3.3 | `?` operator for Result/Fail | ADTs, Effects | Medium |
| 3.4 | Effect handlers (basic) | Effects | Very Large |

**Milestone:** Purity is enforced by the compiler. Side effects are explicit.

### Stage 4: Performance and Scale

| Step | Feature | Depends On | Effort |
|------|---------|-----------|--------|
| 4.1 | Persistent vectors (RRB tree) | — | Large |
| 4.2 | Persistent hash maps (HAMT) | — | Large |
| 4.3 | Lazy sequences | — | Medium |
| 4.4 | Bytecode optimization passes (Proposal 023) | — | Large |
| 4.5 | Trait system | Type inference | Very Large |

**Milestone:** Flux is performant enough for real-world programs.

### Stage 5: Ecosystem

| Step | Feature | Depends On | Effort |
|------|---------|-----------|--------|
| 5.1 | Standard library (modules, not builtins) | Traits, Modules | Large |
| 5.2 | Package manager (Proposal 015) | Module system | Very Large |
| 5.3 | LSP (language server) | Type inference | Very Large |
| 5.4 | REPL | — | Medium |
| 5.5 | Formatter (full, not just indent) | — | Medium |

**Milestone:** Flux has a usable ecosystem for building real projects.

---

## What Flux Is NOT

To keep the vision focused, Flux intentionally avoids:

| Anti-feature | Why |
|-------------|-----|
| OOP (classes, inheritance) | Composition over inheritance; traits for polymorphism |
| Null/undefined | `Option` type handles absence explicitly |
| Exceptions | `Result` type and effect system for errors |
| Implicit mutation | All data is immutable; updates return new values |
| Complex build systems | Single binary compiler; `flux build` just works |
| Heavy runtime | Minimal VM; no GC needed (Rc + immutability = no cycles) |
| Macro-heavy metaprogramming | Prefer clear code over clever macros (macros are opt-in, hygienic) |

---

## Comparison with Other Languages

| Feature | Flux | Elm | Haskell | Rust | TypeScript |
|---------|------|-----|---------|------|-----------|
| Purity | Enforced (effects) | Enforced (Cmd/Sub) | Enforced (IO monad) | Not enforced | Not enforced |
| Syntax familiarity | JS/Rust-like | Haskell-like | Haskell | Rust | JS/Java-like |
| Type inference | Full HM | Full HM | Full HM + extensions | Partial | Partial |
| Null safety | Option type | Maybe type | Maybe type | Option type | Optional chaining |
| Error handling | Result + effects | Result + Cmd | Either + IO | Result + ? | try/catch |
| Immutability | Always | Always | Always | Opt-in (`mut`) | Opt-in (`const`) |
| Pattern matching | Full + guards | Full | Full + extensions | Full + guards | Limited |
| Currying | Auto | Auto | Auto | Manual | Manual |
| Learning curve | Low | Medium | High | High | Low |

---

## Example: What Idiomatic Flux Looks Like

```flux
// A complete, idiomatic Flux program

type Todo = {
  id: Int,
  title: String,
  done: Bool,
}

type Filter = All | Active | Completed

fun create(id, title) {
  Todo { id, title, done: false }
}

fun toggle(todo) {
  { ...todo, done: !todo.done }
}

fun visible(todos, filter) {
  match filter {
    All => todos,
    Active => todos |> filter(\t -> !t.done),
    Completed => todos |> filter(\t -> t.done),
  }
}

fun summary(todos) {
  let total = len(todos)
  let done = todos |> filter(\t -> t.done) |> len
  "#{done}/#{total} completed"
}

fun main() with IO {
  let todos = [
    create(1, "Learn Flux"),
    create(2, "Build something"),
    create(3, "Share it"),
  ]

  let updated = todos
    |> map(\t -> if t.id == 1 { toggle(t) } else { t })

  updated
    |> visible(Active)
    |> each(\t -> print("[ ] #{t.title}"))

  print(summary(updated))
}
```

---

## Open Questions

1. **Match arm syntax**: Keep `->` or migrate to `=>`? The `=>` is more Rust-like, but `->` is already established in the codebase.

2. **Semicolons**: Currently expression-based but some semicolons exist. Fully remove them?

3. **Effect system complexity**: Start with just `IO` and `Fail`, or design the full algebraic effect system upfront?

4. **Currying performance**: Auto-currying has overhead. Accept it, or use a hybrid approach (curry only when partially applied)?

5. **Backward compatibility**: How to migrate existing `.flx` programs as the language evolves? Versioned syntax (`edition` like Rust)?

6. **Standard library scope**: Minimal (like Go) or batteries-included (like Python)?

---

## References

### Inspiration Languages
- **Elm** — Pure FP with great error messages and beginner-friendly design
- **Gleam** — Friendly syntax, runs on BEAM, no footguns
- **F#** — Pipe operator, pragmatic FP on .NET
- **Rust** — Pattern matching, traits, Result type, great tooling
- **Koka** — Algebraic effects done right
- **Roc** — Fast pure FP with friendly syntax (spiritual sibling)

### Papers
- Damas & Milner, "Principal type-schemes for functional programs" (1982) — HM inference
- Plotkin & Pretnar, "Handlers of Algebraic Effects" (2009) — Effect handlers
- Bagwell, "Ideal Hash Trees" (2001) — HAMT for persistent maps
