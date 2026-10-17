- Feature Name: `verification_toolkit`
- Start Date: 2026-10-16
- Proposal PR:
- Flux Issue:
- Status: In progress: S1–S3 done on `feat/verification-toolkit`
- Depends on: [0138](implemented/0138_flux_parity_ways.md) (parity ways and `flux parity-check`), [0188](0188_flux_debugger.md) S6b (the debug parity harness, `// dbg:` fixtures)
- Relates to: [0188](0188_flux_debugger.md) S9c (`llvm_g` and the guide in `xtask verify`)

# Proposal 0190: Verification toolkit — failing parity, optimizer ways, dump comparison, fuzzing

## Summary
[summary]: #summary

`cargo xtask verify` gains what other compilers' pre-merge checks already have:

- **Every parity failure fails the run** (S1). Known gaps are recorded per way in
  the fixture with `// skip-way:`, as Zig's behavior tests do (S2).
- **The optimizer is executed.** New `vm_opt` and `llvm_opt` ways run every parity
  fixture under `--optimize`, as GHC's testsuite runs each test in several ways (S3).
- **Dumps are compared by content.** Today they are compared by status only.
  `--cmp` fails a refactor that changes any Core or CFG dump, as Go's
  `toolstash -cmp` does. A determinism check comes first (S4).
- **Debugger fixtures carry expected text.** `// dbg-check:` lines catch a bug that
  the VM debugger and lldb share, as rustc's `lldb-check` lines do (S5).
- **A differential fuzzer** generates well-typed programs and runs them through
  parity, as Csmith and YARPGen do for C compilers (S6). A reducer shrinks what it
  finds (S7).

## Motivation
[motivation]: #motivation

### What exists today

`cargo xtask verify <tag> [--base <tag>]` (`xtask/src/verify.rs`) runs:

1. `cargo fmt --check` and `cargo clippy -D warnings`.
2. The suite, `cargo test --all --all-features --no-fail-fast`.
3. Three compile-only sweeps of every `.flx` file under `examples/`, `tests/` and
   `lib/` (`xtask/src/sweep.rs`):
   - `cfg`: `--dump-cfg`
   - `core`: `--dump-core`
   - `core-opt`: `--optimize --dump-core`

   With `--base`, each sweep is compared with the base tag's (`xtask/src/compare.rs`).
4. Three `flux parity-check` runs (`src/parity/`):
   - `vm,vm_cached,vm_strict,llvm` over `examples/type_classes`, `examples/generics`,
     `tests/flux` and `tests/parity`
   - `llvm,llvm_g` over `examples/guide`
   - `vm_debug,llvm_debug` over `tests/parity_debug`

### What it misses

- **Expected-output failures pass.** `parity()` in `verify.rs` counts lines that
  start with `MISMATCH` and discards the exit code of `parity-check`. Verdicts
  print as `EXPECTED_OUTPUT_MISMATCH` (`src/parity/report.rs`), so a fixture whose
  expected stdout no longer matches, or an `expect: success` fixture that fails,
  never fails verify. This applies even in the `-g` and debug steps, which
  otherwise fail on a mismatch.
- **The main parity step never fails.** It records a note: "compare with the
  base's parity log". A known divergence can only be recorded by skipping the
  whole fixture (`// skip:`). verify passes `--ways`, which overrides each
  fixture's `// parity:` line, so a fixture cannot say "this way only".
- **The optimizer is never run.** `--optimize` appears only in the `core-opt` sweep,
  which compiles and dumps. No parity way passes it (`build_way_args` in
  `src/parity/runner.rs`), and a single file run applies no package profile. A
  miscompile under `--optimize` is found only if someone runs it by hand.
- **Dump changes go unnoticed.** `compare.rs` reads only `results.tsv` (status and
  diagnostic codes). The `runs/*.txt` files keep the last 40 lines of a dump and are
  never compared. A refactor that changes the Core of every program, but still
  compiles them, passes.
