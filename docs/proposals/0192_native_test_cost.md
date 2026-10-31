- Feature Name: `native_test_cost`
- Start Date: 2026-10-31
- Proposal PR:
- Flux Issue:
- Status: Draft
- Relates to: [0190](0190_verification_toolkit.md) (`--metrics`, deterministic counts)

# Proposal 0192: What native tests cost, and what to do about it

## Summary
[summary]: #summary

Native tests set the pace of `cargo test-all`. This proposal measures where a
native test's time goes and ranks the fixes by what they save. On macOS the
biggest cost is not compiling or linking: it is the system's check of every
new executable the first time it runs, about 0.95 s per binary, 86% of a
native parity fixture's time. Linking is 5% and LLVM 4%. So the plan is to make
fewer new binaries per run (keep unchanged test binaries between runs, put the
parity corpus on the shared stdlib cache, stop building the same program
twice), to fix which tests count as native, and to add scale tests that gate
on counts instead of time. Two ideas from the earlier research are not worth
doing now: a check-only tier for native tests (it covers 9 of 327 tests), and a
dynamic runtime library for test links (it shortens the 5%, not the 86%).

## Motivation
[motivation]: #motivation

### Where a native build's time goes

Measured on 2026-10-31, commit `21f7d95d`, macOS on Apple silicon, release
`flux`. Each of the 147 parity fixtures that run natively was built and run
twice with `--native --stats-json` (added the same day),
against one cache already holding the stdlib, as a test with a seeded cache
has. 144 wrote stats. The other 3 do not compile: two expect a compile error,
and one is skipped for KI-117.

| Phase | Median | 90th percentile | Sum over fixtures | Share |
|---|---:|---:|---:|---:|
| parse | 3.7 ms | 4.3 ms | 0.5 s | 0.3% |
| frontend | 10.1 ms | 13.4 ms | 1.6 s | 1.0% |
| llvm_modules | 43.1 ms | 57.5 ms | 7.2 s | 4.4% |
| archive | 32.1 ms | 53.0 ms | 5.0 s | 3.1% |
| link | 58.1 ms | 61.3 ms | 8.4 s | 5.1% |
| execute, first run | 973.6 ms | 1100 ms | 141.2 s | 86.1% |

On the second run the binary is reused (143 of 144), and `execute` falls to a
median of 3.1 ms. The difference, a median of 966 ms per fixture and 139 s in
total, is the first-launch check, not the program.

The check costs the same for any new executable. A 17 KB C program built with
`cc` takes 0.90–0.93 s on its first run and 0.00 s after. So the cost is per
new binary, whatever its size or how it was linked. Bringing the same binary
back without a new check depends on how:

| Same executable, run from a new path as | First run |
|---|---:|
| the original, second time | 0.00 s |
| a hard link | 0.01 s |
| a plain copy | 0.15 s |
| an APFS clone (`cp -c`) | 0.36 s |
| a freshly linked binary | 0.90–0.97 s |

On 2026-10-29 the same check measured 278 ms, and the suite took 256–270 s.
On 2026-10-31 it measured 0.95 s and the suite took 465 s. Nothing in the test
setup changed in between that would explain a 200 s difference; a slower check
across about 400 new binaries per run would. Why the check got slower is not
known (unresolved question 1).

Linux pays no such check, so on Linux CI the compile and link steps are the
larger share. The steps below help both, in different proportions.

### Where the suite's time goes

One full `cargo test-all` on the same day: 4,246 tests, 465 s wall time,
2,763 s of summed test time. Tests in the `native` group
(`.config/nextest.toml`, at most 4 at a time) account for 378 tests and
1,655 s, 60% of the summed time.

| Test binary | Tests | Summed time | Share |
|---|---:|---:|---:|
| debug | 136 | 771 s | 27.9% |
| flume | 75 | 608 s | 22.0% |
| native_llvm | 147 | 456 s | 16.5% |
| replay | 35 | 393 s | 14.2% |
| vm_runtime | 286 | 154 s | 5.6% |
| integration | 711 | 147 s | 5.3% |
| everything else | 2,856 | 234 s | 8.5% |

The parity corpus (`tests/native_llvm/parity_corpus_tests.rs`, 9 tests that
each run a slice of `tests/parity`) is 245 s of that. Its native leg builds
every fixture with `--no-cache` (`src/parity/runner.rs`), so each fixture
compiles all 19 stdlib modules natively before its own. A cold build of
`examples/guide/arithmetic.flx` against a warm one shows what that costs:
frontend 138 ms against 11 ms, LLVM 431 ms against 5 ms, the support object
24 ms against 0.

Flume's 608 s is mostly package tests on the VM (15–25 s each); only 7 of its
tests are native, at 54 s. It is out of scope here and worth its own look.

### What native tests need from the program

Every native test was classified by what it asserts (read from the code, not
run):

