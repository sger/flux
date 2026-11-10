- Feature Name: `verification_toolkit`
- Start Date: 2026-10-16
- Proposal PR:
- Flux Issue:
- Status: In progress: S1–S7, S13 and S16 done, S15 next, on `feat/verification-toolkit`
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
  parity, as Csmith and YARPGen do for C compilers (S8). A reducer shrinks what it
  finds (S9).

What S1–S4 found reordered the rest (§17). The cheap checks found the bugs: S1
and S2 turned up 11 hidden parity failures, and S4's first run found
non-deterministic CFG lowering. The 6½ minutes of opt ways found nothing. So
cheap invariants come next:

- **Skips are checked** (S5). A skipped way that starts passing is reported, and
  every KI a skip names must still be open, as lit's `XFAIL` and pytest's strict
  `xfail` do.
- **Dumps get a second, normalized hash**, verify checks determinism on a sample,
  and Aether and LIR dumps are swept too (S6).
- **Semantics-preserving rewrites** of existing fixtures, such as reordering
  top-level definitions, must not change their output (S7), as EMI testing does.
- **Behaviour against the base compiler** (S10) and **a faster verify** (S11).
- `// dbg-check:` moves last (S12): debug parity already agrees 21/21.

**The goal is a support census, not only a merge gate** (S16, §16). verify
executes 427 of the 1,521 `.flx` files in the repository; the rest are only
compiled. `cargo xtask census` runs every file on `vm`, `llvm`, `vm_opt` and
`llvm_opt`, sorts each result into a class (works, wrong, expected error,
rejected, crashed, compile-only, not run), tags each file with the language
features it uses, and joins every failure to a KI. The result is a feature ×
way support matrix, as test262.fyi gives for JavaScript engines, and a list of
unfiled failures. S16 comes first after S4.

A survey of other compilers' checks added three more (§13–§15):

- **Optimization bisection** (S13), as LLVM's `-opt-bisect-limit`: an opt-way
  mismatch is narrowed to the one rewrite that causes it.
- **IR checks after every pass** (S14), as rustc's `-Zvalidate-mir` and GHC's
  `-dcore-lint`, with a LIR verifier, which Flux lacks.
- **Garbage-free checks** (S15): leaks and lost reuse fail a run, which no parity
  way can see today.

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
cargo xtask fuzz reduce <store>/fuzz_1/mismatch/case_37.flx
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
    §9 adds a sampled check that verify does run.
  - **Found on S4's first run:** 900 of the `cfg` sweep's dumps changed against
    a base built from the same source. Core lowering to CFG (`src/core/to_ir/`)
    gave every named function a `LoadName` per top-level function, in
    `HashMap` order, which differs from run to run. They are now emitted in
    binder order, with a unit test that lowers one program nine times.

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
  - `xtask/src/rng.rs`: splitmix64, shared with `rewrite` (§10). The same seed
    always gives the same programs, on every platform.
  - `generate.rs`: a type-directed generator (`gen` is a reserved word in Rust
    2024). It keeps an environment of typed variables and functions, and builds
    an expression of a requested type within a depth budget, preferring
    variables in scope so that values flow. Case `i` of seed `s` is seeded from
    `s` and a hash of `i`, so one case can be regenerated alone.
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
  - Literals stay below 100 and every `*` is reduced `% 9973`, so no value gets
    near the backends' different Int widths (KI-038, KI-039).
  - Runtime errors are allowed: parity compares `exit_kind`, and stderr when a run
    fails. Each program's header is `// expect: any`.
- **What open KIs leave out:** helpers are pure and only `main` prints, since a
  lambda can't call an effectful function (KI-029); only `Int` and `String` are
  ordered and only scalars compared (KI-157, KI-162); functions come before their
  callers (KI-117); a recursive call's arguments are variables (KI-155); no
  Floats, whose printed form differs between the backends. Each can return when
  its KI is fixed.
- **Layout:** one statement per line, so the reducer (§7) can work line by line.
- **Effects, type classes and async were out of scope for S8.** Generating handlers
  needs effect-row typing, and async needs deterministic scheduling across
  backends. S17 adds them, with generics, records and modules (below).
- **`cargo xtask fuzz [--seed S] [--count N] [--ways W] [--jobs J] [--gen-only]`:**
  - It writes `<store>/fuzz_<seed>/cases/case_<i>.flx` (the store is verify's
    output folder, `.verify/`), recreated on each run. `--gen-only` stops there.
  - It compiles each case with `flux --dump-core` first, on all cores but two,
    and moves the ones that fail out of `cases/`:
    - to `internal/` when the compiler failed on its own: a panic, a failed Core
      lint or Aether check, a timeout, or a non-zero exit with no diagnostic.
      Each is a compiler bug. E999 counts by its title, since FBIP's E999 is an
      ordinary diagnostic.
    - to `genfail/` when it reported a diagnostic. Each is a generator bug.
    Both are counted on their own line, the target is zero, and either fails
    the run.
  - It runs `flux parity-check` on `cases/` and copies mismatching cases to
    `mismatch/`. The copies sit outside `cases/` because parity-check sweeps a
    folder recursively. `parity()` moved from `verify.rs` to `xtask/src/parity.rs`,
    shared by verify, sanitize, census, rewrite and fuzz.
  - `parity.log` and `summary.txt` sit beside them.
  - Defaults are `--jobs 1` and `--count 50`. Native builds are expensive, and one
    heavy run at a time keeps the machine usable.
  - Not done in S8: running `bisect-opt` on each opt-only mismatch (§13) and
    `--verify-each` on every case (§14, which S14 adds).

**What S8 found.** The first 200 programs of seed 1 found two bugs no fixture
had. Eight failed with E999: an unused `let` released a parameter its branch
returned without counting it, and with the parameter borrowed the same plan
passed the verifier and printed freed memory (KI-172, fixed before going on).
Case 61 is a native miscompile: a call whose result nothing reads, before a join
whose value comes from elsewhere, was made a tail call, so the function returned
the callee's result (KI-173). A planted miscompile, the `--optimize` AST
fold of `+` off by one when the left operand is 2 mod 5, made 47 of the 200
mismatch, all on the opt ways only. That run took 9:51 with `--jobs 4`.

**The clean runs.** Without the plant, seed 1 gave 46 mismatches and seed 2 gave
58. Most were one new bug, KI-174: every multi-statement block the AST fold made
from an `if true` had the same expression id, so the second inference under
`--optimize` typed one block with another's type and rejected valid programs
(43 and 51 cases). It hid the rest on the opt ways: KI-116, the VM moving a
variable out in a `do` block that the function read again, which the fold now
reached from ordinary code (12 and 7 cases), and KI-173. All three were fixed
test-first, and seeds 1 and 2 then agreed on all four ways, 200 of 200 each, in
about 2 minutes a run: most of the 9:51 had gone to diagnosing mismatches.

**S17: whole programs.** S8's programs cover scalars, lists, options, pairs,
positional `data`, pure helpers and `main`. S17 extends the generator in four
parts, each designed around its open KIs, as S8 was, and each turned on with its
own flag so that a seed's programs don't change when a part is added, and a bug
in one part doesn't flood the others:

