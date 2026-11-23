# The parity checker

> Source: [`src/parity/`](../../src/parity/)

`flux parity-check` runs each fixture in several ways and compares the
results. A *way* is one backend in one mode: the VM, the native backend, the
native backend with `--optimize`, and so on. The checker's main job is to
catch a program that behaves differently on the VM and on the native backend.
It compares what each way prints, how each way exits, and the diagnostics a
failing program reports.

Run it when you change the runtime, lowering, diagnostics, type checking, the
standard library or what a program means.

This page is the reference for the checker. To write a fixture, read
[`tests/parity/CONVENTION.md`](../../tests/parity/CONVENTION.md). To check a
change before it merges, read [verification.md](verification.md).

## Running

```bash
# One file, or every .flx file under a folder
cargo run -- parity-check tests/parity/adt_option_equality.flx
cargo run -- parity-check tests/parity

# Choose the ways (the default is vm,llvm)
cargo run -- parity-check tests/parity --ways vm,llvm,vm_opt,llvm_opt

# Check 8 files at a time; results still print in file order
cargo run -- parity-check tests/parity --jobs 8

# Write what the ways printed into each fixture's expected blocks
cargo run -- parity-check tests/parity/new_fixture.flx --bless
```

The ways that build native code need a compiler built with LLVM. Running
through `cargo run --features llvm -- parity-check …` is the usual form. The
checker builds its own copies of the compiler, `target/parity_vm/debug/flux`
and `target/parity_native/debug/flux`. It rebuilds them when they are missing
or older than the source. `--rebuild` forces a rebuild.

All the options:

| Option | What it does |
|---|---|
| `--ways <w,…>` | The ways to run. See [Ways](#ways). |
| `--jobs <n>` | Check `n` files at once. The default is 1. |
| `--show pass\|fail\|all` | Print only passing or failing fixtures. |
| `--timeout <secs>` | The time each way gets per file. |
| `--root <path>` | A module root, passed to `flux`. It can be repeated. |
| `--allow network` | Run fixtures marked `// requires: network`. |
| `--compile` | Compile every fixture first, then run parity against the cached builds. See [The `--compile` mode](#the---compile-mode). |
| `--bless` | When every way ran and they agree, write what they printed into the fixture. See [Updating expected output](#updating-expected-output). |
| `--annotate` | Move a passing fixture from `// expect-error:` lines to `//~` annotations (proposal 0191). |
| `--check-skips` | Also run the ways a `// skip-way:` removes, outside the verdict, and print `UNEXPECTED_PASS` for each one that would now pass. |
| `--known-issues <file>` | Fail a fixture whose skip or `expect-leak` reason names a known issue that is not open in `<file>`, usually `docs/known_issues.md`. |
| `--capture-core`, `--capture-aether`, `--capture-repr`, `--capture-cfg`, `--capture-lir`, `--explain` | Compare the compiler's intermediate output between ways. See [Finding where two ways split](#finding-where-two-ways-split). |
| `--debug-first-failure`, `--save-debug-dir <path>` | Stop at the first failure and print more detail, or save its files. |
| `--vm-binary <path>`, `--llvm-binary <path>` | Use these compilers instead of the checker's own. |
| `--census <file.tsv>` | Classify every file and way and write one row each. Used by `cargo xtask census`. |
| `--metrics <file.tsv>` | Write the `flux --metrics` counts of each file's `vm` and `vm_opt` runs. Used by `cargo xtask verify`. |
| `--behaviour-of <path>` | Run the `vm` way again with another compiler and compare its stdout and exit kind. Used by `cargo xtask verify`. |

Running `flux parity-check` with no arguments prints the same list, after an
error that says a file or folder is missing.

## How it works

```
fixture.flx
    │
    ▼
┌─────────────────┐     ┌─────────────────┐
│ parity_vm/flux  │     │ parity_native/  │
│  VM backend     │     │  flux --native  │
└────────┬────────┘     └────────┬────────┘
         │ stdout, stderr,       │ stdout, stderr,
         │ exit kind             │ exit kind
         ▼                       ▼
    ┌────────────────────────────────┐
    │       normalize and compare    │
    │  (src/parity/normalize.rs)     │
    └──────────────┬─────────────────┘
                   ▼
               verdict
```

Each way runs as a separate process with `NO_COLOR=1` and its own empty cache
folder, unless the way is one of the cached ones.

Before comparing, [`normalize.rs`](../../src/parity/normalize.rs) removes
output that says nothing about the program: build progress, backend banners,
temporary paths, a leading `./` on project paths, ANSI colors, and type
variable numbers such as `var #10603`.

## Ways

The default is `vm,llvm`. A fixture's `// parity:` line chooses its own ways,
and `--ways` on the command line replaces that choice. A `// skip-way:` line
still removes a way either way.

| Way | Runs | Compared with | Use it to check |
|---|---|---|---|
| `vm` | VM, `--no-cache` | the other ways | the VM |
| `llvm` | native, `--native --no-cache` | the other ways | the native backend |
| `vm_cached` | VM, with a cache | `vm` | that a cached build behaves like a fresh one |
| `llvm_cached` | native, with a cache | `llvm` | the same, natively |
| `vm_strict` | VM, `--strict` | `vm` | strict mode (see below) |
| `llvm_strict` | native, `--strict` | `llvm` | strict mode, natively |
| `vm_opt` | VM, `--optimize` | `vm` | that the optimizer changes nothing a program does |
| `llvm_opt` | native, `--optimize` | `llvm` | the same, natively; `--optimize` also raises LLVM's level |
| `vm_verify` | VM, `--optimize --verify-each` | `vm_opt` | that every IR is valid after every pass |
| `llvm_verify` | native, `--optimize --verify-each` | `llvm_opt` | the same for the native pipeline, which also checks LIR |
| `llvm_heap` | native, `--leak-check` | `llvm` | use after free, double release and leaks |
| `llvm_g` | native, `-g` | the other ways | that debug info changes nothing a program does |
| `vm_debug` | `flux debug --script` | `llvm_debug` | the VM debugger against lldb |
| `llvm_debug` | `-g` build in `lldb -b` | `vm_debug` | the native debugger session |

Each pair has its own rule:

- **A cached way** must give the same exit kind and stdout as its fresh way,
  and the same stderr when either fails.
- **A strict way** may reject a program the plain way accepts, since strict
  mode reports more. When both succeed, their stdout must match. When the
  plain way fails, the strict way must fail the same way.
- **An opt way** must give the same exit kind and stdout as its plain way,
  and the same stderr when it fails. A difference is a miscompile.
- **A verify way** must behave like its opt way. A failed IR check stops the
  compiler with `E997`, `E998` or `E999`, which counts as a crash. A fixture
  that skips an opt way skips its verify way too.
- **`llvm_heap`** must behave like `llvm`. A use after free or a double
  release is a `heap_fault`. A block still live at exit fails the fixture
  unless an `// expect-leak:` line allows it.
- **The debug ways** run the fixture's `// dbg:` script and compare the two
  reduced transcripts. Each transcript must also show the fixture's
  `// dbg-check:` lines. The fixtures are in `tests/parity_debug/`, and
  [their README](../../tests/parity_debug/README.md) explains the format.

## What makes a fixture pass

A fixture that expects success passes when:

1. every way has the same exit kind,
2. the normalized stdout is the same on every way,
3. the normalized stderr is the same on every way, if any way failed,
4. the stdout matches the fixture's expected stdout block, if it has one,
5. each pair above follows its rule,
6. any counts it states with `// expect-reuse:` or `// expect-allocs:` hold.

A fixture that expects `compile_error` or `runtime_error` passes when:

1. every way fails in that phase,
2. the diagnostic codes match its `// expect-error:` lines or `//~`
   annotations,
3. the normalized stderr matches its expected stderr block, if it has one,
4. the normalized stderr is the same on every way.

An expected failure is a claim, not a skip. The phase, the codes and the
stderr must all be right.

## Exit kinds and verdicts

Each run ends in one exit kind: `Success`, `CompileError`, `RuntimeError`,
`Timeout`, `ToolFailure` or `Crash`. A crash is the compiler failing, not the
program: a panic, an internal compiler error, a failed IR check, a native
lowering or link failure, or a signal. A crash never equals an ordinary
error, so a crash on one way and a diagnostic on another is a mismatch.

Each fixture gets one verdict, defined in
[`src/parity/mod.rs`](../../src/parity/mod.rs):

| Verdict | Printed as | Meaning |
|---|---|---|
| `Pass` | `PASS` | Everything the fixture claims held. |
| `Mismatch` | `MISMATCH` | Two ways differ, or a pair broke its rule. The details say which. |
| `ExpectedOutputMismatch` | `EXPECTED_OUTPUT_MISMATCH` | The ways agree, but not with what the fixture declares. |
| `Skip` | `SKIP` | The fixture asked to be skipped, or a requirement such as `// requires: unix` isn't met. |

A run also prints these lines, which are not verdicts:

- `SKIP_WAY <file> <way>: <reason>` for each way a `// skip-way:` removed.
- `UNEXPECTED_PASS <file> <way>: KI-nnn may be fixed`, under `--check-skips`,
  for a skipped way that would now pass, or an `expect-leak` that no longer
  leaks.
- `LEAK <file> <way>: <n> blocks live at exit, first <site>`, for a heap way
  that leaked. The leak has already failed the fixture if no marker allows it.

The summary at the end counts each kind. `parity-check` exits with 1 when any
fixture is `MISMATCH` or `EXPECTED_OUTPUT_MISMATCH`, or when no fixture
passed at all. A `--census` run never fails: it only records.

## Fixture metadata

A fixture states what it expects in `//` comments at the top of the file,
parsed by [`src/parity/fixture.rs`](../../src/parity/fixture.rs).
[`tests/parity/CONVENTION.md`](../../tests/parity/CONVENTION.md) describes
each line. In short:

| Line | Says |
|---|---|
| `// parity:` | the ways to run |
| `// expect:` | `success`, `compile_error`, `runtime_error` or `any` |
| `// expect-error:` or `//~` | the diagnostic codes, or the codes on their lines |
| `parity-expected-stdout-begin` … `-end` | the expected stdout |
| `parity-expected-stderr-begin` … `-end` | the expected stderr of a failing fixture |
| `// skip:` | skip the whole fixture, with a reason |
| `// skip-way:` | leave out some ways, citing an open KI |
| `// requires:` | `unix`, `windows` or `network` |
| `// stdout: varies` | compare exits only, not stdout |
| `// expect-reuse:`, `// expect-allocs:` | what Aether must achieve |
| `// expect-leak:` | the leak a heap way may report, citing an open KI |
| `// dbg:`, `// dbg-check:` | a debug fixture's script, and what each debugger must show |
| `// root:`, `// flags:`, `// bug:`, `// census-skip:`, `// feature:` | the rest; see CONVENTION.md |

Comments that are not one of these lines are ignored, since a header is also
prose. A known line with a value the parser can't read, such as
`// expect: sucess`, fails the fixture, so a typo can't quietly change what
is checked.

### Folder defaults: `parity.dir`

A `parity.dir` file gives the same lines, without the `//`, to every fixture
in its folder and the folders below. The nearest folder wins, and a fixture's
own header wins over any folder. A line that starts `name.flx:` applies to
that one file. Use it when a header line would move line numbers that a
snapshot or a `// dbg:` script depends on:

```
# tests/parity/parity.dir
json_derived_codecs.flx: expect-leak: llvm_heap <= 10: KI-209
```

## Updating expected output

`--bless` writes what the ways printed into the fixture, but only when every
way ran and they all agree:

- A fixture that expects success gets its stdout block written, or added
  after its header.
- A fixture that expects an error has its existing stderr block replaced.
- An annotated fixture has its `//~` annotations rewritten.
- A debug fixture gets the VM debugger's transcript as `// dbg-check:` lines
  at the end of the file.

An inline block can move the line numbers in the diagnostics it pins, so run
`--bless` again until nothing changes. Review the diff before you commit it.
A changed stderr block is a change to what users see.

## The `--compile` mode

`--compile` runs in two phases.

**Phase 1** compiles every fixture. A fixture that expects success or a
runtime error is compiled with the cache on, which also fills the cache for
phase 2. A fixture that expects a compile error is compiled fresh, as normal
parity runs it. A failed compile leaves nothing useful in the cache, and a
cache folder can change which diagnostics follow the first.
Phase 1 prints one of these per fixture:

| Line | Meaning |
|---|---|
| `COMPILE OK` | compiled |
| `COMPILE EXPECTED FAIL` | failed in the expected phase |
| `COMPILE DIAGNOSTIC MISMATCH` | reported other codes than `expect-error` |
| `COMPILE STDERR MISMATCH` | the ways printed different stderr |
| `COMPILE EXPECTED STDERR MISMATCH` | the stderr differs from the fixture's block |
| `COMPILE UNEXPECTED OK` | compiled, but was expected to fail |
| `COMPILE FAIL` | failed, but was expected to compile |
| `COMPILE SKIP` | skipped |

If phase 1 finds a real failure, phase 2 doesn't run and the exit code is 1.

**Phase 2** runs parity. Fixtures that expect success or a runtime error run
on the cached ways, `vm_cached` and `llvm_cached`, unless their header chose
other ways. Fixtures that expect a compile error stay on their fresh ways.

## Finding where two ways split

These options compare the compiler's intermediate output between ways:

| Option | Compares |
|---|---|
| `--capture-core` | Core IR, shared by both backends |
| `--capture-aether` | Aether's ownership plan |
| `--capture-repr` | the backend representation contract |
| `--capture-cfg` | CFG IR, the VM backend's |
| `--capture-lir` | LIR, the native backend's |
| `--explain` | several of these, to show where the split began |

Work down the pipeline: Core, then Aether, then the representation, then CFG
or LIR. If Core differs, the bug is in the frontend or the shared lowering.
If Core matches and CFG or LIR differs, it is in a backend.
[CONVENTION.md](../../tests/parity/CONVENTION.md#how-to-debug-a-parity-failure)
walks through each kind of failure.

## Stale caches

If cached output from an older compiler causes confusing failures, delete the
cache folders:

```bash
rm -rf target/flux/native target/flux/interfaces target/flux/vm target/parity-cache
```

## Extending the checker

To add a way:

1. add a `Way` variant in [`src/parity/mod.rs`](../../src/parity/mod.rs),
   with its name in `parse`, `Display` and `Way::ALL`,
2. give it its flags in [`src/parity/runner.rs`](../../src/parity/runner.rs),
3. if it pairs with another way, add its rule to `judge` in
   [`src/parity/cli.rs`](../../src/parity/cli.rs),
4. show it in [`src/parity/report.rs`](../../src/parity/report.rs) and the
   census (`census.rs`),
5. add tests.

To add a comparison:

1. add a `MismatchDetail` variant,
2. compare in [`src/parity/cli.rs`](../../src/parity/cli.rs),
3. print it in [`src/parity/report.rs`](../../src/parity/report.rs),
4. add tests.

## Files

| File | Holds |
|---|---|
| `src/parity/mod.rs` | ways, exit kinds, verdicts and mismatch details |
| `src/parity/cli.rs` | options, the fixture loop, the pair rules, `--bless` and `--annotate` |
| `src/parity/runner.rs` | running each way as a process |
| `src/parity/fixture.rs` | header lines and `parity.dir` |
| `src/parity/normalize.rs` | output normalization |
| `src/parity/report.rs` | printing results and explaining mismatches |
| `src/parity/debug.rs` | the debug ways: running sessions, reducing transcripts, `dbg-check` |
| `src/parity/census.rs` | the support census rows |
| `src/parity/known_issues.rs` | checking that a cited KI is open |

## Related

- [Verification](verification.md): checking a change before it merges
- [Parity fixture convention](../../tests/parity/CONVENTION.md)
- [Compiler architecture](compiler_architecture.md)
- [Backend representation contracts](backend_representation_contracts.md)
- [Aether debugging](aether_debugging.md)