- **The two debuggers can agree and both be wrong.** Debug parity compares the VM
  debugger's transcript with lldb's (`outputs_match`, `src/parity/debug.rs`). There
  is no expected transcript, so a bug in the shared debug-info model
  (`src/debuginfo/`) that both engines read shows the same wrong value on both
  sides.
- **Parity sees only hand-written programs.** There is no generator, fuzzer or
  property-test crate in the workspace. The roughly 140 fixtures in `tests/parity/`
  cover the cases someone thought of.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### A known divergence

A fixture that the native backend gets wrong, tracked as KI-091:

```flux
// skip-way: llvm, llvm_opt: KI-091 Float formatting of -0.0
fn main() with IO {
    print(0.0 * -1.0)
}
```

`parity-check` runs the other ways, prints
`SKIP_WAY tests/parity/neg_zero.flx llvm: KI-091 …` and counts it under
`SkippedWays:` in the summary. When KI-091 is fixed, the line is deleted. A
mismatch in any way that is not skipped fails verify.

### A refactor that should change nothing

```
cargo xtask verify main --skip-suite                  # once, on main
cargo xtask verify my-refactor --base main --cmp
```

`--cmp` compares every Core and CFG dump byte for byte with main's and fails on
the first difference. It prints the file, the first differing line, and a few
lines of context from each side. Without `--cmp`, changed dumps are counted in a
note.

### A debugger fixture with expected text

```flux
// dbg: break 14
// dbg: continue
// dbg: locals
// dbg-check: stop closures.flx:14
// dbg-check: moved = 11
// dbg-check: offset = 10
```

Both engines must still agree with each other. Each must also contain the
`dbg-check` lines in order.

### Fuzzing

```
cargo xtask fuzz --seed 1 --count 200
cargo xtask fuzz reduce .git/parked/out/fuzz_1/mismatch/case_37.flx
```

The first command writes 200 generated programs and runs them with
`vm,llvm,vm_opt,llvm_opt`. Mismatches are copied to `mismatch/`. The second
command shrinks one of them to the smallest program that still mismatches in the
same way. Once the bug is fixed, the result goes to `tests/parity/` as a fixture.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### §1 Counting parity failures

`parity()` in `xtask/src/verify.rs` returns `ParityCounts { mismatches, expected, exit }`:

- `mismatches` counts lines starting with `MISMATCH`.
- `expected` counts lines starting with `EXPECTED_OUTPUT_MISMATCH`.
- `exit` is the exit code that `run_tee` (`xtask/src/proc.rs`) already returns.

A parity step fails when any count is non-zero or the exit code is. `parity-check`
exits 1 on any mismatch, or when nothing passed (`src/parity/cli.rs`). The outcome
line names both counts.

### §2 `// skip-way:`

- **Syntax:** `// skip-way: <way>[, <way>…]: <reason>`, in the fixture header. It may
  be repeated. The reason must name a KI. `parse_fixture_meta`
  (`src/parity/fixture.rs`) rejects a skip-way line without `KI-` so that skips are
  never anonymous.
- **Meaning:** the named ways are removed from that fixture's way list after
  `--ways` is applied. Unlike `// parity:`, the directive holds when `--ways` is
  given.
  - If fewer than two ways are left and the fixture declares no expected output,
    the verdict is `SKIP`.
  - If it declares expected output, the remaining way is still checked against it.
- **Report:**
  - one `SKIP_WAY <file> <way>: <reason>` line per skipped way;
  - `SkippedWays: n` in the summary.
- **Migration:**
  - Whole-fixture `// skip:` lines become `// skip-way:` where only one backend is
    affected. `ki_073_result_selection_through_constrained_fn.flx` is the first
    candidate. The TCP fixtures stay `// skip:` because they need the network.
  - Every `MISMATCH` left in the main parity run is then either fixed or recorded
    with `// skip-way:` and a KI. Only after that does verify's main parity step
    switch from `note` to `record`.
- **Documentation:** `tests/parity/CONVENTION.md` documents the directive.

### §3 Optimizer ways

- **Ways:** `Way::VmOpt` (`vm_opt`) and `Way::LlvmOpt` (`llvm_opt`) are added to
  `src/parity/mod.rs`, in `Display` and `parse`. `build_way_args` pushes
  `--optimize` for both.