- **S17a, generics and type classes.** Type parameters on helpers and `data`; a
  class with one or two methods, instances for the generated data types,
  `deriving` where it applies, helpers with a constraint (`<a: Show>`) called at
  more than one type. Around: KI-162 (no tuple, Array, Either or `Ord` container
  meets a constraint), KI-104 (no instance method calls itself, since parity runs
  `--no-cache`), KI-076 and KI-086 (no class inside a `module` block).

  As built (`cargo xtask fuzz --with classes`, cases in
  `.verify/fuzz_<seed>_classes/`):
  - **Types:** the data types may derive `Eq`, `Show` or both, and one generic
    `data G<a>` holds `a` or scalars.
  - **Classes:** one or two user classes of one to three methods `(x: a) -> R`,
    or `(x: a, y: a) -> Bool`. Sometimes two methods share a type, so swapped
    dictionary slots would still type-check, and sometimes one has a default
    body.
  - **Instances:** for some of Int, Bool, String and the data types. Their
    methods call no class method (KI-104), and sometimes a contextual
    `instance C<a> => C<G<a>>` calls the class's methods on an `a` field.
  - **Helpers:** half are generic `fn f<a>` or constrained `fn f<a: C>`, fully
    annotated (KI-101). A constrained one always ends in a call of one of its
    class's methods on its `a` parameter, so its dictionary is used.
  - **Expressions:** they also call class methods at any type with an instance,
    `==` on data deriving `Eq`, and `show` on data deriving `Show`.
  - **Unchanged without the flag:** a test pins the hash of seed 1's first 50
    programs.

  Left out: multi-parameter classes (KI-075, KI-073), superclasses,
  instances of prelude classes for user types, and higher kinds.

  **What S17a found** before parity ran, compiling seed 1's first 200 programs:
  - **KI-176:** an uppercase type parameter (`data G<T> … deriving (Eq)`, or
    `instance C<T> => C<G<T>>`) gives E444 where `a` works, so the generator
    writes `a`.
  - **KI-177:** `m(C10)`, a method on a generic type's nullary constructor,
    stops the compiler with E000 rather than E476, so the generator calls
    methods on the generic type only through an annotated variable.
  - **KI-175:** 104 of the 200 still stop with E000. A class method in an
    `if` or `match` that is an annotated `let`'s value is "answered with more
    than one dictionary". Even the prelude's `show` does it, and the fuzz run
    moves those cases to `internal/`. Fixed. The annotated `let` checked its
    value and then inferred it, and both passes raised the value's
    predicates. A constrained function called there compiled, and natively
    printed the wrong value, because it was passed an extra dictionary. The
    check pass now drops what it raised.
  - **Parity** after that, on the 96 that compiled: all agree on all four
    ways. With KI-175 fixed, all 200 compile and agree.
  - **The reducer:** for an internal error it compared only the first error
    line, which is the same for every E000, so a reduction could drift from
    one internal error to another. `judge` now includes the explanation
    line, with its positions masked.
  - **KI-178:** on the VM, a class method named like a prelude function
    (`first`) panics with "No instance of …" when `main` calls it. Natively,
    or from any other function, it works. The generator names methods `m<n>`,
    so it can't produce this; a probe written for the exit plant did.
  - **KI-179:** without `--strict`, `[]` is accepted at any type
    (`fn f(...) -> Bool { [] }`). The VM catches it at run time with E1004,
    and natively nothing checks it. The reducer found it by drifting from
    the plant, which is how the exit-status rule below came about.

  **The exit test, met** on seed 1, 200 programs, all four ways:
  - **Where the plant goes.** The first plant, in `emit_dict_globals`
    (`src/compiler/passes/codegen.rs`), changed nothing. That's the AST
    compiler's; the VM's CFG backend builds a dictionary from its Core
    definition like any other. A plant on the definition doesn't work
    natively either, because the simplifier inlines `__dict_*` into its
    caller, which then builds its own tuple. So the plant goes on the read:
    native lowering of a `TupleField` on a `__dict_*` variable swaps slots 0
    and 1 (`src/lir/lower.rs`, the `AetherExpr` arm, since native lowers
    Aether, not Core).
  - **Found:** 22 of 200 mismatch, every one with a constrained helper.
    Natively, 9 printed a different value, 10 stopped with a runtime error
    and 3 crashed. The VM ran all of them.
  - **Reduced:** `case_148`, 96 lines to 13, in 1:08; `case_2`, 60 lines to
    11, in 0:33. Both still call a method through a constrained helper.
    This took two reducer changes, both in §7. First, a mismatch keeps each
    way's exit status; before that, `case_2` and `case_64` slid to KI-179.
    Second, three passes: a class method with its instances' methods, an
    unused parameter with its arguments, and a one-line body joined to its
    `fn` line. Before them, `case_148` stopped at 24 lines.
- **S17b, records and closures as values.** Named-field `data` (0152), field
  access and update; lambdas bound, returned from helpers, stored in lists and
  data and called later; Arrays, Maps with `Int` or `String` keys (KI-157),
  `Either`. Still no Floats (KI-161 and the printed form).

  As built (`cargo xtask fuzz --with values`, cases in
  `.verify/fuzz_<seed>_values/`; `--with classes,values` combines both parts):
  - **Records:** one or two single-constructor named-field types `Rec<n>` of
    two or three fields (scalars, `Option<Int>`, at most one function). Built
    with the fields in any order, read with `v.f`, updated with
    `({ ...v, f: e })`, and matched with `Rec { f: p, ... }`.
  - **Closures:** function types of one or two scalars. A function value is an
    annotated lambda that may read anything in scope, a record's field, or a
    non-generic helper's name. Helpers take and return them, sometimes a
    helper's whole body is a lambda over its parameters, and lists and record
    fields hold them. A function variable is called with exactly its arity
    (KI-107).
  - **Arrays and Maps:** through `import Flow.Array as Array` and
    `import Flow.Map as Map` (KI-146): literals, `push`, `map`, `filter`,
    `reverse`, `from_list`, `fold`, `length`, `get` and indexing (as
    `Option`s, never `array_get`: KI-147); Map keys `Int` or `String` only
    (KI-157), `set`, `delete`, `merge`, `size`, `has`, `get` and indexing.
  - **Either:** `Left`/`Right`, a two-arm match, and `either` with two lambdas.
  - **Printing:** a value holding a function is never printed whole (a
    closure's printed form is the backend's own); `main` prints what it gives:
    the call for a function, `map` over a list of them, the other fields of a
    record. Never a Map's `keys` or `values`, whose order is the hash's.

  **What S17b found** before any program was generated, probing each form the
  generator would write on one program:
  - **KI-180:** a named-field constructor whose name has no lowercase letter
    (`R5`, `RGB`) can be declared but not built; the parser reads it as a
    constant. The generator names records `Rec<n>`.
  - **KI-181:** a record built anywhere inside an `if` condition or a `match`
    scrutinee doesn't parse, even inside parentheses or a call's arguments. The
    generator builds none there; a record in a condition comes from a
    variable.

  Then 200 programs of seed 1 (and 50 with `classes,values`) compile with no
  rejection, and the first 10 agree on `vm` and `llvm`. A record prints
  positionally (`Rec5(1, Some(2))`), as named fields are erased.

  **The exit plant:** native lowering swaps the first two captures when a
  lambda's closure is built (`src/lir/lower.rs`, the `AetherExpr::Lam` arm's
  `MakeClosure`), outside `Flow` so the prelude's own closures stay right. A
  lambda the simplifier inlines has no closure, so the plant shows through a
  closure that outlives its scope: one a helper returns (`121` natively against
  `112`), or one stored in a list.

  **The exit test, met** on seed 1, 200 programs, all four ways:
  - **Clean:** 200 of 200 compile and agree, before and after the generator
    change below.
  - **A gap the plant showed.** The first plant run caught 2 of 200. Of 118
    helpers returning a closure, 1 had a lambda reading two of its parameters,
    and none was ever called: every function type was drawn fresh, so nothing
    asked for a helper's result type. Now a returned closure combines two of
    the helper's parameters in an order that shows (`a - b`, `a + b` on
    Strings, `a && !b`), function types recur, and `main` binds and calls half
    the closures helpers return: 50 of 200 programs call one, against 0.
  - **Found:** 39 of 200 mismatch, the VM running all of them; natively about
    34 printed another value, 2 crashed, 2 stopped with a runtime error, and 1
    timed out.
  - **Reduced:** `case_130`, 103 lines to 7: a lambda in `main` capturing two
    Strings, called through a variable (`""` against `"bc"`).
  - **Drift, twice, onto real bugs.** The reductions of the second run
    (30, 41 and 43 lines, each to 4) all became `f()(…)`: `reduce` inlined
    `let v = f(); print(v(…))`, and natively a chained call gives back the
    closure (KI-183). One of the first run's printed a list of closures, which
    natively shows as `<unknown obj=0xf5>` (KI-182). Each way exited as before,
    so the exit-status rule couldn't tell; both bugs are filed.
