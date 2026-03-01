# Flux

Flux is a functional language written in Rust with two execution backends: a stack-based **bytecode VM** and a **Cranelift JIT** that compiles to native machine code. It started as a learning project — inspired by [Writing a Compiler in Go](https://compilerbook.com/), [Crafting Interpreters](https://craftinginterpreters.com/), [Rust for Rustaceans](https://nostarch.com/rust-rustaceans), *Virtual Machines* by Iain D. Craig, and [Koka](https://koka-lang.github.io/koka/doc/index.html) for its approach to algebraic effects and effect rows.

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

- **Two execution backends** — Bytecode VM for portability; Cranelift JIT for native-speed execution. Both share the same Base function registry, GC heap, and collection library.
- **Functional core** — Immutable `let` bindings, first-class functions, closures, higher-order Base functions
- **Gradual type system** — Optional type annotations with Hindley-Milner inference. Unannotated code infers as `Any`; typed paths are statically checked at compile time.
- **Algebraic effects** — Declare custom effects (`effect Console { ... }`), perform operations (`perform Console.print(...)`), and discharge them with `handle`. Built-in `IO` and `Time` effects enforced statically.
- **Pure-by-default** — Typed functions are pure unless they carry a `with ...` effect annotation. Effectful top-level code is rejected (`E413`/`E414`); effectful execution belongs in `fn main() with IO { ... }`.
- **Effect polymorphism** — `with e` row variables propagate callback effects through higher-order wrappers without losing purity guarantees.
- **Strict mode** — `--strict` enforces fully-annotated `public fn` boundaries: parameter types, return types, effect sets. Flags `Any` leaking into exported APIs (`E423`).
- **ADTs** — User-defined nominal algebraic data types: `type Shape = Circle(Float) | Rect(Float, Float)`. Generic ADTs with `<T>` parameters. Constructor-space exhaustiveness checking (`E083`).
- **Pattern matching** — `match` on literals, `Some`/`None`, `Left`/`Right`, cons cells `[h | t]`, tuples, ADT constructors, with guards. Non-exhaustive matches diagnosed at compile time (`E015`/`E083`).
- **List comprehensions** — `[x * 2 | x <- xs, x > 0]` desugared at parse time to `map`/`filter`/`flat_map`
- **Persistent collections** — GC-managed cons lists and HAMT hash maps with structural sharing
- **Pipe operator** — `a |> f(b)` desugars to `f(a, b)` — natural left-to-right data pipelines
- **Modules** — Static qualified namespaces, imports with aliases, `public fn` exported boundaries, cycle detection at compile time
- **String interpolation** — `"Hello #{name}, result is #{1 + 2}"`
- **Unit testing** — Built-in `--test` runner, `test_*` discovery, assert functions, `Flow.FTest` stdlib
- **Diagnostics** — Elm-style errors with stable codes (`E030`, `W200`, `E300`, `E400`), source snippets, inline suggestions
- **Tooling** — Linter, formatter, bytecode inspector, free-variable analyzer, tail-call detector, `--stats`
- **Bytecode cache** — `.fxc` files with SHA-2 hash–based dependency-aware invalidation
- **GC** — Mark-and-sweep for persistent data structures, configurable threshold, optional telemetry
- **Editor support** — VS Code syntax highlighting (`tools/vscode/`)

---

## Quick Start

**Build:**
```bash
cargo build
cargo build --features jit        # with Cranelift JIT backend
cargo build --profile dev-fast    # opt-level 3 with lighter debug info
```

**Run a program:**
```bash
cargo run -- examples/basics/print.flx
cargo run -- --root examples/ examples/advanced/grade_analyzer.flx
cargo run --features jit -- examples/basics/fibonacci.flx --jit
```

**Helper script** (sets module roots automatically):
```bash
scripts/run_examples.sh basics/pipe_operator.flx
scripts/run_examples.sh --all          # run all non-error examples (VM)
scripts/run_examples.sh --all --jit    # run all using JIT backend
```

---

## Backend Parity

| Capability | VM | JIT | Notes |
|---|---|---|---|
| Run Flux programs | Yes | Yes | JIT requires `cargo build/run --features jit` and `--jit` flag |
| Base functions | Yes | Yes | Shared Base function registry/runtime behavior |
| Unit test runner (`--test`) | Yes | Yes | Same `test_*` discovery and assertions |
| AST optimizations (`-O`) | Yes | Yes | Shared optimization pipeline |
| Analysis passes (`-A`) | Yes | Yes | Shared analysis pipeline |
| GC / persistent collections | Yes | Yes | Shared runtime heap/collection model |
| Bytecode cache | Yes | Partial | `--jit` bypasses VM cache-hit execution path |
| VM instruction trace (`--trace`) | Yes | No | VM-only debugging feature |

---

## Version Feature Matrix

| Feature | v0.0.1 | v0.0.2 | v0.0.3 | v0.0.4 |
|---|---|---|---|---|
| Bytecode VM backend | Yes | Yes | Yes | Yes |
| Cranelift JIT backend | No | No | Yes | Yes |
| Pattern matching (core) | Yes | Yes | Yes | Yes |
| Pattern guards | No | Yes | Yes | Yes |
| Cons list + tuple patterns | No | Partial | Yes | Yes |
| Persistent cons lists | No | No | Yes | Yes |
| HAMT hash maps | No | No | Yes | Yes |
| Tuples | No | No | Yes | Yes |
| Do-blocks | No | No | Yes | Yes |
| Where clauses | No | No | Yes | Yes |
| List comprehensions | No | No | Yes | Yes |
| Base function count (approx) | 20+ | 35 | 75+ | 77 |
| Built-in test runner (`--test`) | No | No | Yes | Yes |
| Bytecode cache | Yes | Yes | Yes | Yes |
| Type annotations (let + fn) | No | No | Partial | Yes |
| Hindley-Milner type inference | No | No | No | Yes |
| Gradual typing (`Any`) | No | No | No | Yes |
| Algebraic effects (declare/perform/handle) | No | No | No | Yes |
| Pure-by-default enforcement | No | No | No | Yes |
| Effect polymorphism (`with e`) | No | No | No | Yes |
| User-defined ADTs | No | No | No | Yes |
| Generic ADTs (`<T>`) | No | No | No | Yes |
| Exhaustiveness checking (ADT + general) | No | No | Partial | Yes |
| Strict mode (`--strict`) | No | No | No | Yes |
| Public API boundary enforcement | No | No | No | Yes |

Release notes:
- `v0.0.1`: `docs/versions/whats_new_v0.0.1.md`
- `v0.0.2`: `docs/versions/whats_new_v0.0.2.md`
- `v0.0.3`: `docs/versions/whats_new_v0.0.3.md`

---

## Language Tour

A quick taste. The full [Manual](#documentation) has chapter-by-chapter coverage.

### Pattern Matching with Guards

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

Pattern matching works on literals, `Some`/`None`, `Left`/`Right`, cons cells `[h | t]`, tuples, and wildcards.

### Pipes and Comprehensions

```flux
// Pipe: a |> f(b) desugars to f(a, b) at parse time
let total = [1, 2, 3, 4, 5, 6]
    |> filter(\x -> x % 2 == 0)
    |> map(\x -> x * x)
    |> fold(0, \(acc, x) -> acc + x)

// List comprehension — desugared to map/filter/flat_map at parse time
let pairs = [(x, y) | x <- [1,2,3], y <- [1,2,3], x < y]
```

### Persistent Collections

Three collection types with distinct semantics:

```flux
// Cons lists — GC-managed, immutable, O(1) prepend, [h | t] pattern
let nums = [1, 2, 3]
let more = [0 | nums]          // [0, 1, 2, 3] — original unchanged

// Hash maps — HAMT, structural sharing on every update
let user    = {"name": "Alice", "age": 30}
let updated = put(user, "email", "alice@example.com")
print(get(user, "email"))       // None  (original unchanged)

// Arrays — Rc-backed, [| |] syntax, O(1) indexed access
let scores = [|10, 20, 30|]
print(slice(scores, 1, 3))      // [|20, 30|]
```

### Do-blocks and Where Clauses

```flux
// do-block: sequential expressions, last is the value
let result = do {
    let x = 10;
    let y = x * 2;
    y + 5
}

// where: local bindings scoped after the body expression
fn hypotenuse(a, b) {
    sqrt(sq_a + sq_b)
    where sq_a = a * a
    where sq_b = b * b
}
```

### Modules

```flux
// examples/Modules/Math.flx
module Modules.Math {
    fn square(x)  { x * x }
    fn _helper(x) { x }   // private: underscore prefix
}

// main.flx
import Modules.Math as M
print(M.square(5))   // 25
```

Module names must be PascalCase. Import cycles are detected at compile time (error `E021`).

### Type Annotations and HM Inference

Type annotations are optional. Hindley-Milner inference fills types for unannotated code. Annotated paths are statically validated.

```flux
// Fully typed — statically checked
fn add(x: Int, y: Int) -> Int { x + y }

// Inferred — HM resolves types from usage
fn double(x) { x * 2 }

// Typed let binding
let total: Int = add(10, 32)
```

Unannotated functions that can't be inferred remain `Any`, enabling incremental adoption.

### Algebraic Effects

Declare custom effects, perform operations, and discharge them with handlers:

```flux
effect Console {
    fn print(msg: String) -> Unit
}

fn greet(name: String) with Console {
    perform Console.print("Hello, #{name}!")
}

fn main() with IO {
    greet("Alice") handle Console {
        print(msg) -> print(msg)
    }
}
```

Built-in effects `IO` and `Time` are enforced statically — calling `print` or `read_file` in a pure context is a compile error (`E400`).

### Pure-by-Default

Typed functions are pure unless they carry a `with` effect annotation. Effectful code at the top level is rejected:

```flux
// Error E413 — effectful top-level expression
print("this is rejected")

// Correct: effectful execution inside main
fn main() with IO {
    print("Hello!")
}
```

### ADTs and Exhaustiveness

User-defined algebraic data types with exhaustiveness checking:

```flux
type Shape = Circle(Float) | Rect(Float, Float)

fn area(s: Shape) -> Float {
    match s {
        Circle(r)    -> 3.14159 * r * r,
        Rect(w, h)   -> w * h,
    }
}
```

Missing constructors in a `match` are a compile error (`E083`). Generic ADTs (`type Tree<T> = Leaf | Node(T, Tree<T>, Tree<T>)`) are also supported.

### Strict Mode

`--strict` enforces fully-annotated `public fn` boundaries — parameter types, return types, and effect sets. Useful for library APIs:

```flux
// In strict mode: E416 if param type missing, E417 if return type missing
public fn compute(x: Int, y: Int) -> Int { x + y }
```

---

## Diagnostics

Flux produces structured, phase-aware diagnostics with stable error codes:

```
--> compiler error[E030]: UNKNOWN KEYWORD

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
- Stable error code catalog: `E001–E077` (compiler/parser), `E300–E301` (type system), `E400–E425` (effects/purity/strict), `E1000–E1021` (runtime), `W200+` (warnings)
- Set `NO_COLOR=1` to disable ANSI output

**Key type/effect error codes:**

| Code | Meaning |
|------|---------|
| `E300` | Type mismatch (HM unification failure) |
| `E301` | Occurs-check failure (infinite type) |
| `E400` | Missing ambient effect on a call |
| `E401`–`E405` | `perform`/`handle` semantic errors |
| `E413`/`E414` | Effectful top-level expression / missing `main` |
| `E416`–`E418` | Strict-mode annotation requirements for `public fn` |
| `E423` | `Any` type in strict/exported position |
| `E015`/`E083` | Non-exhaustive `match` (general / ADT) |

---

## Unit Testing

Name any function `test_*` and run it with `--test`. No framework to install.

```flux
fn test_arithmetic() {
    assert_eq(2 + 2, 4)
    assert_eq(10 % 3, 1)
}

fn test_grade() {
    assert_eq(grade(95), "A")
    assert_false(grade(55) == "A")
}
```

```bash
cargo run -- --test examples/tests/math_test.flx
cargo run -- --test examples/tests/math_test.flx --test-filter test_grade
cargo run --features jit -- --test examples/tests/math_test.flx --jit
```

Exit code `0` on all-pass, `1` on any failure. Functions inside a module named exactly `Tests` are also discovered (`Tests.test_*`). The `Flow.FTest` stdlib (`--root lib/`) provides `describe`, `it`, `for_each`, `with_fixture`, and `approx_eq`.

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
| `--jit` | — | — | Use JIT backend (requires `--features jit`) |
| `--test` | Yes | Yes | Discover and run `test_*` functions; exit 0 on pass, 1 on failure |
| `--test-filter <s>` | Yes | Yes | Only run tests whose names contain `<s>` (requires `--test`) |
| `--stats` | Yes | Yes | Print timing and code metrics after run |
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
| `--strict` | Yes | Yes | Enforce fully-annotated `public fn` boundaries; reject `Any` in exported positions |

**`--stats` output:**
```
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

JIT output shows `jit compile [cranelift]` and `execute [native]` instead.

---

## Base Functions

All 77 Base functions are available without imports:

| Category | Functions |
|----------|-----------|
| **I/O** | `print`, `to_string`, `read_file`, `read_lines`, `read_stdin`, `parse_int`, `parse_ints`, `split_ints`, `now_ms`, `time` |
| **Numeric** | `abs`, `min`, `max` |
| **Strings** | `split`, `join`, `trim`, `upper`, `lower`, `chars`, `substring`, `starts_with`, `ends_with`, `replace` |
| **Arrays** | `len`, `push`, `concat`, `reverse`, `slice`, `sort`, `sort_by`, `flatten`, `contains`, `range`, `sum`, `product` |
| **Higher-order** | `map`, `filter`, `fold`, `flat_map`, `zip`, `find`, `any`, `all`, `count`, `first`, `last`, `rest` |
| **Cons lists** | `list`, `hd`, `tl`, `is_list`, `to_list`, `to_array` |
| **Hash maps** | `put`, `get`, `delete`, `keys`, `values`, `merge`, `has_key` |
| **Type checks** | `type_of`, `is_int`, `is_float`, `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some`, `is_map` |
| **Testing** | `assert_eq`, `assert_neq`, `assert_true`, `assert_false`, `assert_throws` |

---

## Documentation

**Language manual** — chapter-by-chapter guide in [`docs/guide/`](docs/guide/):

| Chapter | Topic |
|---------|-------|
| [1. Getting Started](docs/guide/01_getting_started.md) | Variables, types, arithmetic, strings, conditionals |
| [2. Functions and Closures](docs/guide/02_functions_and_closures.md) | Named functions, lambdas, closures, do-blocks, where clauses |
| [3. Collections](docs/guide/03_collections.md) | Arrays `[| |]`, cons lists, hash maps, tuples |
| [4. Pattern Matching](docs/guide/04_pattern_matching.md) | match, guards, Option, Either, cons patterns |
| [5. Higher-Order Functions](docs/guide/05_higher_order_functions.md) | map, filter, fold, zip, sort\_by, find, any, all |
| [6. Pipe Operator and List Comprehensions](docs/guide/06_pipe_and_comprehensions.md) | `\|>` pipelines, `[x \| x <- xs]` comprehensions |
| [7. Modules](docs/guide/07_modules.md) | Declaring modules, imports, aliases, private members |
| [8. Testing](docs/guide/08_testing.md) | Unit test framework, assert functions, FTest stdlib |
| [9. Type System Basics](docs/guide/09_type_system_basics.md) | Type annotations, HM inference, `Any`, `E300` |
| [10. Effects and Purity](docs/guide/10_effects_and_purity.md) | `with IO`/`with Time`, `perform`/`handle`, `E400` family |
| [11. HOF and Effect Polymorphism](docs/guide/11_hof_effect_polymorphism.md) | `with e` row variables, effect propagation through HOFs |
| [12. Modules, Public API, and Strict Mode](docs/guide/12_modules_public_api_and_strict.md) | `public fn`, `--strict`, `E416`–`E423` |
| [13. Match Exhaustiveness and ADTs](docs/guide/13_match_exhaustiveness_and_adts.md) | `type T = A \| B`, exhaustiveness, `E015`/`E083` |
| [14. Real-World Pipeline Walkthrough](docs/guide/14_real_world_pipeline_walkthrough.md) | Combining modules, typed APIs, effects, and ADTs |

**Compiler internals** — [`docs/internals/`](docs/internals/) covers bytecode, GC, JIT, value system, Base functions, diagnostics, error codes, linter, formatter, and the full type system + HM inference architecture.

**Release history** — [`docs/versions/`](docs/versions/) has What's New documents for each tagged release.

---

## Project Layout

```
src/
  syntax/       Lexer, parser, string interner, module graph, linter, formatter
  ast/          AST transforms: constant folding, desugaring, free vars, tail calls
                type_infer.rs — Algorithm W HM inference engine (infer_program)
  types/        HM type primitives: InferType, TypeSubst, Scheme, TypeEnv, unify
  bytecode/     Bytecode compiler, opcodes, symbol tables, .fxc cache
                compiler/contracts.rs — TypeExpr → RuntimeType conversion
                compiler/hm_expr_typer.rs — strict-path HM expression type consumer
  runtime/
    vm/         Stack-based VM, instruction dispatch, tracing
    base/       Base functions (array, string, map, numeric, higher-order, type checks)
    gc/         Mark-and-sweep GC, HAMT persistent maps, telemetry
    runtime_type.rs   — RuntimeType enum for boundary checks
    function_contract.rs — FunctionContract (param/return runtime types)
  jit/          Cranelift JIT backend (feature-gated): IR generation, runtime helpers, value arena
  diagnostics/  Error types, rendering, builder pattern, aggregation, registry
  primop/       PrimOp enum (71 ops), effect classification, fastcall allowlist
examples/
  basics/       ~260 Flux programs organized by category
  type_system/  Type system regression fixtures (passing + failing/)
  guide_type_system/  Runnable examples for guide chapters 9–14
tests/          50+ integration test files + snapshot tests (insta)
benches/        8 Criterion benchmark suites
tools/          VS Code syntax highlighting extension
lib/            Flux standard library (Flow.FTest, etc.)
docs/
  guide/        Chapter-by-chapter language manual (14 chapters)
  internals/    Compiler internals: bytecode, GC, JIT, type system, HM inference, diagnostics
  versions/     Release notes (v0.0.1 – v0.0.3)
  roadmaps/     Version-specific implementation plans
  proposals/    Language design proposals (0032, 0042, 0054, ...)
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
    ▼
  PASS 1         predeclare all top-level names (enables mutual recursion)
    │
    ▼
  HM Inference   Algorithm W — infer types, produce TypeEnv + ExprTypeMap
                 emit E300/E301 on concrete mismatches
    │
    ├──────────────────────────────────────┐
    ▼                                      ▼
  PASS 2 (Bytecode)                   Cranelift JIT
  type/effect validation              native machine code
  .fxc cache                          (shares HM output)
    │                                      │
    ▼                                      ▼
  Stack VM                            Native Execution
    │                                      │
    └──────────────────────────────────────┘
                    │
                    ▼
              GC Heap (cons lists · HAMT maps)
              + Runtime Boundary Checks (E055 · E1004)
```

---

## Development

### Tests

```bash
cargo test                                # full suite (~915 tests)
cargo test test_base_len                  # single test by name
cargo test --test parser_tests            # specific test file
cargo test --features gc-telemetry        # include telemetry tests
```

Snapshot tests use [insta](https://insta.rs). After changes that affect output:

```bash
cargo insta test --accept    # accept all new snapshots non-interactively
cargo insta review           # review snapshots interactively
```

### Formatting and Linting

```bash
cargo fmt --all -- --check                                # check formatting
cargo clippy --all-targets --all-features -- -D warnings  # lint (warnings = errors in CI)
```

### Benchmarks

```bash
cargo bench --bench lexer_bench    # run a specific benchmark suite
cargo bench                        # run all 8 suites
```

### Smoke Test

```bash
scripts/run_examples.sh --all              # run all non-error examples
scripts/run_examples.sh --all --no-cache   # bypass bytecode cache
```

### Releasing

1. For each feature/fix PR, add a changelog fragment:
   ```bash
   cp changes/_template.md changes/$(date +%Y-%m-%d)-short-topic.md
   ```
   Fill only relevant sections (`Added`, `Changed`, `Fixed`, `Performance`, `Docs`).

2. Run local release preflight:
   ```bash
   scripts/release_check.sh
   ```
   This runs the same core gates used in CI release checks:
   - `cargo fmt --all -- --check`
   - `cargo clippy --all-targets --all-features -- -D warnings`
   - `cargo test --all --all-features`
   - VM + JIT smoke tests with `--test`

3. Rebuild `[Unreleased]` from fragments:
   ```bash
   scripts/changelog_from_fragments.sh
   ```

4. Cut the release section + links automatically:
   ```bash
   scripts/release_cut.sh v0.0.3
   ```

5. Commit release docs/changelog, then create and push the tag:
   ```bash
   git checkout main
   git pull --ff-only
   git tag v0.0.3
   git push origin v0.0.3
   ```

Tagging behavior:
- Pushing a tag matching `v*` triggers `.github/workflows/release.yml`.
- The workflow first runs release gates (fmt, clippy, tests, VM/JIT smoke).
- If gates pass, it builds the Linux release binary, creates `.tar.gz` + `.sha256`, and publishes a GitHub Release.
- Release notes are extracted from the matching `## [vX.Y.Z]` section in `CHANGELOG.md`.

Recommended:
- Keep using `docs/versions/release_regression_v0.0.3.md` as a manual release checklist for extra parity/perf checks.
- Changelog fragment format/details: `changes/README.md`
- Release operations playbook: `docs/versions/release_playbook.md`
