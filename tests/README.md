# Flux test suite

This directory holds integration tests for the Flux compiler. 

## Layout

```
tests/
├── support/          Shared helpers (primop parity, examples snapshot runner, …)
├── lexer/            Lexer, tokens, interner, parse perf
├── parser/           Parser, recovery, spans, linter, list-literal syntax
├── ast_passes/       desugar, constant_fold, free_vars, rename, tail_position, tail_call
├── type_inference/   HM inference, semantics matrices, static typing, guards
├── core_ir/          Core IR contracts, primop lowering, backend representation
├── aether/           Aether RC pass (dup/drop/reuse insertion, FBIP)
├── bytecode/         OpCode, superinstruction, cmp/jump fusion
├── vm_runtime/       VM, program, stdlib, runtime list, stackless drop
├── native_llvm/      LLVM ADTs, codegen, closures, native primop parity
├── diagnostics/      Render, span render, aggregator, error registry, guards
├── integration/      Modules, flow prelude, compiler rules, CLI test runner
├── examples/         Examples snapshot harness (basics, advanced, fixtures)
├── error_fixtures/   Parser / compiler / runtime error fixture snapshots
├── fixtures/         Flux source fixtures grouped by topic
├── parity/           VM-vs-native parity fixtures (.flx)
├── flux/             CLI / test-runner fixtures
└── snapshots/        insta snapshot trees, organized by phase
```

Target phase subdirectories (per proposal 0169):

| Phase                       | Subdirectory        |
|-----------------------------|---------------------|
| Lexer                       | `lexer/`            |
| Parser / formatter / linter | `parser/`           |
| AST passes                  | `ast_passes/`       |
| Type inference & classes    | `type_inference/`   |
| Core IR & Core passes       | `core_ir/`          |
| Aether RC                   | `aether/`           |
| CFG / bytecode compiler     | `bytecode/`         |
| VM / runtime / stdlib       | `vm_runtime/`       |
| LLVM native backend         | `native_llvm/`      |
| Diagnostics                 | `diagnostics/`      |
| Integration / CLI           | `integration/`      |
| Examples                    | `examples/`         |
| Error fixtures              | `error_fixtures/`   |

## Conventions

### Placement

New tests go under the subdirectory for the phase they assert against.
If a test spans multiple phases, place it with the phase whose code it is
most likely to break. When in doubt, follow the explicit mapping in
proposal 0169.

### Shared helpers live in `tests/support/`

Do not duplicate helpers across test files. If two tests need the same
helper, put it in `tests/support/<topic>.rs` and include it via:

```rust
#[path = "support/<topic>.rs"]
mod <topic>;
```

Current helpers:

- [`support/primop_parity.rs`](support/primop_parity.rs) — `run_native`, `run_vm`, `assert_vm_native_parity` for VM/LLVM parity checks on `tests/parity/*.flx`
- [`support/examples_snapshot.rs`](support/examples_snapshot.rs) — snapshot harness driving fixture directories (examples, error fixtures)
- [`support/semantic_infer.rs`](support/semantic_infer.rs) — type-inference matrix driver
- [`support/semantic_runtime.rs`](support/semantic_runtime.rs) — runtime matrix driver
- [`support/semantic_core_dump.rs`](support/semantic_core_dump.rs) — core-IR contract matrix driver
- [`support/purity_parity.rs`](support/purity_parity.rs) — purity analysis parity driver

### Cargo registration

Cargo's `autotests` only discovers `tests/*.rs`. Files in a subdirectory
need an explicit `[[test]]` entry in `Cargo.toml`:

```toml
[[test]]
name = "aether_cli_snapshots"
path = "tests/aether/aether_cli_snapshots.rs"
```

Keep the `name` identical to the old flat filename (minus `.rs`) so that
insta snapshot filenames — which are prefixed with the test binary name —
do not drift. This is load-bearing: renaming a test binary invalidates
every `.snap` file it produced unless the test uses
`prepend_module_to_snapshot => false`.

### Snapshot files

Snapshots live under [`snapshots/<phase>/`](snapshots/). The test asserting
a snapshot picks the directory via `insta::with_settings!`:

```rust
insta::with_settings!({
    snapshot_path => "snapshots/<phase>",
    prepend_module_to_snapshot => false,
}, {
    insta::assert_snapshot!("<name>", rendered);
});
```

Review snapshot diffs with `cargo insta review` before committing.

### Fixture directories

- `tests/fixtures/` — per-topic `.flx` sources used by various tests
- `tests/parity/` — VM-vs-native parity fixtures; see `CONVENTION.md` inside
- `tests/flux/` — CLI and test-runner fixtures (`--test`, flow tests, etc.)

These are not redundant; they serve different test runners and have
different conventions.

## Running

```bash
cargo test --all --all-features                 # full suite
cargo test --test <binary_name>                 # single binary
cargo test <partial_test_name>                  # filter by test name
cargo insta review                              # review snapshot diffs
cargo insta test --accept                       # accept all new snapshots
```
