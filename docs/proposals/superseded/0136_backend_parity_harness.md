- Feature Name: Backend Parity Harness
- Start Date: 2026-03-30
- Status: Superseded by Proposal 0138 (Flux Parity Ways)
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0104 (Flux Core IR), Proposal 0105 (LLVM Backend), Proposal 0118 (Backend Consolidation)

## Summary

Add a Rust-native parity tool and supporting workflow that detects VM/native
semantic drift immediately when compiler, runtime, stdlib, or lowering changes
land.

The harness treats `Core` as the semantic checkpoint and classifies failures
into:

- `dump-core failed`
- `core mismatch`
- `backend mismatch with identical core`
- `build/runtime failure`

The tool runs the same Flux program against:

- the default VM backend
- the native `core_to_llvm` backend
- optional `--dump-core` inspection for both command shapes

and compares:

- stdout
- stderr
- exit status

This proposal does not change language semantics. It strengthens the compiler's
ability to catch parity regressions immediately.

## Motivation

Flux now has two maintained execution paths:

```text
AST -> Core -> cfg -> bytecode -> VM
AST -> Core -> lir -> LLVM -> native
```

That architecture is correct only if both backends execute the same Core-level
meaning.

Recent regressions showed three distinct failure classes:

1. Native-only execution bugs with correct Core
   - top-level float constants lowered incorrectly
   - ADT equality for `Some(true)` missing in the native runtime

2. VM-only execution bugs with correct Core
   - imported exposed names incorrectly winning over local bindings

3. Tooling/orchestration bugs
   - `--dump-core` seeing a different program than normal execution because
     prelude injection was skipped
   - parity shell script accidentally comparing stale or wrong binaries

Without a parity harness, these bugs are discovered late and debugged by hand.

The goal is stronger than "eventually compare outputs." The goal is:

> detect semantic drift immediately, localize it quickly, and make parity a
> routine engineering contract rather than an occasional manual exercise.

This follows the same general discipline used by GHC-style architectures:

- one semantic IR
- backend split after semantics
- behavioral parity tests at the edges
- IR inspection in the middle to localize faults

## Non-goals

- Replacing `Core` with a new semantic IR
- Unifying `cfg` and `lir` in this proposal
- Making `--dump-core` a backend-specific surface
- Solving all optimizer/runtime bugs automatically

## Design

### Core principle

`Core` is the semantic contract.

Therefore parity checking must answer two different questions:

1. Are both backends starting from the same semantic program?
2. Do both backends execute that program identically?

The harness must check both.

### Proposed tool

Add a parity harness entry point:

```text
cargo run -- parity-check [dir] [--root dir ...]

  [dir]            Single .flx file or directory of .flx files

Options:
  --root <DIR>     Extra module root passed through to flux (repeatable)
```

Expected capabilities:

- Build an isolated VM binary in its own target dir
- Build an isolated native-feature binary in its own target dir
- Run each `.flx` file in a directory on both binaries
- Compare stdout, stderr, and exit code
- Optionally run `--dump-core` for both command shapes
- Report mismatches with useful classification

### Why Rust, not shell

The current shell script is fragile for parity work:

- shell timeout behavior can hide failures as empty output
- mixed feature/non-feature `target/debug` state can produce misleading runs
- stdout-only comparison can report false passes
- stderr and exit status are difficult to classify robustly

A Rust tool can own:

- process spawning
- timeout behavior
- output capture
- diff formatting
- fixture discovery
- target-dir isolation
- structured failure reporting

### Required comparison contract

For each fixture, record:

```rust
struct RunResult {
    backend: Backend,          // Vm or Native
    exit_code: Option<i32>,    // None if killed/timed out
    stdout: String,
    stderr: String,
    timed_out: bool,
}
```

Parity is success only when all of the following match:

- exit status
- stdout
- stderr (modulo allowed backend banner filtering)
- timeout state

### Core checkpoint

When requested, the harness also records:

```rust
struct CoreResult {
    command: CommandShape,     // vm-style or native-style invocation
    ok: bool,
    output: String,            // printed Core or diagnostic text
}
```

Classification rules:

- if either `--dump-core` run fails:
  - `dump-core failed`
- if both `--dump-core` runs succeed and differ:
  - `core mismatch`
- if `--dump-core` matches but execution differs:
  - `backend mismatch with identical core`
- if the tool cannot build or launch a backend:
  - `build/runtime failure`

This turns parity debugging from guesswork into a small decision tree.

### Output filtering

The tool should normalize only known backend banners and tool noise, for
example:

- `[cfg→vm] Running via CFG → bytecode VM backend...`
- `[lir→llvm] Compiling via LIR → LLVM native backend...`
- build-system progress lines emitted before execution

It must not normalize user-visible program behavior.

### Fixture strategy

The tool should support two fixture layers:

1. Large example directories
   - `examples/advanced`
   - `examples/basics`

2. Tiny parity regressions
   - `tests/parity/` or similar
   - one file per bug shape

Examples of required tiny regressions:

- top-level constant use
- local variable shadowing imported binding
- `Some(true) == Some(true)`
- map value filtering over `Option<Bool>`

The small cases catch exact bug classes immediately. The large examples provide
integration coverage.

## Workflow

### Developer workflow

When compiler behavior changes:

1. Run the smallest relevant parity slice
2. If mismatch:
   - inspect `--dump-core`
   - inspect backend-specific lowering only after Core is confirmed
3. Reduce the bug to a tiny fixture
4. Add the fixture to the parity suite

### CI workflow

At minimum, require parity runs for changes touching:

- `src/core/`
- `src/aether/`
- `src/cfg/`
- `src/lir/`
- `src/core_to_llvm/`
- `src/bytecode/`
- `runtime/`
- `lib/Flow/`
- `src/main.rs`

Recommended CI steps:

```text
1. cargo test --all --all-features
2. cargo run -- parity-check tests/parity
3. cargo run -- parity-check examples/advanced
```

## Proposed implementation

### Phase 1: parity harness entry point

Provide a maintained parity harness with:

- directory traversal for `.flx`
- isolated target dirs for VM and native builds
- backend execution with timeout
- structured comparison of stdout/stderr/exit code

Verification:

- reproduces current shell-script use case for `examples/advanced`
- catches non-zero exit mismatches

### Phase 2: Core-aware classification

Add optional `--check-core` behavior:

- run `flux file.flx --dump-core`
- run native-feature `flux file.flx --native --dump-core`
- classify mismatch source

Verification:

- can distinguish orchestration bugs from backend runtime bugs

### Phase 3: Tiny regression corpus

Add a dedicated parity fixture directory with reduced cases for known bug
classes.

Verification:

- each historical parity bug has a direct fixture

### Phase 4: CI integration

Make parity checks part of normal compiler-change validation.

Verification:

- compiler/runtime PRs fail fast on backend drift

## Alternatives considered

### Keep the shell script

Rejected.

It is useful as a quick local helper but too brittle for a correctness contract.
Feature-build isolation, timeout handling, exit status checks, and mismatch
classification are all materially better in Rust.

### Compare only `--dump-core`

Rejected.

Equal Core is necessary but not sufficient. Several recent bugs had identical
Core and still diverged at runtime.

### Compare only end-to-end execution

Rejected.

This catches parity regressions but makes localization too slow. The Core
checkpoint is necessary to determine whether the bug is semantic or backend-only.

## Drawbacks

- Additional maintenance for a dedicated tooling binary
- Extra CI time from building two backend variants
- Need to keep output filtering narrow and honest

These are acceptable because parity drift between maintained backends is a
higher-cost failure mode than the extra tooling.

## Success criteria

This proposal is successful when:

- new backend mismatches are detected immediately in local or CI runs
- failures are automatically classified as semantic, backend, or tooling issues
- `--dump-core` remains trustworthy as the first semantic debugging surface
- every fixed parity bug leaves behind a minimal regression fixture

## Open questions

- Should the parity harness continue to live as a shell script or move to a future `xtask`?
- Should `stderr` comparison allow an explicit ignore list for compile banners?
- Should the tool support JSON output for CI annotation?
- Should parity checks be sharded by directory to control CI time?
