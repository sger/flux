# Testing

The suite has two tiers. Use the fast tier while you work and the full tier
before you push. CI runs the full tier. Where tests live and how to write them
is in [`tests/README.md`](tests/README.md).

The tiers need [cargo-nextest](https://nexte.st): `cargo install cargo-nextest`.

## Commands

| Command | Runs | Use it |
|---|---|---|
| `cargo test-fast` | Every test in the `flux` package that builds with default features: no LLVM backend, stops at the first failure | While iterating |
| `cargo test-changed` | The tests for the phases your uncommitted changes touch | While iterating, when `cargo test-fast` is still too slow |
| `cargo test-phase <phase>` | The tests for one phase, for example `typeck` or `llvm` | When you know which phase you are working on |
| `cargo test-all` | Everything, LLVM backend included, without stopping at failures | Before you push |
| `cargo test --workspace --all-features --doc` | The doctests, which nextest does not run | Before you push |

`cargo test-fast` builds only the `flux` package with default features, the way
`cargo build` does. The tests of `flux-lsp`, `flux-dap` and `xtask` run under
`cargo test-all`, or under `cargo test-changed` when you change those crates.
`cargo test-all` builds with `--all-features`, which is a separate build of the
compiler, so the first `cargo test-all` after a change pays for it.

`cargo test-fast` and `cargo test-all` take nextest arguments. For example,
`cargo test-fast -E 'test(/effect/)'` runs only the tests whose names mention
effects, and `cargo test-fast --no-fail-fast` keeps going after a failure.

`cargo test-fast` also skips the three replay stress tests, which run one
program a hundred times. `cargo test-all` runs them.

## A typical loop

Widen the run as you get more confident, and stop when you are.

1. While editing: `cargo test-changed`. Seconds, plus the rebuild.
2. Fixing one failing test: `cargo test-fast -E 'test(/name/)'`.
3. Before committing: `cargo test-fast`.
4. Before pushing: `cargo test-all` and the doctests.

Times measured on an Apple M2 Pro (12 cores), with the build already cached:

| Command | Test run |
|---|---|
| `cargo test-all` | 16½ min (987s) |
| `cargo test-fast` | 3 min (186s) |
| `cargo test-changed` after a change in `src/types/` | 6s (about 500 tests) |
| `cargo test-changed` after a change to one example | 2s (1 test) |
| `cargo test-changed` after a change in `src/llvm/` | 8 min (274 native tests) |

A change to one source file rebuilds the compiler in about 30s. Backend
changes stay slow because each native test compiles and links a real program.

## Seeing where the time goes

- Put `time` in front of any command. `real` covers build, listing and run.
- Nextest prints the build (`Finished ... in 31s`), then the run
  (`Summary [ 186s ]`), and each test's own time (`PASS [ 2.1s ]`). Tests over
  60s are marked `SLOW`.
- `cargo test-all` writes a JUnit report to `target/nextest/ci/junit.xml`,
  with a time for every test. Sort it to find the slowest.
- `cargo build --tests --timings` writes
  `target/cargo-timings/cargo-timing.html`, which shows how long each crate
  took to compile.

## macOS: slow start before every run

If `real` is minutes longer than the build plus the run, macOS is checking the
test binaries. Nextest starts every test binary twice to list its tests, and
Gatekeeper checks each launch. With one binary per file that added about 200s
to every run; with one per folder there are 16.

Add the app you run cargo from (Terminal, iTerm or VS Code) under System
Settings > Privacy & Security > Developer Tools, then restart it. To check,
`time cargo nextest run --no-run` should take seconds once nothing needs
rebuilding.

## Why the fast tier skips the LLVM tests

The LLVM tests compile under `#[cfg(feature = "llvm")]`, so without that
feature they are not built at all. Each of them also builds and links a native
program, which makes them the slowest part of the suite. `cargo test-all` runs them
in a nextest test group capped at four at a time, so they don't swamp the
machine. The group and both profiles are defined in
[`.config/nextest.toml`](.config/nextest.toml).

## Affected tests

`cargo test-changed` (short for `cargo xtask affected`) looks at the files git
reports as changed: staged, unstaged and untracked. With a branch argument, for
example `cargo test-changed main`, it also looks at everything committed since
that branch. It then runs the tests for the matching phases.

- A file under `src/` selects its phase. For example, `src/types/` selects the
  type-inference tests and `src/llvm/` selects the native tests (with
  `--all-features`).
- A test file selects its own tests: its module of the folder's test binary.
- A helper in `tests/support/` selects the tests that use it.
- A fixture or snapshot selects the tests that name it.
- A change in `crates/flux-lsp`, `crates/flux-dap` or `xtask` selects that
  package and everything that depends on it.
- Anything it has no rule for (`Cargo.toml`, the Flux stdlib in `lib/`, the
  pipeline driver in `src/compiler/`, shared code) runs `cargo test-fast` instead.

The whole compiler is one crate. Cargo sees every test as depending on every
source file, so this selection is a judgement, not a guarantee. A change in one
phase can break a test filed under another. `cargo test-all` is the check that counts.

To see what would run without running it, pass `--dry-run`. To ask about
specific paths instead of git's view, pass `--path`, once per path. Arguments
after `--` go to nextest:

```sh
cargo test-changed --dry-run --path src/types/scheme.rs
cargo test-changed -- --no-fail-fast
```

Some fixtures in `tests/parity/` are not run by any Rust test, only by
`flux parity-check`. When you change one of these, `cargo test-changed` prints the
`parity-check` command instead.

## Phases

`cargo test-phase --list` (short for `cargo xtask phase --list`) prints the
phases. Most of them correspond to a directory under `tests/`, plus the unit
tests in the matching `src/` module.
`effects` and `typeclasses` cut across directories, so they select tests by
name. The mapping is in [`xtask/src/affected.rs`](xtask/src/affected.rs).

## CI

Both CI workflows run `cargo test-all` followed by the doctests. In CI, nextest
writes a JUnit report to `target/nextest/ci/junit.xml`.

To spread the suite over several runners, add a partition to the matrix and
pass `--partition hash:<n>/<total>` to `cargo test-all`. Each shard builds the
tests separately, unless the build is shared through `cargo nextest archive`.
So check that a shard actually finishes sooner before you turn it on.