- **Opt check:** modelled on the strict check in `check_file`
  (`src/parity/cli.rs`). Each opt way is compared with its plain way on:
  - `exit_kind`,
  - normalized stdout,
  - normalized stderr when either run failed.

  Opt ways also take part in the ordinary cross-way comparison.
- **verify:** the main parity ways become
  `vm,vm_cached,vm_strict,vm_opt,llvm,llvm_opt`, which is two more runs per
  fixture. A native `--optimize` build is the slowest of these.
- **Measured cost (2026-10-17, S3):** `tests/parity` took 4:49 with the four
  ways before S3 and 8:34 with six, about 78% longer. The main parity step over
  its four directories took 15:15, which puts the four-way step at about 8:35.
  The opt ways therefore add roughly 6½ minutes to a verify run. A
  `--skip-suite --skip-sweeps` run took 20:58 in all. The ways stay in the
  default set for now; verify prints each step's time, so the cost stays
  visible.

### §4 Content comparison and determinism

- **Sweep output:** `sweep_file` (`xtask/src/sweep.rs`) hashes each file's full
  stdout (FNV-1a, 64-bit) into `<sweep>/hashes.tsv`, with its bytes and lines.
  The dumps themselves are not kept. Measured on 2026-10-17, a CFG dump is about
  2.7 MB, most of it the prelude, and a Core dump about 65 KB. Full dumps of the
  1,521-file corpus would take about 4.4 GB per tag. Each sweep keeps the
  compiler it ran, so a dump can be produced again. `runs/` and `results.tsv` are
  unchanged.
- **`cargo xtask compare <base> <head> --content`:** for each file on both sides
  whose status is `ok` on both, it compares the dumps' hashes.
  - For up to 20 changed files, it reruns the base sweep's and the head sweep's
    compilers on the file and prints the first differing line, with three lines
    of context from each side.
  - The full list goes to `<head>/content_diff.txt`.
  - It exits 1 on a changed dump as well as on a status failure.
- **`cargo xtask verify --cmp`:** with `--base`, content changes are always
  counted in a note. With `--cmp` they record a failure.
- **Determinism, a precondition for `--cmp`:**
  - `cargo xtask sweep --determinism --mode <m>` runs the same sweep twice, into
    `<out>_a` and `<out>_b`, with the same binary, then compares them with
    `--content`.
  - A difference is a bug, such as `HashMap` iteration order or fresh-name
    counters that depend on scheduling. It is either fixed in the dump printer or
    filed as a KI.
  - This command is not run by verify, because it doubles the cost of a sweep.

### §5 `// dbg-check:`

- **Syntax:** `// dbg-check: <text>`, in the header of a `tests/parity_debug/`
  fixture. It may be repeated, and is parsed next to `debug_script()`
  (`src/parity/debug.rs`).
- **Semantics:** FileCheck's `CHECK`. The checks must match lines of the reduced
  transcript, in order, each as a substring of a later line than the previous
  check matched. The reduced transcript is what `normalize_vm` / `normalize_lldb`
  produce and what the cross comparison sees.
- **Where it is checked:**
  - `check_file` checks `vm_debug` and `llvm_debug` independently. The first check
    that does not match gives `EXPECTED_OUTPUT_MISMATCH`, naming the way and the
    check.
  - `tests/debug/parity_debug_tests.rs` applies the same checks.
- **Coverage:** all 21 existing fixtures gain checks for their stops, the frames of
  each `bt`, and the locals of each `locals`. The format is documented in
  `tests/parity_debug/README.md`.

### §6 The generator

- **Location:** `xtask/src/fuzz/`. Like the rest of xtask it uses only the
  standard library, so `cargo xtask` still builds in seconds.
  - `rng.rs`: splitmix64. The same seed always gives the same programs, on every
    platform.
  - `gen.rs`: a type-directed generator. It keeps an environment of typed
    variables and functions, and builds an expression of a requested type within a
    depth budget, preferring variables in scope so that values flow.
