# Flux test suite

This directory holds the Flux compiler's integration tests. Unit tests live
next to the code in `src/`. `cargo xtask tidy` checks that this file and the
directory agree.

## Layout

```
tests/
├── support/          Shared helpers; every test reaches the CLI, files and ports through these
├── lexer/            Lexer, tokens, interner, parse perf
├── parser/           Parser, recovery, spans, linter, list-literal syntax
├── ast_passes/       desugar, constant_fold, free_vars, rename, tail_position, tail_call
├── type_inference/   HM inference, semantics matrices, static typing, guards
├── core_ir/          Core IR contracts, primop lowering, backend representation
├── aether/           Aether RC pass (dup/drop/reuse insertion, FBIP)
├── bytecode/         OpCode, superinstruction, cmp/jump fusion
├── vm_runtime/       VM, fibers, HTTP/TCP, JSON, streams, stdlib modules
├── native_llvm/      LLVM codegen, ADTs, closures, native runtime, the parity corpus test
├── debug/            VM and native debuggers, the debug heap, DWARF and CodeView
├── diagnostics/      Render, span render, aggregator, error registry, guards
├── error_fixtures/   Parser / compiler / runtime error fixture snapshots
├── flume/            Manifests, locks, resolution, packages and projects
├── replay/           Recording a run and replaying it on either backend
├── integration/      Modules, flow prelude, compiler rules, CLI test runner
├── examples/         Snapshots of the programs under examples/
├── fixtures/         Flux source fixtures grouped by topic
├── parity/           Programs both backends must run the same; see CONVENTION.md
├── parity_debug/     Scripted debugger sessions the VM and lldb must agree on
├── flux/             Flux test files run with `flux --test`
└── snapshots/        insta snapshot trees, organized by phase
```

Which folder a phase's tests go in:

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
| Debuggers and debug info    | `debug/`            |
| Diagnostics                 | `diagnostics/`      |
| Error fixtures              | `error_fixtures/`   |
| Package manager (Flume)     | `flume/`            |
| Record and replay           | `replay/`           |
| Integration / CLI           | `integration/`      |
| Examples                    | `examples/`         |

## Where a new test goes

1. **A program both backends must run the same way** is a `.flx` file in
   `parity/`, not Rust. Give it a header (`// parity: vm, llvm`,
   `// expect: success`) and let
   `cargo run --features llvm -- parity-check tests/parity/<file>.flx --bless`
   write its expected output. `parity_corpus_tests` runs every file there.
   [`parity/CONVENTION.md`](parity/CONVENTION.md) lists the header lines.
2. **A test of one phase** goes in that phase's folder above. If it spans
   several phases, put it with the phase it is most likely to break.
3. **A Flux test file for `flux --test`** goes in `flux/`, with a Rust test
   that runs it through `support/stdlib_fixture.rs` or `run_flux_test`.

A Rust test file needs a `[[test]]` entry in `Cargo.toml`, or it is never
compiled:

```toml
[[test]]
name = "aether_cli_snapshots"
path = "tests/aether/cli_snapshots.rs"
```

Keep an existing `name` when you move a file: insta prefixes snapshot names
with it unless the test sets `prepend_module_to_snapshot => false`.

### Naming

Name a test for what it checks. A regression test also gets the number of its
known issue as a suffix, so searching for the KI finds it:
`resume_keeps_handler_frames_ki136`.

## Shared helpers

Do not copy a helper into a test file. Include it from `support/`:

```rust
#[path = "../support/flux_runner.rs"]
mod flux_runner;
```

- [`support/scratch.rs`](support/scratch.rs): `Scratch`, a private directory
  and cache for one test, removed on drop; `free_port`; `workspace_root`.
  Temporary files always come from a `Scratch`.
- [`support/flux_runner.rs`](support/flux_runner.rs): runs the `flux` CLI.
  `flux_source(source, tag)` and `flux_file(path)` build a run, with
  `.native()`, `.args(..)`, `.env(..)` and `.timeout(..)`. Shortcuts:
  `run_flux` (inline source), `run_vm` and `run_native` (a `parity/` file),
  `run_flux_test` (a `flux/` file), and `program_elapsed` for timed
  programs. It re-exports `scratch`.
- [`support/stdlib_fixture.rs`](support/stdlib_fixture.rs): runs a `flux/`
  test file and checks its summary; `assert_backends_agree` runs it on the VM
  and, with the `llvm` feature, natively.
- [`support/primop_parity.rs`](support/primop_parity.rs):
  `assert_vm_native_parity` for a `parity/` file.
- [`support/examples_snapshot.rs`](support/examples_snapshot.rs): snapshots
  every program in a fixture directory (examples, error fixtures).
- [`support/diagnostics_env.rs`](support/diagnostics_env.rs): sets
  `NO_COLOR` under a lock for tests that render diagnostics.
- [`support/semantic_infer.rs`](support/semantic_infer.rs),
  [`support/semantic_runtime.rs`](support/semantic_runtime.rs),
  [`support/semantic_core_dump.rs`](support/semantic_core_dump.rs): the
  type-inference, runtime and Core contract matrix drivers.
- [`support/proc_helper.rs`](support/proc_helper.rs): the `flux_proc_helper`
  program that the process tests spawn.

## Snapshots

Snapshots live under [`snapshots/<phase>/`](snapshots/). A test picks the
directory with `insta::with_settings!`; the path is relative to the test file:

```rust
insta::with_settings!({
    snapshot_path => "../snapshots/<phase>",
    prepend_module_to_snapshot => false,
}, {
    insta::assert_snapshot!("<name>", rendered);
});
```

Review snapshot diffs with `cargo insta review` before committing. CI fails
on a snapshot no test produces.

## Fixture directories

- `fixtures/`: per-topic `.flx` sources used by various tests.
- `parity/`: programs both backends must run the same; see `CONVENTION.md`.
- `parity_debug/`: debugger sessions; see its `README.md`.
- `flux/`: Flux test files for `flux --test`.

They serve different runners and have different conventions.

## Running

The test tiers (`cargo test-fast`, `cargo test-all`, `cargo test-changed`,
`cargo test-phase`) are described in [`TESTING.md`](../TESTING.md).

```bash
cargo test-all --test <binary_name>             # one binary
cargo test-all -E 'test(<partial_name>)'        # filter by test name
cargo insta review                              # review snapshot diffs
cargo xtask tidy                                # check this layout
```

The heavy suites (`native_llvm/`, `debug/`, `parity/`, `parity_debug/` and
`cargo xtask verify`) build and run a lot of native binaries. Do not run two of
them at once.
