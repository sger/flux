# Flux

[![CI](https://github.com/sger/flux/actions/workflows/ci.yml/badge.svg)](https://github.com/sger/flux/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org/)
[![Status: experimental](https://img.shields.io/badge/status-experimental-yellow.svg)](#project-status)

**Flux is an experimental, pure functional language with full type inference, algebraic
effects, and two backends — a bytecode VM and an LLVM-native compiler — sharing one frontend.**

It started as a compiler-construction learning project and grew into a real pipeline:
Hindley–Milner inference, row-polymorphic algebraic effects, brace-style syntax, and
Elm-grade error messages. Flux is inspired by [Haskell](https://www.haskell.org/) (purity,
inference), [Koka](https://koka-lang.github.io/koka/doc/index.html) (effects, effect rows),
[Elm](https://elm-lang.org/) (human-friendly errors), and [Rust](https://www.rust-lang.org/)
(syntax, tooling).

```flux
// Types are inferred. Effects are tracked. Pattern matching is exhaustive.
type Shape = Circle(Float) | Rect(Float, Float)

fn area(s) {
    match s {
        Circle(r)    -> 3.14159 * r * r,
        Rect(w, h)   -> w * h,
    }
}

fn main() with IO {
    let shapes = [Circle(2.0), Rect(3.0, 4.0)]
    let total  = fold(map(shapes, area), 0.0, \(acc, a) -> acc + a)
    print("total area = " + to_string(total))   // "total area = 24.56636"
}
```

---

## Features

- **Type inference that just works** — Hindley–Milner with type classes; you rarely write a
  type annotation, yet everything is statically checked. `forall`-quantified schemes,
  constrained type parameters (`Eq<a>`, `Ord<a>`, …), and ADTs with exhaustiveness checking.
- **Algebraic effects, with rows** — `effect`, `perform`, and `handle` give you typed,
  resumable control flow (state, logging, async, I/O). Effect *rows* are polymorphic, so
  higher-order functions stay effect-generic instead of hard-coding `IO`.
- **Purity by default** — a function that touches the outside world has to say so
  (`fn main() with IO`). The type system separates pure computation from effects.
- **Two backends, one frontend** — the same Core IR feeds a stack-based **bytecode VM** and
  an **LLVM native** backend. A `parity-check` harness asserts both produce identical output.
- **Errors you can read** — Elm-style diagnostics with spans, codes, and suggestions.
- **First-class tooling** — a Language Server (`crates/flux-lsp`) powers diagnostics, hover,
  completion, goto-definition, references, rename, semantic tokens, inlay hints, and signature
  help, plus a bundled VS Code extension.
- **Async & concurrency** — fibers, structured concurrency (`both`, `race`, `first_of`,
  scopes), channels, timeouts, and an `mio`-backed I/O reactor.

## Quick start

Requires a Rust toolchain (Edition 2024, MSRV **1.93.0**).

```sh
git clone https://github.com/sger/flux.git
cd flux

# Run a program on the bytecode VM (default backend)
cargo run -- examples/guide/01_getting_started.flx

# Run the test blocks in a file
cargo run -- --test examples/tests/array_test.flx
```

Building the **LLVM native backend** is opt-in (it needs LLVM installed) and gated behind a
Cargo feature:

```sh
cargo run --features llvm -- --native examples/guide/factorial.flx
```

Install the `flux` binary onto your `PATH`:

```sh
cargo install --path .
flux examples/guide/fibonacci.flx
```

## A tour of the language

**Algebraic effects** — define an effect, perform it, and handle it. The handler decides what
`resume` does, so effects are fully resumable:

```flux
effect Audit {
    log: String -> Int
}

fn audited_value() -> Int with Audit {
    perform Audit.log("started") + 1
}

fn main() with IO {
    let value = audited_value() handle Audit {
        log(resume, message) -> resume(len(message))
    }
    print("audit_value=" + to_string(value))   // "audit_value=8"
}
```

**Structured concurrency** — `both` runs two fibers and returns both results; tuple position
follows source order, not finish order:

```flux
import Flow.Async exposing (..)

fn left()  -> Int with Async { sleep(60); 10 }   // slow on purpose
fn right() -> Int with Async { sleep(20); 20 }   // fast; finishes first

fn body() -> (Int, Int) with Async {
    both(left, right)   // result is (left, right) regardless of finish order
}

fn main() with IO {
    let pair = run_async(body)
    print("left = "  + to_string(pair.0))   // "left = 10"
    print("right = " + to_string(pair.1))   // "right = 20"
}
```

**Records, pipes, and pattern matching** make data wrangling concise:

```flux
let summary =
    students
    |> map(summarize)
    |> filter(\s -> s.passed)

match results["Alice"] {
    Some(r) -> format_result_line("Alice", r),
    _       -> "Alice: not found",
}
```

More to explore in [`examples/`](examples/): [`effects/`](examples/effects/),
[`guide_async/`](examples/guide_async/), [`type_system/`](examples/type_system/),
[`patterns/`](examples/patterns/), and the walkthrough series in
[`examples/guide/`](examples/guide/).

## CLI

`flux <file.flx>` runs a program; explicit subcommands expose the pipeline:

| Command | What it does |
| --- | --- |
| `flux run <file>` | Compile and run on the VM (the default if you omit `run`) |
| `flux run --native <file>` | Compile and run via the LLVM backend (needs `--features llvm`) |
| `flux --test <file>` | Run the `test` blocks in a file |
| `flux tokens <file>` | Dump the lexer token stream |
| `flux bytecode <file>` | Dump compiled VM bytecode |
| `flux fmt <file>` | Format source |
| `flux lint <file>` | Lint source |
| `flux parity-check <dir> --ways vm,llvm` | Assert VM and LLVM produce identical output |
| `flux clean` / `flux cache-info <file>` | Manage / inspect the compile cache |

Run `flux --help` for the full list (cache inspection, free-variable and tail-call analysis,
interface info, and more).

## Architecture

One frontend, one canonical semantic IR (`core/`), two backends:

```
Source → syntax/ (lexer, parser, module graph) → AST
       → HM inference (ast/type_infer + types/)
       → core/   (canonical semantic IR — the only semantic IR)
       → aether/ (ownership / reuse lowering)
       ├── cfg/ → compiler/ → bytecode/ → vm/        (VM backend)
       └── lir/ → llvm/                              (native backend, feature = "llvm")
```

The DAG is one-way and `core/` is the single source of semantic truth — `cfg/` is VM-only,
`lir/` is native-only, and `llvm/` never touches bytecode. The authoritative reference is
[`docs/internals/compiler_architecture.md`](docs/internals/compiler_architecture.md).

## Workspace layout

This is a Cargo workspace with two members:

- **`flux`** (root) — compiler, VM, LLVM backend, and the `flux` CLI.
- **`crates/flux-lsp`** — the Language Server. It reuses the root crate's frontend (lexer,
  parser, module graph, HM inference) rather than reimplementing parsing or typing.

The VS Code extension lives in [`editors/vscode/`](editors/vscode/) (TypeScript client +
bundled server). Rebuild it with `scripts/rebuild-vsix.ps1` (Windows) or
`scripts/rebuild-vsix.sh` (Unix); see [`editors/vscode/README.md`](editors/vscode/README.md).

## Development

The quality gates mirror CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)):

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --all-features
```

- Tests are organized by pipeline stage under [`tests/`](tests/); snapshot tests use
  [`insta`](https://insta.rs/) (`cargo insta review` to accept).
- [`CHANGELOG.md`](CHANGELOG.md) is written from the merged PRs at release time; a PR's
  description is its changelog entry.
- Contributor and architecture notes live in [`CLAUDE.md`](CLAUDE.md) and
  [`docs/internals/`](docs/internals/).

## Documentation

- [`docs/guide/`](docs/guide/) — the user-facing language guide (start at
  [`01_getting_started.md`](docs/guide/01_getting_started.md))
- [`lib/Flume/README.md`](lib/Flume/README.md) — package-manager usage and manifest reference
- [`docs/internals/`](docs/internals/) — compiler internals (architecture, IRs, type/effect
  system, diagnostics, error codes)
- [`docs/proposals/`](docs/proposals/) — RFCs (e.g. proposal 0174 for async)
- [`docs/roadmaps/`](docs/roadmaps/) — per-version roadmaps

## Project status

Flux is **experimental** (currently `v0.0.6`). The language and internals move fast and APIs
can change without notice — it's a place to learn and tinker, not yet a production runtime.

## License

[MIT](LICENSE) © Spiros Gerokostas