- **Types:** `Int`, `Bool`, `String`, `Option<T>`, two or three generated `data`
  types (with fields), tuples, lists.
- **Forms:**
  - `let`;
  - `if`;
  - `match`, exhaustive by construction: one arm per constructor, or a final
    wildcard;
  - lambdas that capture, with `map`, `filter` and folds;
  - top-level functions;
  - the pipe operator;
  - string interpolation;
  - recursion through a function whose first argument is an `Int` that decreases
    by a positive literal and is guarded by `if n <= 0`.
- **Observability:** `main` is `fn main() with IO`. Every binding a function
  computes is printed, so a wrong value shows up in stdout and not only in the
  program's final result.
- **Safety:**
  - Divisors and moduli are non-zero literals.
  - Literals stay small, so overflow is rare.
  - Runtime errors are allowed: parity compares `exit_kind`, and stderr when a run
    fails.
- **Layout:** one statement per line, so the reducer (§7) can work line by line.
- **Effects, type classes and async are out of scope.** Generating handlers needs
  effect-row typing, and async needs deterministic scheduling across backends.
  Both are listed under Future possibilities.
- **`cargo xtask fuzz [--seed S] [--count N] [--ways W] [--jobs J]`:**
  - It writes `.git/parked/out/fuzz_<seed>/case_<i>.flx` and runs
    `flux parity-check` on that folder. `parity()` moves from `verify.rs` to a
    module shared by verify and fuzz.
  - It copies mismatching cases to `mismatch/`.
  - A case that fails to compile on every way is a generator bug. These are counted
    on their own line, and the target is zero.
  - Defaults are `--jobs 1` and `--count 50`. Native builds are expensive, and one
    heavy run at a time keeps the machine usable.

### §7 The reducer

- **`cargo xtask fuzz reduce <case.flx> [--ways W]`:** delta debugging (ddmin)
  over lines. A candidate deletes a set of lines; a deletion that splits a brace
  pair is widened to the whole statement or match arm.
- **Keeping a candidate:** it is kept when it compiles on every way and
  `parity-check` reports the same verdict, between the same pair of ways, on the
  same stream (exit kind, stdout or stderr) as the original.
- **Output:** `<case>.min.flx`.
- **Promotion:** a minimized case goes to `tests/parity/` with `// expect: success`
  and a `parity-expected-stdout` block taken from the way that is right. It goes in
  the commit that fixes the bug, or with a `// skip-way:` and a KI if the bug stays
  open.

### §8 Staging

| stage | change | exit |
|---|---|---|
| S1 ✓ | verify counts `EXPECTED_OUTPUT_MISMATCH` and the `parity-check` exit code (§1) | A deliberately broken expected block in `examples/guide` makes `debug build parity` FAIL. Any failure this uncovers is fixed or filed as a KI. |
| S2 ✓ | `// skip-way:` (§2); whole-fixture skips narrowed; the main parity step records instead of noting | Unit tests in `src/parity/fixture.rs` for parsing, including the KI requirement. Main parity on `feat/debugger` has 0 MISMATCH, with every skip naming a KI. |
| S3 ✓ | `vm_opt` / `llvm_opt` and the opt check (§3); added to verify's main parity | `parity-check tests/parity --ways vm,vm_opt,llvm,llvm_opt` has 0 MISMATCH, after fixes or KI skips. The verify run-time increase is recorded in §3. |
| S4 | Dump hashes in sweeps, `compare --content`, `verify --cmp`, `sweep --determinism` (§4) | `cargo xtask sweep --determinism` reports 0 differences for `cfg`, `core` and `core-opt`. `verify <tag> --base main --cmp` on a no-op change passes; on a dump-printer change it fails and names the files. `compare.rs` unit tests. |
| S5 | `// dbg-check:` (§5), checks added to all 21 debug fixtures | Debug parity 21/21. Changing one expected local makes both the parity run and `parity_debug_tests` fail. |
| S6 | The generator and `cargo xtask fuzz` (§6) | Unit tests: the same seed gives the same text; 200 seeds compile with 0 generator failures (an ignored test that runs `flux --dump-core`). A planted miscompile under `--optimize`, not committed, is found within `--count 200`. |
| S7 | `cargo xtask fuzz reduce` (§7) | The planted miscompile's case shrinks to at most 10 lines. Reducer unit tests on a fake oracle. |

