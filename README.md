# Flux

> A functional language with Rust's structure, JS's familiarity, and FP's power.

Flux is a small, expressive, purely functional language with two execution backends: a stack-based **bytecode VM** and a **Cranelift JIT** that compiles to native machine code. Written in Rust.

```flux
fn greet(name) { "Hello, #{name}!" }

let names = ["Alice", "Bob", "Charlie"]
let result = names
    |> filter(\n -> len(n) > 3)
    |> map(\n -> greet(n))

print(result)  // ["Hello, Alice!", "Hello, Charlie!"]
```

---

## Features

- **Two execution backends** — Bytecode VM for portability; Cranelift JIT for native-speed execution
- **Functional core** — Immutable `let` bindings, first-class functions, closures, higher-order builtins
- **Pattern matching** — `match` on literals, `Some`/`None`, `Left`/`Right`, cons cells `[h | t]`, with guards
- **List comprehensions** — `[x * 2 | x <- xs, x > 0]` desugared at parse time to `map`/`filter`/`flat_map`
- **Persistent collections** — Cons lists and HAMT hash maps with structural sharing; GC-managed
- **Pipe operator** — `a |> f(b)` desugars to `f(a, b)` — natural left-to-right data pipelines
- **Modules** — Static qualified namespaces, Haskell-style imports, alias, cycle detection
- **String interpolation** — `"Hello #{name}, result is #{1 + 2}"`
- **Diagnostics** — Elm-style errors with stable codes (`E101`, `W200`), source snippets, inline suggestions
- **Tooling** — Linter, formatter, bytecode inspector, free-variable analyzer, tail-call detector, `--stats`
- **Bytecode cache** — `.fxc` files with dependency-aware invalidation
- **GC** — Mark-and-sweep for persistent data structures, configurable threshold, optional telemetry
- **Editor support** — VS Code and Zed syntax highlighting

---

## Quick Start

**Build:**
```bash
cargo build
cargo build --features jit   # with Cranelift JIT backend
```

**Run a program:**
```bash
cargo run -- examples/basics/print.flx
cargo run -- --root=examples examples/advanced/grade_analyzer.flx
cargo run --features jit -- examples/basics/fibonacci.flx --jit
```

**Helper script** (sets module roots automatically):
```bash
scripts/run_examples.sh basics/pipe_operator.flx
scripts/run_examples.sh --all   # run all non-error examples
```

---

## Language Tour

### Variables and Functions

All bindings are immutable. There is no `var`, `mut`, or reassignment.

```flux
let x = 42
let name = "Flux"

fn add(x, y) { x + y }
fn double(x) { x * 2 }

let triple = \x -> x * 3      // lambda
let add5   = \(a, b) -> a + b // multi-param lambda

print(add(3, 4))    // 7
print(triple(5))    // 15
```

### Pipe Operator

`a |> f(b)` desugars to `f(a, b)` at parse time. Chain transforms naturally:

```flux
let result = [1, 2, 3, 4, 5, 6]
    |> filter(\x -> x % 2 == 0)
    |> map(\x -> x * x)
    |> fold(0, \(acc, x) -> acc + x)

print(result)  // 56
```

### Pattern Matching

```flux
fn describe(value) {
    match value {
        0       -> "zero",
        Some(x) -> "got: " + to_string(x),
        None    -> "nothing",
        Left(e) -> "error: " + e,
        Right(v)-> "ok: " + to_string(v),
        _       -> "other",
    }
}
```

With guards:

```flux
fn grade(score) {
    match score {
        n if n >= 90 -> "A",
        n if n >= 80 -> "B",
        n if n >= 70 -> "C",
        _            -> "F",
    }
}
```

### List Comprehensions

Desugared at parse time — no VM changes needed.

```flux
let xs = [1, 2, 3, 4, 5]

// Single generator
let doubled = [x * 2 | x <- xs]                // map(xs, \x -> x * 2)

// With guard
let evens = [x | x <- xs, x % 2 == 0]          // map(filter(...), \x -> x)

// Multiple generators (uses flat_map)
let pairs = [(x, y) | x <- xs, y <- xs, x < y] // all pairs where x < y

print(doubled)  // [2, 4, 6, 8, 10]
print(pairs)    // [(1, 2), (1, 3), ...]
```

### Collections

**Cons lists** — immutable linked lists, GC-managed, O(1) prepend:
```flux
let nums = [1, 2, 3, 4]     // list literal
let more = [0 | nums]       // prepend: [0, 1, 2, 3, 4]

fn sum(lst) {
    match lst {
        [h | t] -> h + sum(t),
        _       -> 0,
    }
}

print(sum(nums))  // 10
```

**Arrays** — mutable-style interface, `Rc`-backed:
```flux
let scores = [|10, 20, 30|]                     // array literal (use [| |] to distinguish from lists)
let extended = concat(scores, [|40, 50|])
print(slice(scores, 1, 3))  // [|20, 30|]
```

