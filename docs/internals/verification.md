# Verification

This page explains how to check a change before it merges, and what to do
when a check fails. The tools come from proposal
[0190](../proposals/implemented/0190_verification_toolkit.md), which records
why each one exists.

Flux has two backends that must print the same output for every program.
The tests alone can't show that, so three kinds of check run on top of them:

- **Parity** runs each fixture on the VM and on the native backend, with and
  without the optimizer, and compares the results.
- **Invariants** check things that must hold for any program: compiling a
  file twice gives the same output, every IR is valid after every pass, and a
  native program frees what it allocates.
- **The fuzzer** writes new programs that no one has written yet and runs
  them through parity.

All of them run through `cargo xtask`. The `//!` header of
[`xtask/src/main.rs`](../../xtask/src/main.rs) is the full reference for
every command and flag. This page covers the common cases.

## Before you merge

Run `verify` on your branch, with a tag that names the run, and compare it
with a run on `main`:

```sh
# Once, on main: a run to compare against
git switch main && cargo build --features llvm
cargo xtask verify main

# On your branch
git switch my-branch && cargo build --features llvm
cargo xtask verify my-change --base main
```

A full run takes about 15 minutes. Its output goes to `.verify/<tag>/`, which
git ignores, and ends with a summary.

Add `--cmp` when the change is in a soundness area: types, effects, Aether,
lowering, the backends or the runtimes (the list is in
[`docs/llm_policy.md`](../llm_policy.md#soundness-areas)). Without `--cmp`,
`verify` compares the compiler's dumps with the base run only to find files
that newly fail to compile. With `--cmp`, any changed dump fails the run, so
a change that shouldn't alter the compiler's output can prove it didn't.
`--cmp=structure` is the gentler form: it fails only on a dump that changed
more than its numbering.

`verify` builds native programs by the thousand, so don't run it alongside
another heavy suite (`native_llvm/`, `debug/`, `parity/`, `parity_debug/`).
They slow each other down until tests time out.

### Running part of it

- `--only parity,metrics` runs only those checks.
- `--skip suite` runs everything else.
- `--jobs <n>` sets how many files run at once. The default is the number of
  cores minus 2.

The check names are `fmt`, `clippy`, `suite`, `sweeps`, `parity`, `metrics`,
`behaviour`, `debug-build-parity`, `debug-parity` and `heap-parity`. A
partial run says in its summary what it skipped, since a merge needs them
all.

`cargo xtask prune` deletes old runs and keeps the newest five.

## Reading the summary

This is the summary of a real run, trimmed:

```
ok   fmt                                                                    0:01
ok   clippy                                                                 0:06
ok   tests (exit 0)                                                         6:14
     sweep cfg                                                              0:38
ok   determinism cfg (0 of 50 rerun file(s) differ)                         0:03
ok   determinism cfg shuffled (seed 9334508582089782708) (0 of 50 rerun file(s) build a different program)
     cfg: 0 internal error(s)
ok   compare sweep_cfg with s10 (0 dump(s) changed)                         0:00
ok   parity (0 MISMATCH, 0 EXPECTED_OUTPUT_MISMATCH, 0 of 4 dir(s) exited non-zero)  4:00
FAIL metrics (1 count(s) worse than s10, 0 better, 2 only on one side)
       tests/parity/async_select_loser_watchers_reclaimed.flx vm vm_instructions 33493 -> 33543 (+0.1%)
ok   behaviour against s10 (0 file(s) behave differently, 0 of 6 dir(s) exited non-zero)  0:20
ok   debug build parity (0 MISMATCH, 0 EXPECTED_OUTPUT_MISMATCH, exit 0)    0:15
ok   debug parity (0 MISMATCH, 0 EXPECTED_OUTPUT_MISMATCH, exit 0)          0:12
ok   heap parity (0 MISMATCH, 0 EXPECTED_OUTPUT_MISMATCH, 9 file(s) leak, 0 of 2 dir(s) exited non-zero)  1:27
     total                                                                 15:17
```

Each line is one step:

- `fmt`, `clippy` and `tests` are the CI gates.
- Each sweep compiles every `.flx` file under `examples/`, `tests/` and
  `lib/` and saves one dump per file. There are five sweeps: `cfg`, `core`,
  `core-opt`, `aether` and `lir`. Each is followed by two determinism checks
  and an internal-error count.
- `compare` lines compare each sweep with the base run's.
- `parity` runs the main ways (`vm`, `vm_cached`, `vm_strict`, `vm_opt`,
  `vm_verify`, `llvm`, `llvm_opt` and `llvm_verify`) over four folders:
  `examples/type_classes`, `examples/generics`, `tests/flux` and
  `tests/parity`.
- `metrics`, with `--base`, compares the counts that `flux --metrics` reports
  for each fixture.
- `behaviour`, with `--base`, runs each fixture again with the base run's
  compiler.
- `debug build parity` builds each guide example natively with and without
  `-g`.
- `debug parity` runs each `tests/parity_debug/` session in the VM debugger
  and in lldb.
- `heap parity` runs `tests/parity` and `examples/aether` natively on the
  debug heap.

The run above passed everything except `metrics`. That one count is a known
flake, explained below.

## When a step fails

Each step writes a log in `.verify/<tag>/`, and the summary names it.

| Step | What a failure means | What to do |
|---|---|---|
| `fmt`, `clippy` | Formatting or a lint. | Run `cargo fmt --all` or fix the lint. |
| `tests` | A test failed. | Read `test.txt`. Rerun one test with `cargo test-fast -E 'test(/name/)'`. |
| `snapshots` | A snapshot test wrote a `.snap.new` file. | Run `cargo insta review` and read each change before you accept it. |
| sweep internal errors | The compiler panicked or failed inside on some file. | `sweep_<mode>/results.tsv` lists each file with its class; look for `ice`, `panic` or `crash`. Run `flux <file> --no-cache` to see it. |
| `determinism` | Compiling the same file twice gave different dumps. | The compiler depends on something it shouldn't, such as hash-map order. `cargo xtask compare <a> <b> --content` on the two sweep folders shows which files and how. |
| `determinism … shuffled` | Compiling with the input order shuffled built a different program. | Same as above. A change of numbering alone doesn't fail; a different program does. |
| `compare` | A file newly fails to compile, or, with `--cmp`, a dump changed. | If the change was meant to alter the output, look at the diffs and say so in the PR. If it wasn't, that's a bug. |
| `parity` | Two ways disagree, or a fixture's expected output no longer matches. | Read `parity.txt`, then follow [CONVENTION.md](../../tests/parity/CONVENTION.md#how-to-debug-a-parity-failure) for that kind of mismatch. |
| `metrics` | A count got worse than in the base run. | If the change was meant to cost more, rerun with `--accept-metrics`. `vm_allocs` and `compiler_allocs` may move by 1%; every other count must match exactly. See the note below. |
| `behaviour` | A fixture prints something else, or exits differently, than with the base run's compiler. | The log shows the first line that differs. This catches a change that breaks both backends the same way, which parity can't see. |
| `debug build parity` | A guide example behaves differently when built with `-g`. | Read `parity_g.txt`. |
| `debug parity` | The two debuggers disagree, or a session no longer shows its `// dbg-check:` lines. | If the change was meant to alter what a session shows, re-bless: `flux parity-check tests/parity_debug --ways vm_debug,llvm_debug --bless`, then read the diff. |
| `heap parity` | A native program used freed memory, freed twice, or leaked. | A fault names both sites; see [debug-heap failures](../../tests/parity/CONVENTION.md#8-for-debug-heap-failures). For a leak, fix it, or file a known issue and add an `expect-leak` marker that cites it. |

**About `metrics` and KI-212.** Some async fixtures run a slightly different
number of instructions from run to run, depending on timing. A `metrics`
failure that names only an `async_*` or task fixture's `vm_instructions` or
`vm_allocs`, such as the one in the sample summary, is
[KI-212](../known_issues.md#ki-212), not your change.

**About `behaviour`.** It needs the compiler the base run kept, which only a
run with sweeps keeps. If the base run skipped its sweeps, the step fails and
says so. Both compilers read your branch's `lib/Flow`, so a change to the
standard library itself isn't compared.

## Finding a bug the tests missed

### The fuzzer

`cargo xtask fuzz` writes well-typed programs from a seed and runs each one
that compiles through parity, on `vm`, `llvm`, `vm_opt`, `llvm_opt`,
`vm_verify` and `llvm_verify`:

```sh
cargo xtask fuzz --count 200 --jobs 4
cargo xtask fuzz --count 200 --with classes,effects
cargo xtask fuzz --count 200 --with async --reruns 10
```

`--with` adds parts of the language: `classes` (generics and type classes),
`values` (records, closures as values, arrays, maps and `Either`), `effects`
(user effects and handlers), `async` (tasks and channels) and `modules` (a
module beside `main`). `--reruns 10` runs each program ten more times on
each way and reports one whose output changes.

A program the compiler fails on is a compiler bug. A program it rejects with
a diagnostic is a bug in the generator. The cases go to
`.verify/fuzz_<seed>/`. The same seed always writes the same programs.

### Shrinking a case

`fuzz reduce` shrinks a failing case to a small program that still fails the
same way:

```sh
cargo xtask fuzz reduce .verify/fuzz_1/case_130.flx
```

It writes `case_130.min.flx` beside the case. A case from `--with modules`
is a folder, and shrinks to a folder.

### Naming the optimizer pass

When a fixture differs only under `--optimize`, `bisect-opt` finds the first
rewrite that causes it and shows the function before and after:

```sh
cargo xtask bisect-opt tests/parity/some_fixture.flx --way vm_opt
```

It works by numbering every optimizing rewrite and searching with
`flux --opt-bisect-limit <n>`, which stops optimizing after rewrite `n`.

### Naming the pass that broke the IR

`--verify-each` checks the IR after every compiler pass, instead of at a few
points. A pass that leaves invalid IR stops the build with an error that
names it:

```sh
cargo run --features llvm -- program.flx --native --optimize --verify-each
```

The errors are `E998` (Core lint), `E999` (Aether's ownership contract) and
`E997` (CFG or LIR). The `vm_verify` and `llvm_verify` parity ways run every
fixture this way.

### Other checks

- `cargo xtask rewrite` rewrites each fixture in ways that can't change its
  output, such as reordering top-level definitions or renaming variables, and
  checks that the output stays the same.
- `cargo xtask sweep --out .verify/det --determinism --shuffle` compiles
  everything twice, once with the input order shuffled, and reports any
  difference.
- `cargo xtask sanitize` runs the native ways over `tests/parity` with the C
  runtime built with AddressSanitizer and UndefinedBehaviorSanitizer.
  `--thread` uses ThreadSanitizer instead.

## Seeing what works

`cargo xtask census` runs every `.flx` file in the repository on `vm`,
`llvm`, `vm_opt` and `llvm_opt` and sorts each result into a class: works,
wrong, rejected, crashed, and so on. It tags each file with the language
features it uses and matches each failure to a known issue. The result is a
feature-by-way table of what works, in `.verify/census_<tag>/`:

```sh
cargo xtask census --tag c2 --base c1    # and what changed since c1
```

A failure with no known issue is listed in `unfiled.txt`. Each one is either
a wrong fixture header or a bug that needs a known issue.
[CONVENTION.md](../../tests/parity/CONVENTION.md#the-support-census) lists
the classes.

## Where things live

| Path | Holds |
|---|---|
| [`xtask/src/main.rs`](../../xtask/src/main.rs) | every `cargo xtask` command and flag, in its `//!` header |
| [`xtask/src/verify.rs`](../../xtask/src/verify.rs) | the `verify` steps |
| [`xtask/src/fuzz/`](../../xtask/src/fuzz/) | the generator and the reducer |
| [`src/parity/`](../../src/parity/) | `flux parity-check`; see [parity.md](parity.md) |
| [`tests/parity/CONVENTION.md`](../../tests/parity/CONVENTION.md) | how to write a fixture and debug a parity failure |
| `.verify/<tag>/` | a run's logs and summary (ignored by git) |