| | Runs it, checks output | Runs it, checks behaviour | Build only | IR only | Other | Total |
|---|---:|---:|---:|---:|---:|---:|
| native_llvm | 73 | 34 | 0 | 36 | 5 | 148 |
| debug (native group) | 5 | 68 | 6 | 5 | 17 | 101 |
| other binaries | 65 | 7 | 3 | 1 | 2 | 78 |
| **All** | **143** | **109** | **9** | **42** | **24** | **327** |

"Behaviour" is timing, concurrency, the network, panic traces, the debugger or
the debug heap's counts. "IR only" tests already stop before linking. Only 9
tests need a binary without running it, and several of those still run it
through a shared helper. A check-only tier has almost nothing to cover.

The same pass turned up three smaller problems:

- `debug_info_tests` (12 tests) sits in the native group, but 11 of its tests
  never use the native backend, so they wait for native slots for nothing.
- About 11 tests build natively but are outside the group's 4-at-a-time cap:
  the 9 flume `assert_backends_agree` tests,
  `non_zero_smart_constructor_and_division` and
  `stdlib_process_fixture_passes_on_both_backends`.
- Some programs are built natively by several tests: the `safe_arithmetic`
  fixture by 6, `runtime_list_index` by 4, the bitwise, math and
  `stackless_drop` fixtures by 2 each.

### Compile-time regressions nobody would see

Nothing gates how the compiler's own cost grows with its input. A probe with
`--metrics` (0190 S11), which counts the compiler's allocations, on one
function made of a chain of `let`s, `let x1 = x0 + 1` and so on:

| `let`s | Allocations above an empty program | Growth when doubled |
|---:|---:|---:|
| 125 | 0.44 M | |
| 250 | 1.38 M | ×3.12 |
| 500 | 4.42 M | ×3.19 |
| 1,000 | 15.74 M | ×3.56 |
| 2,000 | 59.39 M | ×3.77 |

A linear pass grows ×2 per doubling. This one approaches ×4, so something
allocates in proportion to the square of the function's length. At 4,000
`let`s the compiler overflows its own stack. Neither has an entry in
`docs/known_issues.md`. Wall time grew less (1.2 s to 2.2 s from 1,000 to
2,000), which is why a timing check would not have caught it; the count is
exact on every run.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

For someone writing a native test, three things change.

**A native test's binary is kept between runs.** The first run of a test
builds and links its program as today. A later run whose program, compiler and
runtime are unchanged finds the binary in a store under `target/` and runs it
through a hard link in the test's scratch directory. It pays no link and no
first-launch check. Nothing changes in how a test is written: the store sits
behind `FluxRun`, `run_native` and the parity runner.

**The parity corpus's native leg uses the shared stdlib cache.** Its fixtures
compile only their own module, as `flux_runner` tests already do. A few
fixtures keep `--no-cache` on purpose, so the uncached path stays tested.

**Scale tests.** A scale test names an input that grows with `n`, a count from
`--metrics`, and a limit on how fast the count may grow. It runs the compiler
at four sizes and fails if the count grows faster than the limit. The first
one is the `let` chain above, marked ignored with the known-issue number until
that is fixed:

```text
scale test: let_chain
  n        250      500     1000     2000
  allocs   1.38M    4.42M   15.74M   59.39M
  growth exponent 1.92, limit 1.20: FAIL (KI-nnn)
```

Counts, not times, so the test gives the same answer on a laptop and on CI.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The steps are ordered by measured saving per hour of work. Each lands on its
own.

### S1. Keep native test binaries between runs

- A store directory, `target/native-test-bins/`, keyed by the hash the native
  cache already gives a linked program (the program hash in
  `src/driver/run_program/backend/native.rs`) plus the compiler's build id and
  the runtime library's identity, so a rebuilt compiler or runtime never
  reuses an old binary.
- After a test's build links a binary, the driver hard-links it into the store
  under that key if the key is new. Before linking, it looks the key up and,
  on a hit, hard-links the stored binary to the output path instead.
- The store is opted into by the test harness (an environment variable set in
  `tests/support/scratch.rs`), not by users, so `flux` itself behaves as
  today.
- Where a hard link fails (another filesystem, Windows without the
  permission), the build links as today.
- Eviction: a size cap with oldest-first removal, or clearing the directory
  when the compiler's build id changes. To settle in implementation.

Expected saving on macOS: about 0.95 s plus about 90 ms of archive and link for
every native build whose program did not change since the last run, which in
an edit-and-test loop is nearly all of them. On Linux, the archive and link
only.

### S2. The parity corpus on the seeded cache

- `src/parity/runner.rs` builds a native way with `--no-cache --cache-dir
  <per fixture>`. Give the corpus the seeded cache `tests/support/scratch.rs`
  already prepares (`seeded_cache_args`), copied per fixture as `flux_runner`
  does.
- Keep `--no-cache` for a fixed list of fixtures (or a `// parity: ...
  llvm_nocache` way), since KI-051 hid on the cached path and the uncached one
  needs some coverage too.