**Hash maps** — persistent HAMT, structural sharing on update:
```flux
let user    = {"name": "Alice", "age": 30}
let updated = put(user, "email", "alice@example.com")

print(get(user,    "name"))   // Some(Alice)
print(get(updated, "email"))  // Some(alice@example.com)
print(get(user,    "email"))  // None  (original unchanged)
```

### Tuples

```flux
let point = (3, 4)
let x = point.0    // 3
let y = point.1    // 4

let (a, b) = point  // destructuring in let

fn swap((a, b)) { (b, a) }
print(swap((1, 2)))  // (2, 1)
```

### Do-Blocks

Sequential expressions in a single block — last expression is the value:

```flux
let result = do {
    let x = 10;
    let y = x * 2;
    y + 5
}
print(result)  // 25
```

### Closures

```flux
fn make_adder(n) {
    \x -> x + n   // captures n
}

let add10 = make_adder(10)
print(add10(5))   // 15
print(add10(20))  // 30
```

### String Interpolation

```flux
let name  = "Flux"
let score = 99

print("Language: #{name}")                // Language: Flux
print("Score: #{score}, Grade: #{grade(score)}")  // Score: 99, Grade: A
print("2 + 2 = #{2 + 2}")                // 2 + 2 = 4
```

### Modules

```flux
// examples/Modules/Math.flx
module Modules.Math {
    fn square(x)   { x * x }
    fn cube(x)     { x * x * x }
    fn _helper(x)  { x }  // private: underscore prefix
}

// main.flx
import Modules.Math as M

print(M.square(5))  // 25
print(M.cube(3))    // 27
```

Module names must be PascalCase. Import cycles are detected at compile time (error `E035`). Forward references within a module are allowed.

---

## Two Execution Backends

Flux ships with two backends that share the same AST and front-end pipeline.

### Bytecode VM (default)

The default backend compiles to a compact stack-based instruction set (~45 opcodes) and executes in a Rust VM with call frames and closures.

```bash
cargo run -- examples/basics/fibonacci.flx
```

### Cranelift JIT