- **S17c, effects and handlers.** One or two user effects; helpers that declare
  and perform them; `handle` with clauses that resume once or don't resume, in
  helpers and in `main`. Around: KI-029 (no `perform` inside a lambda), KI-102
  and KI-114 (no effect named after a builtin), KI-032 (wrappers annotated).
  Multi-shot handlers come after.

  As built (`cargo xtask fuzz --with effects`, cases in
  `.verify/fuzz_<seed>_effects/`; it combines with `classes` and `values`):
  - **Effects:** one or two `effect Eff<n> { op<n> : (P) -> R }` over
    scalars, `P` mostly `R`. One operation of one parameter each (KI-185,
    KI-184), and never a builtin's name (KI-114, KI-102).
  - **Effectful helpers:** `fn f(..) -> R with Eff1[, Eff2]`, which performs
    each effect of its row first and then may `perform` where a type fits, or
    call another helper whose row is within its own. A lambda never does
    either (KI-029).
  - **Handlers:** a `let` anywhere (in `main`, in a pure helper, in an
    effectful one) may be `g(args) handle Eff1 { clause }` for an effectful
    `g`, nested in `do { } handle Eff2 { }` when `g` has two effects: a call,
    bound to a `let` (KI-025), so every effect is handled before `main`.
  - **Clauses:** they resume once, give the handle's type without resuming,
    or one or the other on a condition. A clause resumes with its parameter
    and a variable in scope combined in an order that shows (`p - k`), so a
    clause that read the two swapped gives another value.
  - Left out: multi-shot resume, handlers with state, `Async` (KI-169,
    KI-144), and a clause that prints.

  **What S17c found** before any program ran, probing each form on one
  program and then compiling 200:
  - **KI-184 (High):** natively, an operation of two parameters gives its
    handle a closure: `lower_perform_aether` lowers only the first argument.
  - **KI-185 (High):** natively, every operation of an effect runs its
    handler's first clause: the handler is built from `handlers[0]` alone.
    The one parity fixture with a two-operation effect never reads the second
    one's result.
  - **KI-186 (Medium):** a clause that reads a value the handled call takes
    over stops the compiler with E999 (Aether, "read after it was released");
    1 of the 200 programs.
  - **The reducer:** on that case, `vm` and `vm_opt` printed different
    diagnostics, so `reduce` took it for a mismatch and shrank it to a
    program neither way could parse. When no way compiles a case, the
    compiler's own failure now comes first (test-first).

  Then 199 of 200 compile (the other is KI-186), 10 agree on `vm` and `llvm`,
  and 50 with all three parts compile.

  **The exit plant:** native lowering binds a clause's parameter and its first
  capture to each other's values (`lower_handler_clause_aether`, outside
  `Flow`). A capture the simplifier folds to a constant isn't one, so the plant
  shows through a clause that captures a parameter or a computed value
  (`9` against `-5`).
- **S17d, async and modules.** Tasks whose results are awaited in a fixed order,
  and channels with one producer and one consumer, so the output doesn't depend
  on scheduling; a case can be a folder with a module file beside its `main`,
  which the reducer then has to shrink as a whole. Around: KI-108 and KI-169
  (native async; one `run_async` per program), KI-144, KI-103 (no constructor
  through a module alias), KI-110 and KI-146. KI-108 may have to be fixed first.

### §7 The reducer

- **`cargo xtask fuzz reduce <case.flx> [--ways W] [--jobs J] [--timeout S]`:**
  delta debugging (ddmin) over lines, after the passes below. A candidate
  deletes a set of lines; a deletion that splits a brace pair is widened to the
  lines the pair encloses. Code: `xtask/src/fuzz/reduce.rs`.
- **Keeping a candidate:** the pair of ways that disagreed on the original
  must still differ on the same stream (exit, stdout or stderr), each way must
  exit as it did on the original, and each way must compile it exactly when it
  compiled the original. Without the exits, a case where native fails and the
  VM runs slid to one where the VM fails and native runs (S17a's plant
  reduced to KI-179). The pair is the one
  with the fewest native ways among those that disagree, so a bug the VM shows
  is reduced on the VM alone. The two ways run directly, not through
  `parity-check`, which would run every way per candidate; a native pair
  checks `--dump-core` first. A case every way agrees on but the compiler
  fails on itself (one in fuzz's `internal/`) is reduced while it fails with
  the same first error line.
- **Syntax-aware passes first.** Line ddmin spends most of its runs on
  candidates that do not parse. Before it, the reducer tries edits that keep
  the program well formed, as Perses and llvm-reduce do:
  - delete a whole top-level item;
  - delete a class method together with its method in every instance of the
    class, a contextual one included;
  - delete a parameter a top-level function never reads, together with the
    argument in its place at every call (none when the function is passed as
    a value);
  - delete a statement, or a `match` arm when a later arm is a wildcard;
  - replace an `if` with one of its branches, or a `match` with one arm's
    value; a `match` with one arm whose pattern names the scrutinee or its
    tuple's parts gets the parts put in for the names;
  - replace a `let`'s value with a literal of its declared type (`0`, `false`,
    `""`, `[]`, `None`, tuples of these), the type taken from the annotation the
    generator writes;
  - inline a `let` used once;
  - replace an expression (a bracketed group, a call or a string) with a
    literal, one of its operands, a call's argument, or a tuple's or list's
    element, largest first. Its type is not known, so a wrong one is turned
    away by the compile check.

  Line ddmin runs after the deleting passes and before the expressions pass,
  which tries the most candidates per line and should see the smallest
  program. Last in each round, a function or instance method whose body is
  one value is written on one line (`fn m(x) { "" }`). It doesn't make the
  program smaller, but it makes the line count measure the program rather
  than the generator's layout, which gives every body three lines. Rounds
  repeat until one changes nothing. A pass that keeps a
  candidate goes on from there rather than starting over, which would retest
  every candidate before it.
- **Cost:** candidates are tested `--jobs` at a time (default 4) and each
  distinct text once. Native candidates build the standard library into a
  cache per job slot instead of on every run (about 0.8 s down to 0.3 s), and
  the result is checked again with `--no-cache`. `--timeout` defaults to 10 s:
  a candidate can recurse forever, and a timeout kills what flux started as
  well as flux, since the built program would otherwise hold the output pipes
  open.
- **Output:** `<case>.min.flx` beside the case, and the run's candidates in
  `.verify/reduce_<case>/`.
- **Promotion:** a minimized case goes to `tests/parity/` with `// expect: success`
  and a `parity-expected-stdout` block taken from the way that is right. It goes in
  the commit that fixes the bug, or with a `// skip-way:` and a KI if the bug stays
  open.

**What S9 measured.** On seed 1's mismatches: the planted fold's case_106, 69
lines to 3 (`print((67 + 10))`) in 170 runs and 0:25 on `vm` vs `vm_opt`;
KI-173's case_61, 61 lines to 11 in 282 runs and 0:59 on `vm` vs `llvm`; and
case_6, 42 lines to 14 in 129 runs and 0:08. case_6 was not the plant: on the
clean compiler `--optimize` rejects it with E300, typing a `let` of a data type
as `String`, where `vm` runs it. A planted run can hide real opt bugs among its
mismatches, so a fuzz run is judged on the clean compiler.

### §8 Checked skips

A `// skip-way:` hides a way until someone removes it, even after its bug is
fixed. Two checks keep skips honest:

- **Unexpected passes.** `parity-check --check-skips` runs the skipped ways as
  well, but outside the verdict. When a skipped way agrees with the others (and
  with the expected output, when there is one), it prints
  `UNEXPECTED_PASS <file> <way>: KI-102 may be fixed`, and the summary counts
  `UnexpectedPass: n`. verify passes the flag and records a note, not a failure:
  the fix may be on another branch. This is lit's `XPASS` and pytest's
  `xfail(strict=True)`, reported rather than failed.
- **Open KIs only.** Every KI a `// skip:` or `// skip-way:` reason names must
  be listed under `## Open` in `docs/known_issues.md`. One under `## Resolved`,
  one whose heading says FIXED, or one missing from the file is a metadata
  error, like a `skip-way:` reason without `KI-` (§2). A `skip:` that names no
  KI is not checked. After S5, 12 remain: 9 companion files for compiler
  diagnostics, one that prints the clock, and two TCP fixtures that time out
  intermittently. The 13th described a VM/native divergence and is now a
  `skip-way:` citing KI-114. `parity-check` reads the file given as
  `--known-issues <path>`, so a plain run without it behaves as before.

### §9 Better dump comparison

- **Normalized hashes.** Next to the exact hash, `hashes.tsv` gains a hash of
  the dump with every compiler-chosen number replaced by `_`
  (`DumpHash::normalized`, `sweep::normalize`). These are the digits after `#`,
  `%`, `%t` or `<sym:`, and after `v`, `b`, `bb` or `fn` when that prefix starts a word,
  which covers the cfg, core, aether and lir dumps. Literals and identifiers
  such as `sub2` keep their digits. `compare --content` then reports each
  changed file as one of:
  - `structure`: both hashes changed, so the program differs;
  - `numbering`: only the exact hash changed, so the same program is numbered or
    ordered differently.

  `--cmp` fails on either. `--cmp=structure` fails only on `structure`, for a
  change that renumbers on purpose. S4's 900 files would all have read
  `numbering`, which points at iteration order at once.
- **Determinism in verify.** After each sweep, verify reruns a sample of its
  files (`--det-sample`, 50 by default: every file whose name hashes into the
  sample, so the sample is the same on every run) with the same compiler and
  compares the exact hashes. A difference fails the step and names the file.
  It costs about two seconds per mode.
- **More stages swept.** An `aether` mode (`--dump-aether`), where ownership and
  reuse decisions show, and a `lir` mode (`--dump-lir`). Each costs about what
  `core` does; §13 records the measured cost. LLVM IR stays a future
  possibility, since it needs the native toolchain.
- **Shuffled internal order.** A hidden `--shuffle-seed <n>` flag (in
  `FLUX_SHUFFLE_SEED` too) reorders, by seed, what the compiler iterates in an
  order that should not matter: the top-level items handed to each pass, and
  the members of any `HashMap` or `HashSet` that a pass iterates, through one
  `ordered_iter` helper that sorts normally and shuffles under the flag. Go's
  `ssa/check/seed` does the same for its SSA passes. `sweep --determinism
  --shuffle` runs the second sweep under a seed, and verify's determinism
  sample uses a seed taken from the run's tag. A difference then shows on every
  run instead of by chance, as S4's did. Converting the iteration sites is the
  bulk of the work; it starts with `src/core/to_ir/`, `src/types/class_env.rs`
  and `src/core/passes/dict_elaborate.rs`.
- **Bytecode cache determinism.** `vm_cached` runs read the cache written by an
  earlier run. `sweep --determinism --mode cache` runs each program twice, each
  time with an empty `--cache-dir` and `FLUX_HOME` at the same path, and
  compares the files written, byte for byte.
  - **It has to run the programs:** any `--dump-*` flag turns the VM cache off,
    and `check` writes none.
  - **Its corpus is `tests/parity`**, whose programs are meant to run, quickly
    and without the network. It is not part of verify.

**What S6 found** (2026-10-17).

- **LIR closures captured their variables in `HashSet` order.** The sets came
  from `collect_free_vars` and the Aether `collect_free_vars_aether`. So two
  native builds of one program differed, although each was correct. The sets
  are now `BTreeSet`s. The `lir` sweep showed this as `numbering` on its first
  three files.
- **The bytecode cache differed between runs:** 126 cache files, from three
  programs. Four causes were fixed:
  - interface maps were serialized in hash order;
  - a loaded interface interned its new names in hash order;
  - effect lists in runtime contracts and effect rows were in hash order;
  - an interface's members were defined as globals in hash order.

  Six instance-heavy programs still differ, and are filed as KI-115.

**Measured cost** (verify s6, the whole corpus):

| step | time |
|---|---|
| `aether` sweep | 0:21 |
| `lir` sweep | 0:21 |
| `core` sweep, for comparison | 0:20 |
| each determinism sample | 1–3 s |

Together they add about 45 s to a run. The cache check, which verify does not
run, takes about 70 s over `tests/parity`.

### §10 Semantics-preserving rewrites

Every fixture with expected output, or that passes parity, is already a test
with a known answer. A rewrite that cannot change the meaning of a program must
not change that answer. This is metamorphic testing, as EMI (Equivalence Modulo
Inputs) does for C compilers.

- **`cargo xtask rewrite [--kind K|all] [--seed S] [--ways W] [<dir|file>…]`**
  writes rewritten copies of each fixture to
  `.verify/rewrite_<seed>/<kind>/<path>` and runs `parity-check` on each kind's
  folder. The defaults are every kind, seed 1, the ways
  `vm,vm_opt,llvm,llvm_opt`, and `tests/parity` and `examples/guide`. It is a
  text transform over the fixture's tokens and top-level items, std-only, in
  `xtask/src/rewrite/`, with its own small tokenizer (strings with `#{…}`,
  nested comments). Each file's choices come from the seed and its path, so a
  run can be repeated exactly.
- **Each copy is checked against the original's output.** A fixture with an
  expected-stdout block keeps it. For one without, the original is run once on
  the VM and what it prints is written into the copy's header, so the copy is
  checked against it and not only for agreement between its ways.
- **Kinds,** in order of value:
  1. `reorder`: shuffle the top-level `fn`, `data`, `type`, `effect`, `alias`,
     `class` and `instance` items, each within the run between two pinned
     items. Imports and top-level `let`s are pinned: a `let` runs where it is
     written, and whether a function may read a `let` written below it is a
     language rule, not something a rewrite should decide. Comments before an
     item move with it. This targets any lowering or inference that depends on
     definition order.
  2. `rename`: give each parameter, `let` binder and lambda parameter of a
     function, class or instance a fresh name (`rw<k>_<name>`), at every use in
     that item and inside `#{…}`, but not after `.`. Names that could be
     something else are left alone: anything the file declares, any field
     (a name before `:` in braces, or a punned one), and a name used in the
     item before it is bound. This catches name capture and lookups by name
     that should be by binder.
  3. `let-wrap`: replace a call's argument `a` with `do { let rw<k> = a; rw<k> }`,
     at most eight per file. Evaluation order and effects stay the same; the
     shape changes, which exercises the optimizer's inliner and the ownership
     pass. Patterns, handler-arm heads, definitions, lambdas (which take their
     parameter types from the call) and calls inside interpolations are left
     alone.
- **What is skipped,** and counted by reason in `summary.txt`: files without
  `fn main`, files that declare a `module` or import anything outside `Flow`,
  `skip:` and `stdout: varies` fixtures, `expect: compile_error` fixtures,
  files under a folder with a `parity.dir` (its defaults would not travel with
  the copy), and files the splitter cannot read.
- **A copy that no longer compiles** is counted on its own line, with a target
  of zero, and set aside (`*.flx.no-compile`) so parity does not count it again.
  It is either a rewriter bug or a compiler that depends on what was rewritten,
  and is triaged as one or the other. `failures.txt` lists every failure.
- verify does not run it. It is a command for refactors of lowering and for a
  weekly run, like `fuzz`.