- `flux parity-check` from the command line keeps today's behaviour; only the
  test harness passes the seeded cache.

Expected saving: about 0.6 s of stdlib compilation per native fixture, roughly
90 s of summed time across 147 fixtures, on both platforms.

### S3. Native test group membership

- Take `debug_info_tests` out of the native group, apart from
  `native_dump_matches_vm_dump` if it is kept native.
- Add the flume `assert_backends_agree` tests, `non_zero_smart_constructor_and_division`
  and `stdlib_process_fixture_passes_on_both_backends`, by name or by
  renaming them to match `test(/native|llvm/)`.

No time saved in total; fewer native slots wasted, and no hidden fifth and
sixth native build at once.

### S4. Build each native program once

Where several tests build the same program, build it once in a shared
`OnceLock` helper and let the tests read the result, or merge the tests:
`safe_arithmetic_tests` (6 builds), `runtime_list_index_tests` (4), the
bitwise, math and `stackless_drop` tests (2 each). S1 makes this less pressing
on later runs, but the first run still pays every build.

### S5. Scale tests on counts

- A small harness in `xtask` (`cargo xtask scale`), not a test binary, since it
  needs several compiler runs and prints a table.
- Each scale test is a generator `fn(n) -> String`, the metric to read from
  `--metrics` (`compiler_allocs` first), the sizes (default 250, 500, 1000,
  2000) and a limit on the growth exponent (default 1.2, the slope of log
  count against log `n` after subtracting the count for `n = 1`).
- First inputs: a `let` chain (fails today; file a KI and keep the test with
  the KI number until it is fixed), many small functions, a deeply nested
  `match`, a long list literal, and many modules importing one another.
- `cargo xtask verify` runs it once; a growth over the limit fails verify the
  way a parity failure does.

### Not doing now

- **A check-only tier for native tests.** It would cover 9 of 327 native tests,
  and some of those need the binary run anyway. Revisit if a later count finds
  a large group of native tests that only need the build.
- **A dynamic Flux runtime for test links.** It would shorten the link, which
  is 58 ms and 5% of a native build. It would not touch the first-launch
  check, which is the same for a 17 KB binary as for a 2 MB one. Revisit if
  link time grows past the first-launch cost on macOS, or becomes the largest
  native step on Linux CI.

## Drawbacks
[drawbacks]: #drawbacks

- S1 adds state that outlives a test run. A wrong key would run a stale
  binary and hide a bug. The key must include everything that changes the
  binary, and a stale-store bug would be hard to see.
- S2 moves most native parity runs onto the cached path, so the uncached path
  gets less coverage than today. The fixed `--no-cache` list is the answer,
  but it has to be kept up.
- The macOS numbers depend on a system check whose cost changed between two
  measurements two days apart. If it drops back, S1's saving drops with it.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Exempt the terminal from the check** (System Settings → Privacy & Security
  → Developer Tools). It removes the 86% on one machine at no code cost, and
  is the first thing to do locally. It does not help other contributors, CI
  runners that someday run macOS, or a terminal that cannot be exempted.
- **Batch many programs into one binary**, as the `flux test` harness already
  does for one file's tests. It cuts new binaries further, but changes what
  each test runs and how failures are reported. S1 gets most of the saving
  without that.
- **Copy or clone into the scratch directory instead of hard-linking.** Both
  measured at 0.15–0.36 s, a third of the full check. Hard links measured
  0.01 s.

## Prior art
[prior-art]: #prior-art

- Koka's test driver compiles the standard libraries once before running tests
  in parallel (`test/Spec.hs`); Go's `testdir` resolves the standard library
  once into one import config. S2 is the same idea for the parity corpus.
- The rustc dev guide warns that macOS may check each new test binary at
  launch, at a large cost to test time (compiletest docs).
- Swift's `utils/scale-test` compiles inputs of growing size and fails a
  counter that grows faster than a set polynomial degree, which catches
  quadratic passes without timing noise. S5 follows it on Flux's own counts.
- Content-addressed build caches (ccache, sccache, Bazel's action cache) key
  outputs by everything that went into them. S1's store is a small one.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Why the first-launch check went from 278 ms to 0.95 s between 2026-10-29
   and 2026-10-31, and whether that explains the suite going from about 260 s
   to 465 s. Re-measure before S1 and after any macOS update.
2. The same measurements on Linux, where there is no first-launch check, to
   size S1 and S2 for CI.
3. S1's eviction policy and the exact key (which build id for the compiler,
   which identity for `libflux_rt.a` and `libflux.a`).
4. Which pass allocates in proportion to the square of a function's length, and
   why 4,000 `let`s overflow the compiler's stack. Both get known-issue entries
   when S5 starts.

## Future possibilities
[future-possibilities]: #future-possibilities

- The same store could serve `flux test` binaries and replay tests.
- `--stats-json` on every native test build would give these numbers on each
  CI run instead of from a one-off script.
- Flume's 608 s of mostly VM package tests deserves its own measurement.