The `--jit` flag compiles the AST directly to native machine code via [Cranelift](https://cranelift.dev/). No interpreter overhead.

```bash
cargo run --features jit -- examples/basics/fibonacci.flx --jit
```

Both backends use the same builtin function table, GC heap, and persistent collection library — new builtins are automatically available in both.

---

## Execution Analytics (`--stats`)

Pass `--stats` to get a breakdown of each compilation and execution phase:

```
$ cargo run -- --root=examples examples/advanced/using_list_module.flx --stats

  ── Flux Analytics ───────────────────────────
  parse                    1.54 ms
  compile                  0.31 ms  [bytecode]
  execute                  0.19 ms  [vm]
  total                    2.04 ms

  modules                     2
  source lines              119
  globals                    20
  functions                  19
  instructions              529 bytes
  ────────────────────────────────────────────
```

JIT output shows `jit compile [cranelift]` and `execute [native]` instead. The cache path shows `compile (cached)`.

---

## Diagnostics

Flux produces structured, phase-aware diagnostics with stable error codes:

```
--> compiler error[E101]: UNKNOWN KEYWORD

Flux uses `fn` for function declarations.

  --> examples/basics/hello.flx:1:1
  |
1 | fun greet(name) {
  | ^^^
  |
help: Replace 'fun' with 'fn'
   |
1 | fn greet(name) {
  | ~~~
```

- Errors grouped by file, sorted by line/column/severity
- Inline suggestions and did-you-mean hints
- Runtime errors include stack traces
- Stable error code catalog: `E1xx` (parser), `E2xx–E9xx` (compiler), `E10xx` (runtime), `W2xx` (warnings)
- Set `NO_COLOR=1` to disable ANSI output

---

## CLI Reference

**Subcommands:**
```
flux <file.flx>                        Run a Flux program
flux run <file.flx>                    Run (explicit)
flux tokens <file.flx>                 Show token stream
flux bytecode <file.flx>               Show compiled bytecode
flux lint <file.flx>                   Run linter
flux fmt [--check] <file.flx>          Format source (or check only)
flux cache-info <file.flx>             Inspect bytecode cache for a source file
flux cache-info-file <file.fxc>        Inspect a .fxc cache file directly
flux analyze-free-vars <file.flx>      Show free variable analysis
flux analyze-tail-calls <file.flx>     Show tail-call sites
```

**Flags:**

| Flag | VM | JIT | Description |
|------|----|-----|-------------|
| `--stats` | Yes | Yes | Print timing and code metrics after run |
| `--jit` | — | — | Use JIT backend (requires `--features jit`) |
| `--verbose` | Yes | Yes | Show cache hit/miss/store status |
| `--trace` | Yes | No | Print VM instruction trace |
| `--optimize`, `-O` | Yes | Yes | AST optimizations (desugar + constant fold) |
| `--analyze`, `-A` | Yes | Yes | Analysis passes (free vars + tail calls) |
| `--root <path>` | Yes | Yes | Add module search root (repeatable; `--root=path` also works) |
| `--roots-only` | Yes | Yes | Only use explicit `--root` values, skip defaults |
| `--no-cache` | Yes | Yes | Disable bytecode cache |
| `--max-errors <n>` | Yes | Yes | Limit displayed errors (default: 50) |
| `--no-gc` | Yes | Yes | Disable garbage collection |
| `--gc-threshold <n>` | Yes | Yes | Set GC collection threshold (default: 10,000) |
| `--gc-telemetry` | Yes | Yes | Print GC stats after run (requires `--features gc-telemetry`) |
| `--leak-detector` | Yes | Yes | Print allocation stats on exit |

---

## Builtin Functions

All builtins are available without imports:

| Category | Functions |
|----------|-----------|
| **I/O** | `print`, `to_string` |
| **Arithmetic** | `abs`, `floor`, `ceil`, `round`, `sqrt`, `pow`, `min`, `max`, `clamp` |
| **Strings** | `len`, `split`, `join`, `trim`, `upper`, `lower`, `chars`, `contains`, `substring`, `starts_with`, `ends_with`, `replace`, `split_ints` |
| **Arrays** | `push`, `pop`, `concat`, `reverse`, `slice`, `sort`, `flatten`, `unique`, `index_of` |
| **Collections** | `map`, `filter`, `fold`, `flat_map`, `each`, `zip`, `find`, `any`, `all`, `count` |
| **Lists** | `list`, `to_list`, `to_array`, `first`, `last`, `tail`, `take`, `drop` |
| **Hash maps** | `put`, `get`, `delete`, `keys`, `values`, `entries`, `merge`, `has_key` |
| **Type checks** | `is_int`, `is_float`, `is_string`, `is_bool`, `is_none`, `is_some`, `is_list`, `is_array`, `is_map` |
| **Tuples** | `fst`, `snd` |
| **Option/Either** | `unwrap`, `unwrap_or` |

---

## Project Layout

```
src/
  syntax/       Lexer, parser, string interner, module graph, linter, formatter
  ast/          AST transforms: constant folding, desugaring, free vars, tail calls
  bytecode/     Bytecode compiler, opcodes, symbol tables, .fxc cache
  runtime/
    vm/         Stack-based VM, instruction dispatch, tracing
    builtins/   Built-in functions (array, string, hash, numeric, list ops)
    gc/         Mark-and-sweep GC, HAMT persistent maps, telemetry
  jit/          Cranelift JIT backend (feature-gated): IR generation, runtime helpers, value arena
  diagnostics/  Error types, rendering, builder pattern, aggregation, registry
examples/       ~170 Flux programs organized by category
tests/          ~50 integration test files + snapshot tests (insta)
benches/        7 Criterion benchmark suites
tools/          VS Code and Zed syntax highlighting extensions
docs/           Architecture notes, language design, proposals
```

**Pipeline:**
```
Source (.flx)
    │
    ▼
  Lexer          token stream
    │
    ▼
  Parser         AST (string-interned identifiers)
    │
    ▼
  AST Passes     constant folding · desugaring · free var collection · tail call detection
    │
    ├──────────────────────────────────────┐
    ▼                                      ▼
  Bytecode Compiler                   Cranelift JIT
  .fxc cache                          native machine code
    │                                      │
    ▼                                      ▼
  Stack VM                            Native Execution
    │                                      │
    └──────────────────────────────────────┘
                    │
                    ▼
              GC Heap (cons lists · HAMT maps)
```

---

## Development

### Tests

```bash
cargo test                                # full suite (~560 tests)
cargo test test_builtin_len               # single test by name
cargo test --test parser_tests            # specific test file
cargo test --features gc-telemetry        # include telemetry tests
```

Snapshot tests use [insta](https://insta.rs). After changes that affect output:

```bash
cargo insta review
```

### Formatting and Linting

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

### Benchmarks

```bash
cargo bench --bench lexer_bench
cargo bench --bench hamt_bench
cargo bench --bench map_filter_fold_bench
```

---

## Editor Support

### VS Code

```bash
python3 tools/vscode-flux/scripts/build-vsix.py
```

Install `tools/vscode-flux/dist/flux-language-0.0.1.vsix` via **Extensions → Install from VSIX**.

### Zed

Open Command Palette → **Extensions: Install Dev Extension** → select `tools/zed-flux`.

---

## Roadmap

Derived from [docs/proposals/](docs/proposals/):

- **Tail-call optimization** — Stack reuse for self-recursive functions
- **Type system** — Algebraic data types, traits, type inference, generics
- **Effect system** — Algebraic effects and handlers for IO, async, failure
- **Concurrency** — Async/await, actor model
- **Standard library** — Extracted stdlib modules
- **Tree-sitter grammar** — Full syntax highlighting for all editors

---

## Contributing

1. Fork and create a feature branch
2. Ensure `cargo test`, `cargo fmt --all -- --check`, and `cargo clippy --all-targets --all-features -- -D warnings` all pass
3. Add tests for new functionality; use snapshot tests for output-sensitive changes
4. Keep commits focused; open a pull request against `main`

For architecture details, see [docs/architecture/](docs/architecture/). Language design and proposals live in [docs/proposals/](docs/proposals/).
