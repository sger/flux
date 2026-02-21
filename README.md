# Flux

Flux is a functional language written in Rust with two execution backends: a stack-based **bytecode VM** and a **Cranelift JIT** that compiles to native machine code. It started as a learning project — inspired by [Writing a Compiler in Go](https://compilerbook.com/), [Crafting Interpreters](https://craftinginterpreters.com/), [Rust for Rustaceans](https://nostarch.com/rust-rustaceans), and *Virtual Machines* by Iain D. Craig.

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

- **Two execution backends** — Bytecode VM for portability; Cranelift JIT for native-speed execution. Both share the same builtin table, GC heap, and collection library.
- **Functional core** — Immutable `let` bindings, first-class functions, closures, higher-order builtins
- **Pattern matching** — `match` on literals, `Some`/`None`, `Left`/`Right`, cons cells `[h | t]`, tuples, with guards
- **List comprehensions** — `[x * 2 | x <- xs, x > 0]` desugared at parse time to `map`/`filter`/`flat_map`
- **Persistent collections** — GC-managed cons lists and HAMT hash maps with structural sharing
- **Pipe operator** — `a |> f(b)` desugars to `f(a, b)` — natural left-to-right data pipelines
- **Modules** — Static qualified namespaces, imports with aliases, cycle detection at compile time
- **String interpolation** — `"Hello #{name}, result is #{1 + 2}"`
- **Unit testing** — Built-in `--test` runner, `test_*` discovery, assert builtins, `Flow.FTest` stdlib
- **Diagnostics** — Elm-style errors with stable codes (`E030`, `W200`), source snippets, inline suggestions
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
- Stable error code catalog: `E001–E077` (compiler/parser), `E1000–E1021` (runtime), `W200+` (warnings)
- Set `NO_COLOR=1` to disable ANSI output

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

## Builtin Functions

All 77 builtins are available without imports:

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
| [8. Testing](docs/guide/08_testing.md) | Unit test framework, assert builtins, FTest stdlib |

**Compiler internals** — [`docs/internals/`](docs/internals/) covers bytecode, GC, JIT, value system, builtins, diagnostics, error codes, linter, and formatter.

**Release history** — [`docs/versions/`](docs/versions/) has What's New documents for each tagged release.

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
examples/       ~260 Flux programs organized by category
tests/          50 integration test files + snapshot tests (insta)
benches/        8 Criterion benchmark suites
tools/          VS Code syntax highlighting extension
lib/            Flux standard library (Flow.FTest, etc.)
docs/
  guide/        Chapter-by-chapter language manual (8 chapters)
  internals/    Compiler internals: bytecode, GC, JIT, diagnostics, builtins
  versions/     Release notes (v0.0.1, v0.0.2, v0.0.3)
  roadmaps/     Version-specific implementation plans
  proposals/    Language design proposals
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
cargo test                                # full suite (~915 tests)
cargo test test_builtin_len               # single test by name
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

1. Add entries to the `[Unreleased]` section of `CHANGELOG.md` as you develop.
2. When ready to release, move `[Unreleased]` entries to a new versioned section:
   ```markdown
   ## [v0.0.4] - YYYY-MM-DD
   ```
3. Update the diff links at the bottom of `CHANGELOG.md`:
   ```markdown
   [Unreleased]: https://github.com/sger/flux/compare/v0.0.4...HEAD
   [v0.0.4]: https://github.com/sger/flux/compare/v0.0.3...v0.0.4
   ```
4. Commit the changelog, then push the tag:
   ```bash
   git tag v0.0.4
   git push origin v0.0.4
   ```

The CI release workflow (`.github/workflows/release.yml`) picks up `v*` tags automatically — it builds the Linux binary, packages it as a `.tar.gz` with a SHA-256 checksum, and creates a GitHub release using the matching `CHANGELOG.md` section as the release body.
