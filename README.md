# Flux

A small, functional programming language with a custom bytecode VM, written in Rust.

## Features

- **Functions** — `fun` declarations, closures, higher-order functions, forward references, mutual recursion, lambdas (`\x -> x + 1`)
- **Immutability** — `let` bindings are immutable; reassignment is a compile error
- **Pattern matching** — `match` on literals, `Some`/`None`, `Left`/`Right`, cons lists `[h | t]`, with guards
- **Modules** — Static qualified namespaces with Haskell-style imports, aliases, and cycle detection
- **Collections** — Arrays, persistent hash maps (HAMT), and cons lists with structural sharing
- **Operators** — Arithmetic, comparison, logical (`&&`, `||`), modulo (`%`), pipe (`|>`)
- **String interpolation** — `"Hello #{name}, sum is #{1 + 2}"`
- **Higher-order builtins** — `map`, `filter`, `fold`, `sort`, `concat`, `reverse`, `slice`, and more
- **Diagnostics** — Elm-style errors with stable codes (`E101`, `W200`), source snippets, inline suggestions, and did-you-mean hints
- **Tooling** — Linter, formatter, bytecode inspector, free variable analyzer, tail call detector
- **Bytecode cache** — `.fxc` files with dependency-aware invalidation
- **GC** — Mark-and-sweep garbage collector for persistent data structures, with optional telemetry (`--features gc-telemetry`)
- **Editor support** — VS Code and Zed syntax highlighting extensions

## Quick Start

### Build

```bash
cargo build
```

### Run a program

```bash
cargo run -- examples/basics/print.flx
cargo run -- examples/advanced/grade_analyzer.flx --root examples/
```

Or use the helper script (sets module roots automatically):

```bash
scripts/run_examples.sh basics/pipe_operator.flx
scripts/run_examples.sh --all                      # run all non-error examples
```

## Language Overview

### Variables and functions

```flux
let name = "World"
print("Hello #{name}!")

fun double(x) { x * 2 }
let triple = \x -> x * 3

print(double(5))   // 10
print(triple(5))   // 15
```

### Pipe operator

```flux
fun double(x) { x * 2 }
fun add(x, y) { x + y }

let result = 2 |> double |> add(10) |> double
print(result)  // 28
```

### Pattern matching

```flux
fun describe(value) {
    match value {
        0 -> "zero",
        Some(x) -> "got: " + to_string(x),
        Left(err) -> "error: " + err,
        Right(val) -> "ok: " + to_string(val),
        _ -> "other",
    }
}
```

### List cons patterns

```flux
fun sum(lst) {
    match lst {
        [h | t] -> h + sum(t),
        _ -> 0,
    }
}

let nums = to_list([1, 2, 3, 4])
print(sum(nums))  // 10
```

### Higher-order functions

```flux
let numbers = [1, 2, 3, 4, 5, 6]

let doubled = map(numbers, \x -> x * 2)
let evens = filter(numbers, \x -> x % 2 == 0)
let total = fold(numbers, 0, \(acc, x) -> acc + x)

print(doubled)  // [2, 4, 6, 8, 10, 12]
print(evens)    // [2, 4, 6]
print(total)    // 21
```

### Modules and imports

```flux
// Modules/Math.flx
module Modules.Math {
    fun square(x) { x * x }
    fun _helper(x) { x }  // private (underscore prefix)
}

// main.flx
import Modules.Math as M
print(M.square(5))  // 25
```

### Closures

```flux
fun counter(start) {
    let count = start
    \() -> count
}

let c = counter(10)
print(c())  // 10
```

### Hash maps (persistent)

```flux
let m = {"name": "Alice", "age": 30}
let m2 = put(m, "email", "alice@example.com")

print(get(m, "name"))    // Some(Alice)
print(get(m2, "email"))  // Some(alice@example.com)
print(get(m, "email"))   // None (original unchanged)
```

## CLI Reference

```
flux <file.flx>                          Run a Flux program
flux run <file.flx>                      Run (explicit subcommand)
flux tokens <file.flx>                   Show token stream
flux bytecode <file.flx>                 Show compiled bytecode
flux lint <file.flx>                     Run linter
flux fmt [--check] <file.flx>            Format source (or check)
flux cache-info <file.flx>               Inspect cache for source file
flux cache-info-file <file.fxc>          Inspect a .fxc cache file
flux analyze-free-vars <file.flx>        Show free variables
flux analyze-tail-calls <file.flx>       Show tail-call sites
```

### Flags