**What the first run found** (seed 1, `--ways vm`, 0:25 for `reorder` alone, 1:29
for all three kinds). 190 of the 195 fixtures could be rewritten. `rename`
found nothing in its 162 copies. Two bugs and one rewriter bug:

- **[KI-116](../known_issues.md#ki-116)**, from `let-wrap`. On the VM, a
  variable read by `let t = x` inside a `do` block is uninitialised afterwards.
  47 of the 190 copies fail, some with wrong output and no error.
- **[KI-117](../known_issues.md#ki-117)**, from `reorder`. A function written
  after its caller is typed from the call, giving a spurious `E400` in one copy
  and the VM's closure check ([KI-111](../known_issues.md#ki-111)) in two.
- **A rewriter bug:** `let (x, y) = …` was taken for a call to `let` and
  wrapped. The fix came with its test.

**The full run** (seed 1, `vm,vm_opt,llvm,llvm_opt`, 198 fixtures, 1:34:32:
`reorder` 8:50, `rename` 9:16, `let-wrap` 1:16:14) found nothing new.
`rename`: 164 copies, 0 failures. `reorder`: 1 copy that no longer compiles and
2 `MISMATCH`, all KI-117. `let-wrap`: 51 `MISMATCH`, all with the VM ways
against the native ones, which agree: KI-116. Parity takes most of the time, so
a quick check is `--ways vm`, about 1:30 for all three kinds. `rewrite` runs
parity without `--check-skips`, since a rewritten copy that passes (reorder
can undo a KI fixture's order) says nothing about the KI.

**The exit check.** A planted order bug, predeclaration skipping every
function written after `main`, made 78 reordered copies stop compiling
(`E004`), against 1 without it (KI-117), in 0:32 with `--ways vm`.

### §11 Behaviour against the base compiler

`--cmp` (§4) fails on any dump change, but most refactors change dumps on
purpose. What they must not change is behaviour.

- **`verify --base <tag> --behaviour`** runs every parity fixture once more,
  with the base sweep's kept compiler on the `vm` way, and compares stdout and
  exit kind with the head's `vm` run. A difference fails the step and names the
  file, with the first differing line.
- It needs no expected output, so it covers every fixture, including the ones
  that only check that the backends agree. When both compilers are wrong in the
  same way it shows nothing, which parity already accepts.

### §12 Run time

verify takes about 22 minutes; main parity is 15 of them, and `parity-check`
runs one fixture at a time.

- **Parallel fixtures** (moved into S16, which needs them first).
  `parity-check --jobs N` runs N fixtures at once, each
  with its own ways in sequence, and prints verdicts in fixture order. verify
  passes `--jobs 2` by default and `--jobs` overrides it. Native builds are
  heavy, so the default stays low on purpose.
- **Opt ways when they matter.** `vm_opt` and `llvm_opt` run when the diff
  against the base touches the optimizer (`src/core/passes/`, `src/aether/`),
  under `--opt`, or when no base is given. Otherwise the summary notes that they
  were skipped. This answers the open question from §3.
- **Per-fixture counts, not times.** Wall time is noisy, which is why
  rustc-perf compares instruction counts and GHC keeps per-test metrics with
  acceptance windows. A hidden `--metrics <file>` flag makes `flux` write
  numbers that are the same on every run:
  - Core size after optimization, and the number of `dup`, `drop` and reuse
    sites in Aether;
  - bytecode size;
  - the VM's executed instruction count and allocation count, from
    `src/runtime/leak_detector.rs`'s counters (§15).

  `parity-check --metrics` collects them per fixture on the `vm` and `vm_opt`
  ways. verify keeps them in `<run>/metrics.tsv` and, with `--base`, fails on a
  fixture whose count grows beyond its metric's tolerance (0% for Core size and
  instruction count, which are exact) unless the run passes
  `--accept-metrics`, which records the new values as accepted. Wall time
  stays in the summary as a coarse signal.
- **A quick tier.** `verify --quick` runs fmt, clippy, the build, the three
  sweeps with determinism sampling, and main parity over `tests/parity` only.
  It is the check while working; the full verify is the check before a merge.
- **Choosing checks (2026-11-02).** `--only <checks>` runs just the checks it
  names and `--skip <checks>` all but those. The names are `fmt`, `clippy`,
  `suite`, `sweeps`, `parity`, `metrics`, `debug-build-parity`, `debug-parity`
  and `heap-parity`. `--skip-suite` and `--skip-sweeps` stay as
  `--skip suite` and `--skip sweeps`.
  - **Refused:** a check that needs another (`metrics` without `parity` or
    `--base`), and a flag that needs a check (`--cmp` without `sweeps`,
    `--accept-metrics` without `metrics`).
  - **A partial run says so:** each check it left out is a `skipped` line in
    `summary.txt`, and the run ends with "partial run: … a merge needs the
    full verify". Its total is not compared with a base's.
  - **Quick tier:** this replaces the quick-tier flag, since a preset over
    these names gives the same thing if one is ever wanted.

### §13 Optimization bisection

An opt-way mismatch (§3) says the optimizer broke a program, not which of its
rewrites did. LLVM's `-opt-bisect-limit` answers that by numbering rewrites.

- **A rewrite counter.** `--opt-bisect-limit <n>` (hidden; also
  `FLUX_OPT_BISECT`; `src/shared/opt_bisect.rs`) numbers every optimizing
  rewrite as it is about to fire, and skips those numbered above `n`, so `0`
  runs none of them. Each fire site asks `opt_bisect::allow(pass, what)` and,
  when told no, keeps the term as it was. What is counted:
  - Core simplification (`src/core/passes/`): beta, case-of-case,
    case-of-known-constructor, known-shape specialization, algebraic
    identities, constant folding, both let inliners, dead `let`;
  - the AST rewrites `--optimize` adds: desugaring, constant folding, and the
    type-informed fold's dead branches, constant propagation and inlining;
  - Aether: drop specialization (with the reuse on its unique path, which has
    no drop left to fall back on), dup/drop fusion, reuse, and selective field
    writes.

  Not counted, since a program is wrong without them or they are shared
  across modules: builtin promotion, canonicalization, ANF, dictionary and
  evidence passes, dup/drop insertion and borrow inference.
- **The same numbers on every run.** The counter is process-wide, and with a
  limit set the process builds rayon's pool with one thread, so modules
  compile, and rewrites are numbered, in one order. Native compiles a module
  more than once; each compile numbers its own rewrites, in order. Without a
  limit, `allow` is one read and always says yes, so ordinary builds are
  unchanged.
- **`--opt-bisect-limit -1`** runs everything and prints one line per rewrite to
  stderr: `opt-bisect: 49 constant_fold: fold IAdd in main (<module>)`. An AST
  rewrite names only its module, since it comes before Core.
  `FLUX_OPT_BISECT_SHOW=<def>` prints that definition's Core after the Core
  passes and its Aether after Aether.
- **`cargo xtask bisect-opt <fixture> [--way vm_opt|llvm_opt]`** runs the plain
  way as the reference, then the opt way with `-1` twice. Two different lists
  of rewrites stop it as non-deterministic. If the full opt run already
  behaves like the plain one, there is nothing to bisect. If the run with
  limit 0 still differs, the cause is outside the counted rewrites: the
  simplifier's extra rounds, LLVM's own `-O2` under `llvm_opt`, or an
  uncounted pass. Otherwise it binary-searches for the smallest limit whose
  run differs, about log2(r) runs, and reports that rewrite's line and a
  diff of its definition with the limit one below and at it. The report is
  in `.verify/bisect_<name>/report.txt`. "Behaves the same" is the same exit
  code, the same stdout, and the same stderr when the run failed.
- **Where it fits.** CONVENTION.md's debugging guide gains it as the first step
  for an opt-mode mismatch, and the fuzz driver (§6) runs it on each mismatch
  under `--optimize`.

**What S13 found.** Optimizing rewrites are rare in the fixtures' own code:
`stdlib_pipelines.flx` has 119 with `--optimize` on the VM, nearly all in
`Flow`. Across `tests/parity`, the 20 most frequent kinds add up to about 190
rewrites in the fixtures' own code; the inliners and the AST constant fold
lead, and Core constant folding fires there 7 times.
The exit check planted a wrong `IAdd` fold, active only under `--optimize`.
`parity-check tests/parity --ways vm,vm_opt` reported no mismatch, which led to
[KI-118](../known_issues.md#ki-118): on the VM, `main`'s own body does not go
through the Core simplifier, and the fixture that should have caught the plant
has all its code in `main`. With the same code in a function
(`tests/parity/core_inline_outside_main.flx`), `bisect-opt` named the planted
rewrite in 11 runs: `opt-bisect: 50 constant_fold: fold IAdd in f`, the diff
showing `IAdd(41, 1)` become `43`, on `vm_opt` in about a second. On `llvm_opt`
it named rewrite 150, the same fold in the last of native's compiles of `f`.

### §14 IR checks after every pass

Flux already checks its IRs at a few points:

- Core lint (`src/core/passes/lint.rs`, modelled on GHC's `-dcore-lint`) runs
  after every simplification round and after normalization, and is fatal
  (`E998`). It checks structure: scope, duplicate binders, parameter-type
  arity, empty `case` and `letrec`. It does not check types.
- `verify_contract` (`src/aether/verify.rs`) checks Aether's drops, reuse tokens
  and field masks.
- `validate_ir` (`src/cfg/validate.rs`) checks CFG IR.
- LIR has no verifier.

rustc's `-Zvalidate-mir`, Swift's `-sil-verify-all` and Scala's `-Ycheck:all`
run their checker after *every* pass, so a broken pass is named where it breaks
the IR rather than seen later as wrong output.

- **`lir::verify`** (`src/lir/verify.rs`): every block ends in one terminator;
  every value is defined before it is used on every path; branch targets
  exist; call arities match their callee; `dup` and `drop` apply only to
  heap-typed values.
- **`--verify-each`** (hidden): Core lint after each Core pass, not only each
  round; `verify_contract` after each Aether pass; `validate_ir` after each CFG
  pass; `lir::verify` after lowering and after each LIR pass. The error names
  the pass that ran last. Off by default, since it multiplies checking cost.
- **Where it runs:** a `vm_verify` parity way (the VM pipeline with
  `--verify-each --optimize`), which verify includes when the diff touches a
  pass; every fuzz run (§6); and the reducer's oracle (§7), so a lint failure
  can be reduced like a mismatch.
- **Types are out of scope here.** A typed Core lint, which re-checks every
  binder's annotation and every application against Core's types, as GHC's
  does, is a larger change that needs its own proposal. It is listed under
  Future possibilities.

### §15 Garbage-free checks

Perceus, which Aether follows, promises that a program frees each value as soon
as it is dead. Parity compares outputs, so a leak, an extra `dup` or a lost
reuse passes every way. Today:

- the native debug heap (`--debug-heap`, `FLUX_DEBUG_HEAP` in
  `runtime/c/rc.c`) catches a use after free or a double drop, with both sites;
- the VM's `src/runtime/leak_detector.rs` counts allocations by kind but not
  frees.

The checks:

- **Live blocks at exit.** The debug heap keeps a count of live blocks, and a
  list of their allocation sites. `flux_rt_shutdown` reports each block still
  live, up to ten with sites, and exits with a distinct code. The VM's leak
  detector counts frees as well, and `--leak-check` makes the VM report the
  same at exit. Values that live for the whole program, such as globals and
  interned constants, are counted separately and excluded.
- **A `llvm_heap` parity way:** native with `--debug-heap`, failing on a leak or
  a heap error. verify runs it over `tests/parity` and `examples/aether`.
- **Expected counts.** Fixtures in `examples/aether/` may state what Aether must
  achieve, checked against the `--metrics` counts (§12):
  `// expect-reuse: >= 1`, `// expect-allocs: 3`. A pass change that loses a
  reuse then fails a fixture, not only a benchmark.
- **Sanitizers.** An occasional run (`cargo xtask sanitize`) builds
  `runtime/c` with AddressSanitizer and UndefinedBehaviorSanitizer and runs the
  native ways over `tests/parity`, as Lean's CI runs a sanitizer job.

### §16 Support census

The checks above guard what is already tested. They do not say what Flux
supports: verify executes 427 of the 1,521 `.flx` files (the four main parity
directories, `examples/guide` and the debug fixtures). The other files are only
compiled, on one pipeline. `examples/type_system` (308 files), effects, async,
aether, `aoc`, `Modules` and `runtime_errors` are never run.

**Why `parity-check` cannot simply be pointed at everything today:**

- It drops every file without `fn main` (455 files) and never reports them
  (`should_skip_fixture`, `src/parity/cli.rs`).
- `classify_exit` (`src/parity/runner.rs`) reports a Rust panic as
  `RuntimeError` and an internal compiler error as `CompileError`, so a crash
  can compare equal to an ordinary error.
- `is_native_skip` turns a native failure into SKIP. Its strings ("llvm
  compilation failed", "unsupported CoreToLlvm") are no longer printed; native
  prints "native lowering failed", "native module compilation failed" and
  "llvm module pipeline failed".
- A file with no header is expected to succeed, so the 20 `runtime_errors`
  files, which fail on purpose, would count as failures.
- It runs one file and one way at a time, prints text only, inherits stdin
  (`tests/replay/fixtures/stdin.flx` blocks until its timeout), and keys its
  per-file cache by file stem, which collides once files run in parallel.
- No diagnostic is marked "unsupported". Gaps surface as panics, E998/E999,
  native-only runtime errors (E1200, E1201), the VM's "native-backend only"
  error, or the E040 and E308 "not supported" messages.

**Classes,** one per file and way:

| class | meaning |
|---|---|
| `works` | succeeds, agrees with the other ways, and matches its expected output |
| `wrong` | succeeds, but its output differs from another way's or from the expected block |
| `expected-error` | declared `compile_error` or `runtime_error`, and failed that way with the declared codes |
| `rejected` | a compile error where success was expected; a **backend gap** when another way of the same file works |
| `crashed` | a panic, an internal compiler error (E061–E070), E998, Aether verification, a signal, a native lowering or link failure, or a timeout |
| `compile-only` | no `main`: compiled, not run |
| `not-run` | `skip:`, an unmet `requires:`, outside the file's `// parity:` line, or `skip-way:` |

**`parity-check` changes:**

- `ExitKind::Crash`, set by `classify_exit` for the crash markers above. It
  never compares equal to an ordinary error, in plain parity runs too.
- `is_native_skip` keeps only the debug ways' "lldb is missing" skip. Its other
  strings were never printed any more; a native compile failure is now a
  `Crash` or a diagnostic, compared like any other result.
- Children get a null stdin, so a fixture that reads stdin sees end of input
  instead of waiting. The cache directory is keyed by the file's whole path.
- `--jobs N` runs N files at once, each file's ways in sequence, and prints
  results in file order.
- `--census <out.tsv>` writes one row per file and way: file, way, exit kind,
  class, error codes, stdout hash, milliseconds, verdict, KI. Under it, files
  without `main` run like any other (they compile and exit), with `--root` set
  above a dotted module name's folders; a file's `// parity:` line limits its
  ways, by backend, instead of being overridden by `--ways`; benchmarks are
  listed as not run.
- `requires: network` joins `unix` and `windows`; it is unmet unless
  `--allow network` is given.

**Feature tags.** `flux features <file>…` (hidden) parses each file and prints
its tags, from a walk of its syntax tree (`src/ast/features.rs`): effect annotations, user effects, effect rows, handlers, parameterized
handlers, sealing, classes, instances,
deriving, associated types, modules, imports, ADTs, match, nested patterns,
records, interpolation, lists, arrays, tuples, hashes, closures, recursion,
do blocks, mutation, annotations, generics, type aliases, `fip`, intrinsics,
`select` and async. A `// feature: a, -b` header adds or removes tags, as
test262's `features:` does. The file's directory is a second, coarse tag.

**`cargo xtask census [--tag T] [--jobs N] [--ways W] [--base T] [--allow …] [<dirs>…]`:**

1. Builds flux and runs `parity-check --census` over `examples`, `tests` and
   `lib` from the repository root, with `--jobs 4` and the ways
   `vm,llvm,vm_opt,llvm_opt`.
2. Tags every file with `flux features` (one process for the whole corpus,
   about 0.2 s).
3. Parses `docs/known_issues.md`: a heading with "FIXED" or "NOT
   REPRODUCIBLE" is resolved whatever its section; a fixture path cited in an
   entry links that entry to the file. Every failing row gets a KI, from its
   `skip-way:` or a citation, or is listed as **unfiled**.
4. Writes `.verify/census_<tag>/`: `census.tsv`, `matrix.tsv`
   (feature × way × class counts), `unfiled.txt`, `summary.txt` and a
   self-contained `census.html` with a pass rate per way, the matrix coloured
   by the share that works, the files behind each cell, the backend gaps and
   the unfiled list.
5. With `--base T`, lists what newly works and newly fails since census `T`,
   as crater does.

**Folder defaults.** About 150 of the files the first census reported are
snapshot-tested or carry `// dbg:` scripts, so a new header line would move
the line numbers those pin. A `parity.dir` file gives directives to a whole
folder, or to one named file in it, as LLVM lit's `lit.local.cfg` does; a
file's own header still wins. Three directives came with it: `expect: any`
for diagnostic demos (the ways must agree and none may crash; snapshots pin
the messages), `stdout: varies` for programs that print the clock, timings or
the host's core count, and `census-skip:` for fixtures another harness drives.

**Fixture hygiene.** The first census will report files whose headers are
missing or wrong. They are fixed in their headers, not filed: `expect:
runtime_error` and codes for `examples/runtime_errors`, the prose `// Expected
Error:` lines in `examples/diagnostics` turned into `expect-error` headers, and
`requires: network` or `stdin` on the fixtures that need them. Every remaining
failure is a bug and gets a KI.

**Result** (2026-10-17, five runs, c1 to c5). Supported went from 87–88% to
98.7–99.9% on every way. c1 had 741 failures, 739 of them unfiled; c5 has 39,
each joined to an open KI:

| way | c1 | c5 |
|---|---|---|
| vm | 88.0% | 99.9% |
| llvm | 87.5% | 99.3% |
| vm_opt | 87.5% | 99.3% |
| llvm_opt | 86.9% | 98.7% |

- **New bugs filed:** KI-105 to KI-113. The worst three are `--optimize`
  hiding type errors (KI-105), a native `--optimize` nested-match miscompile
  (KI-106), and native async and HTTP crashes (KI-108). KI-103 grew a second
  shape.
- **Fixtures repaired:** most of the rest were header gaps, plus 17
  `examples/type_system/failing` fixtures that had never found their imports.
- **Harness problems the census exposed:**
  - `expect: error` was read silently as success;
  - the examples snapshot harness ignored `// root:`, so 27 snapshots pinned
    a module-path error that only that harness produced;
  - a strict-only fixture had no way to say so (now `// flags: --strict`).
- **The remaining 18 gaps**, where one way handles a file and another fails:

  | how many | where it fails | KI |
  |---|---|---|
  | 9 | native only | KI-103, KI-107, KI-108, KI-109, KI-110 |
  | 7 | `--optimize` on both backends | KI-105 |
  | 1 | native `--optimize` | KI-106 |
  | 1 | VM only | KI-111 |

### §17 Staging

| stage | change | exit |
|---|---|---|
| S1 ✓ | verify counts `EXPECTED_OUTPUT_MISMATCH` and the `parity-check` exit code (§1) | A deliberately broken expected block in `examples/guide` makes `debug build parity` FAIL. Any failure this uncovers is fixed or filed as a KI. |
| S2 ✓ | `// skip-way:` (§2); whole-fixture skips narrowed; the main parity step records instead of noting | Unit tests in `src/parity/fixture.rs` for parsing, including the KI requirement. Main parity on `feat/debugger` has 0 MISMATCH, with every skip naming a KI. |
| S3 ✓ | `vm_opt` / `llvm_opt` and the opt check (§3); added to verify's main parity | `parity-check tests/parity --ways vm,vm_opt,llvm,llvm_opt` has 0 MISMATCH, after fixes or KI skips. The verify run-time increase is recorded in §3. |
| S4 ✓ | Dump hashes in sweeps, `compare --content`, `verify --cmp`, `sweep --determinism` (§4); the CFG lowering order fix | `cargo xtask sweep --determinism` reports 0 differences for `cfg`, `core` and `core-opt`. `verify <tag> --base main --cmp` on a no-op change passes; on a dump-printer change it fails and names the files. `compare.rs` unit tests. |
| S5 ✓ | Checked skips: `UNEXPECTED_PASS` and open KIs only (§8) | Unit tests for both. Moving a KI that a fixture names to `## Resolved` gives a metadata error; deleting a skipped bug's cause, not committed, gives `UNEXPECTED_PASS`. |
| S6 ✓ | Normalized hashes, sampled determinism in verify, `aether` and `lir` sweeps, cache determinism (§9) | Undoing the S4 lowering fix, not committed, fails verify's determinism sample and reads `numbering` in `compare --content`. The new sweeps' cost is recorded here. |
| S7 ✓ | `cargo xtask rewrite` with `reorder`, then `rename` and `let-wrap` (§10) | Every rewrite of `tests/parity` and `examples/guide` compiles and matches; each mismatch is fixed or filed as a KI. A planted order bug, not committed, is caught by `reorder`: predeclaration forgetting the functions written after `main` (78 copies no longer compile, against 1). (Undoing the S4 lowering fix, planned here first, changes only numbering, so no output; S6's determinism sample catches it. `binding_groups` in source order, planned next, changes nothing `reorder` touches: it orders statements inside blocks, not top-level items.) |
| S8 ✓ | The generator and `cargo xtask fuzz` (§6) | Unit tests: the same seed gives the same text; 200 seeds compile with 0 generator failures (an ignored test that runs `flux --dump-core`). A planted miscompile under `--optimize`, not committed, is found within `--count 200`. (200 of 200 compile with 0 generator failures and 0 internal errors; the run itself is the compile check, so there is no ignored test. The planted `+` fold made 47 of 200 mismatch. Found KI-172 and KI-173.) |
| S9 ✓ | `cargo xtask fuzz reduce` (§7) | The planted miscompile's case shrinks to at most 10 lines. Reducer unit tests on a fake oracle. (69 lines to 3 in 0:25. 12 unit tests: the passes on their own, and ddmin, the whole reducer and the tester on fake oracles. Found on the way: a timeout left the program flux built running.) |
| S10 | `verify --behaviour` (§11) | A planted change to a `print` builtin's output, not committed, fails it and names the files; a no-op change passes. |
| S11 | `parity-check --jobs`, opt ways on optimizer changes, per-fixture times, `verify --quick` (§12) | Main parity with `--jobs 2` gives the same verdicts as `--jobs 1`, faster; the times are recorded here. A planted slowdown in one fixture is listed. |
| S12 | `// dbg-check:` (§5), checks added to all 21 debug fixtures | Debug parity 21/21. Changing one expected local makes both the parity run and `parity_debug_tests` fail. |
| S13 ✓ | `--opt-bisect-limit` and `cargo xtask bisect-opt` (§13) | A planted wrong rewrite in one Core pass, not committed, makes a fixture mismatch under `vm_opt`; `bisect-opt` names that rewrite. With `-1`, the rewrite count is the same on every run. (A wrong `IAdd` fold, opt-only, named as `constant_fold: fold IAdd in f` in 11 runs; `-1` gives identical lines on two runs, VM and native. `verify s13` passed with 0 mismatches.) |
| S14 | `lir::verify`, `--verify-each`, the `vm_verify` way (§14) | Unit tests per LIR rule. A planted malformed rewrite, not committed, fails `--verify-each` naming its pass, where plain parity shows only wrong output. Main parity under `vm_verify` has 0 failures, after fixes or KIs. |
| S16 ✓ | The support census (§16): `ExitKind::Crash`, `--jobs`, `--census`, `requires: network`, `flux features`, `cargo xtask census` | Every file and way in the corpus is classified; every failure has a KI or a header fix, so the unfiled list is empty; the matrix is published. Plain parity over `tests/parity` gives the same verdicts as before, except crashes now reported as such. |
| S15 | Live blocks at exit, the `llvm_heap` way, expected counts, `cargo xtask sanitize` (§15) | `llvm_heap` over `tests/parity` and `examples/aether` reports 0 leaks, after fixes or KIs. A planted missing `drop`, not committed, fails it with the allocation site. A removed reuse fails an `expect-reuse` fixture. |
| S17 | Whole programs from the generator, in four parts (§6): S17a generics and type classes, S17b records and closures as values, S17c effects and handlers, S17d async and modules | For each part: 200 programs of one seed that use it compile with 0 generator failures; a planted miscompile in that part's lowering (a dictionary's methods swapped on one backend, a clause that resumes with the wrong value), not committed, is found within `--count 200` and reduces to at most 15 lines; every mismatch is fixed or filed. S17d's programs print the same output on 10 reruns of each way. |

S1 comes first, since without it the later stages could fail silently. S2 and S3
go together: the opt ways will find divergences, and S2 is how they are recorded
while they are open. S9 needs S8, and S17 needs both. S6's shuffling and S11's counts need nothing else. The expected
counts in S15 use S11's `--metrics`. The rest are independent of each other.

Stage numbers name the stages; they are not the order of work. The intended
order is S16, S5, S6, S7, S13, S15, S8, S9, S17, S14, S10, S11, S12: the census
first, since the goal is to know what works and what does not, then cheap
invariants, then the tools that turn a found mismatch into a named cause (S13) and
the check for the bugs parity cannot see (S15), then the generator. S17 follows
S9 directly: in its first 400 programs S8's subset found three new bugs and
reached a known one (KI-172 to KI-174, KI-116), and once they were fixed seeds 1
to 3 found nothing, so the next bugs are in the parts of the language it doesn't
generate.

**Why this order.** The first four stages showed where the bugs are. Counting
failures that were already printed (S1, S2) uncovered 11 of them, three of
them real bugs (KI-102 to KI-104). Comparing a run with itself (S4) found
non-deterministic lowering in its first run. The opt ways (S3), the most
expensive addition at about 6½ minutes a run, found nothing. So the cheap
invariants (S5–S7) go before the generator (S8, S9), and `dbg-check` (S12),
which guards debug output that already agrees 21/21, goes last.

## Drawbacks
[drawbacks]: #drawbacks

- **verify gets slower.** Two more ways in the main parity step, full dumps on
  disk, and expected checks. The content comparison itself is cheap. The opt ways
  are not: §3 records the cost, and the ways can move to an opt-in flag if it is
  too high.
- **Disk use.** Each run keeps a folder, `.verify/<tag>/`, at the repository
  root. `.gitignore` lists `.verify/`, so nothing in it is ever committed; `FLUX_VERIFY_OUT` moves it elsewhere. Sweeps keep dump hashes, not
  dumps, plus the compiler they ran (about 20 MB). `cargo xtask prune --keep <n>
  [--keep-tag <tag>]… --yes` deletes old runs. Until S4 the outputs piled up,
  never pruned, under `.git/parked/out/`, which reached 1.5 GB.
- **Skips can rot.** A `// skip-way:` that outlives its bug hides a regression in
  that way. Requiring a KI per skip keeps them listed in `known_issues.md`.
- **Every optimizing rewrite must report to the bisection counter (§13).** A
  pass that forgets is invisible to `bisect-opt`. A unit test runs each Core
  pass on a fixture it changes and checks that the counter moved.
- **Rewrites and checked skips are code to maintain.** The rewriter parses
  top-level items with text rules, not the compiler's parser, so new syntax
  can break it. Its failures are counted on their own line, as the
  generator's are.
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
- **test262 and test262.fyi.** Each JavaScript conformance test lists the
  features it uses; every engine runs the whole suite, and the results are
  shown per feature and per engine. The model for §16's matrix and its
  `// feature:` header.
- **Zig, GHC and the Oils spec tests.** Zig reports each backend's share of
  passing behaviour tests; GHC's `expect_broken(#ticket)` ties each known
  failure to a ticket and lists the unexpected ones; Oils publishes a table of
  shells × test cases. The models for §16's per-way rates, KI join and page.
- **LLVM lit and pytest, `XFAIL`/`XPASS`.** A test marked as failing that
  passes is reported (`XPASS`), and fails under pytest's `strict=True`. This is
  the model for §8.
- **EMI, Equivalence Modulo Inputs (Le, Afshari and Su, PLDI 2014).** Mutates
  programs in ways that cannot change their output on given inputs, and found
  hundreds of GCC and LLVM bugs. This is the model for §10.
- **LLVM, `-opt-bisect-limit`.** Numbers optimization passes and skips those
  past a limit, so a miscompile is bisected to one pass. The model for §13.
- **rustc `-Zvalidate-mir`, Swift `-sil-verify-all`, Scala `-Ycheck:all`, GHC
  `-dcore-lint`, Go `-d=ssa/check/on`.** IR checkers run after every pass. The
  models for §14. Go's `ssa/check/seed`, which randomizes value order, is the
  model for §9's shuffling.
- **rustc-perf and GHC's performance tests.** Deterministic counts compared with
  the base, with per-metric tolerances and an explicit accept step. The models
  for §12's metrics.
- **Lean 4's CI.** A sanitizer job over the runtime. The model for §15's
  `cargo xtask sanitize`.
- **Perses and llvm-reduce.** Reducers that only produce well-formed programs.
  The model for §7's syntax-aware passes.
- **Csmith and YARPGen.** Random, well-defined C programs compared across
  compilers and optimization levels; C-Reduce shrinks the failures. These are the
  models for §6 and §7.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Which paths count as the optimizer for §12, and whether a change outside
  them (for example in type inference) should still run the opt ways.
- Whether `UNEXPECTED_PASS` (§8) should fail verify on `main`, where the fix
  cannot be on another branch.
- Whether `rewrite` (§10) should handle fixtures with modules, by rewriting each
  module's file.
- Whether `--cmp` should also compare the full dumps of files that are rejected on
  both sides, for example diagnostic text. Today only `ok` files are compared.
- How generated programs should treat `Int` overflow, once its semantics are
  pinned down on both backends.

## Future possibilities
[future-possibilities]: #future-possibilities

- Multi-shot handlers in generated programs, after S17c's single-shot ones.
- Generated `-g` programs with generated `// dbg:` scripts, feeding debug parity.
- A nightly or weekly fuzz run on the CI runners, with seeds recorded in the
  report.
- `--cmp` over LLVM IR (`--dump-lir-llvm`) as a further sweep.
- A typed Core lint that re-checks every binder and application against Core's
  types, as GHC's Core Lint does (§14), in its own proposal.
- Translation validation of the emitted LLVM IR with Alive2, for the passes
  Flux asks LLVM to run.
- More rewrite kinds: inlining a single-use `let`, splitting a module in two,
  turning a `match` on `Bool` into `if`.