S1 comes first, since without it the later stages could fail silently. S2 and S3
go together: the opt ways will find divergences, and S2 is how they are recorded
while they are open. S4 and S5 are independent of the rest. S7 needs S6.

## Drawbacks
[drawbacks]: #drawbacks

- **verify gets slower.** Two more ways in the main parity step, full dumps on
  disk, and expected checks. The content comparison itself is cheap. The opt ways
  are not: §3 records the cost, and the ways can move to an opt-in flag if it is
  too high.
- **Disk use.** Full Core dumps for every corpus file, three modes, per tag, in
  `.git/parked/out/`. Old tags must be pruned by hand.
- **Skips can rot.** A `// skip-way:` that outlives its bug hides a regression in
  that way. Requiring a KI per skip keeps them listed in `known_issues.md`.
- **The generator is code to maintain.** It must track surface syntax. Generator
  compile failures are reported separately so that syntax drift shows up at once,
  rather than as missing coverage.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **A separate allowlist file instead of `// skip-way:`.** Rejected. Parity
  metadata already lives in fixture headers (`tests/parity/CONVENTION.md`), and a
  skip next to the program it describes is removed along with the program.
- **Snapshot every dump with insta instead of `--cmp`.** Rejected. The corpus is
  every `.flx` file in the repository, and most dump changes are intended. `--cmp`
  is for the commits that claim to change nothing. Its baseline is the base tag,
  not a file that has to be blessed.
- **proptest or cargo-fuzz for the generator.** Rejected for now. Coverage-guided
  fuzzing of the parser finds crashes, not miscompiles, and proptest's shrinking
  works on its value tree, not on Flux source. A std-only generator also keeps
  xtask free of dependencies.
- **Expected transcripts as separate golden files for debug parity.** Rejected.
  The `// dbg:` script is already in the fixture, and `dbg-check` lines next to
  it say what each step should show.

## Prior art
[prior-art]: #prior-art

- **Go, `toolstash`.** `toolstash save` keeps the current compiler, and
  `toolstash -cmp` rebuilds the standard library with the new one and compares the
  object files byte for byte. It is the model for §4.
- **Rust, crater.** Compiles every crate with a base and a head toolchain and
  reports what newly fails. verify's existing `--base` sweep comparison is the
  same idea at repository scale.
- **Zig, behavior tests.** Each test runs on several backends (LLVM, self-hosted
  x86_64, C, wasm) and skips the ones it is known to fail on, in the test itself.
  This is the model for §2.
- **GHC, testsuite ways.** Each test runs in several modes (`normal`, `optasm`,
  `ghci`, `threaded`…) and output must agree. This is the model for §3.
- **rustc, `tests/debuginfo`.** `gdb-command`/`gdb-check` and
  `lldb-command`/`lldb-check` directives in the test source. This is the model for
  §5.
- **LLVM, FileCheck.** The ordered-substring semantics used by `dbg-check`.
- **Csmith and YARPGen.** Random, well-defined C programs compared across
  compilers and optimization levels; C-Reduce shrinks the failures. These are the
  models for §6 and §7.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Whether `vm_opt` / `llvm_opt` stay in verify's default ways or move behind a
  flag, once their cost is measured.
- Whether `--cmp` should also compare the full dumps of files that are rejected on
  both sides, for example diagnostic text. Today only `ok` files are compared.
- How generated programs should treat `Int` overflow, once its semantics are
  pinned down on both backends.

## Future possibilities
[future-possibilities]: #future-possibilities

- Generated effect handlers (state, exceptions, multi-shot) once the generator can
  build effect rows.
- Generated `-g` programs with generated `// dbg:` scripts, feeding debug parity.
- A nightly or weekly fuzz run on the CI runners, with seeds recorded in the
  report.
- `--cmp` over LLVM IR (`--dump-lir-llvm`) as a fourth sweep.