| Flag | Description |
|------|-------------|
| `--verbose` | Show cache hit/miss/store status |
| `--trace` | Print VM instruction trace |
| `--leak-detector` | Print allocation stats after run |
| `--no-cache` | Disable bytecode cache |
| `--optimize`, `-O` | Enable AST optimizations (desugar + constant fold) |
| `--analyze`, `-A` | Enable analysis passes (free vars + tail calls) |
| `--max-errors <n>` | Limit displayed errors (default: 50) |
| `--root <path>` | Add a module search root (repeatable) |
| `--roots-only` | Use only explicit `--root` values |
| `--no-gc` | Disable garbage collection |
| `--gc-threshold <n>` | Set GC collection threshold |
| `--gc-telemetry` | Print GC report after execution (requires `--features gc-telemetry`) |
| `NO_COLOR=1` | Disable ANSI color output (env var) |

## Diagnostics

Flux produces structured, phase-aware diagnostics with stable error codes:

```
--> compiler error[E101]: UNKNOWN KEYWORD

Flux uses `fun` for function declarations.

  --> examples/hint_demos/inline_suggestion_demo.flx:24:1
  |
24 | fn add(a, b) {
  | ^^   |
help: Replace 'fn' with 'fun'
   |
24 | fun add(a, b) {
  | ~~~
```

- Errors grouped by file, sorted by line/column/severity
- Inline suggestions and did-you-mean hints
- Runtime errors include stack traces
- Error code catalog: `E1xx` (parser), `E2xx`-`E9xx` (compiler), `E10xx` (runtime), `W2xx` (warnings)

## Project Layout

```
src/
  syntax/          Lexer, parser, string interner, module graph, linter, formatter
  ast/             AST transforms: constant folding, desugaring, free vars, tail calls
  bytecode/        Bytecode compiler, opcodes, symbol tables, .fxc cache
  runtime/
    vm/            Stack-based VM, instruction dispatch, tracing
    builtins/      Built-in functions (array, string, hash, numeric, list ops)
    gc/            Mark-and-sweep GC, HAMT persistent maps, telemetry
  diagnostics/     Error types, rendering, builder pattern, aggregation, registry
examples/          ~170 Flux programs organized by category
tests/             ~50 integration test files + snapshot tests
benches/           7 Criterion benchmark suites
tools/             VS Code and Zed syntax highlighting extensions
docs/              Architecture docs, language design, proposals
```

## Architecture

```
Source (.flx)
    |
    v
  Lexer          token stream
    |
    v
  Parser         AST (with string interning)
    |
    v
  AST Passes     constant folding, desugaring, free var collection, tail call detection
    |
    v
  Compiler       stack-based bytecode (45+ opcodes)
    |
    v
  Cache          .fxc files under target/flux/ (dependency-hashed)
    |
    v
  VM             stack machine with call frames, closures, builtins
    |
    v
  GC             mark-and-sweep for cons lists and HAMT nodes
```

Multi-file programs are compiled via `ModuleGraph`: imports are resolved across module roots, topologically sorted, and compiled in dependency order.

Runtime values use `Rc` for sharing with a no-cycle invariant (values form DAGs). Persistent collections (HAMT maps, cons lists) are GC-managed with structural sharing.

## Documentation

- [docs/architecture/](docs/architecture/) — Compiler architecture, visitor patterns, error catalog, GC roadmap
- [docs/language/](docs/language/) — Language design, module system, semicolon rules, grammar
- [docs/proposals/](docs/proposals/) — 21 design proposals tracking language evolution
- [docs/tooling/](docs/tooling/) — Debugging tools, benchmarking guides
- [OPTIMIZATION_GUIDE.md](OPTIMIZATION_GUIDE.md) — AST optimization and analysis guide
- [CHANGELOG.md](CHANGELOG.md) — Version history

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

### Formatting and linting

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

## Editor Support

### VS Code

```bash
python3 tools/vscode-flux/scripts/build-vsix.py
```

Install `tools/vscode-flux/dist/flux-language-0.0.1.vsix` via Extensions > Install from VSIX.

### Zed

Open Command Palette > `Extensions: Install Dev Extension` > select `tools/zed-flux`.

## Roadmap

Derived from [docs/proposals/](docs/proposals/):

- **Tail-call optimization** — Stack reuse for self-recursive calls
- **Zero-copy value passing** — Eliminate unnecessary clones in the VM
- **Standard library** — Extracted builtin modules
- **Gradual typing** — Optional type annotations (planned)
- **Tree-sitter grammar** — Full syntax highlighting for editors (planned)

## Contributing

1. Fork the repository and create a feature branch
2. Ensure `cargo test`, `cargo fmt --all -- --check`, and `cargo clippy --all-targets --all-features -- -D warnings` all pass
3. Add tests for new functionality; use snapshot tests for output-sensitive changes
4. Keep commits focused and descriptive
5. Open a pull request against `main`
