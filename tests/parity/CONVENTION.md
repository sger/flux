# Parity Fixture Convention

Each `.flx` file in this directory is a self-contained parity regression test.

## Naming

Files follow the pattern: `<bug_class>_<short_description>.flx`

Bug classes:
- `toplevel_` — top-level declarations, constants, pure expressions
- `adt_` — ADT construction, matching, equality
- `closure_` — lambda captures, higher-order functions
- `import_` — module imports, shadowing, qualified access
- `effect_` — algebraic effects, handle/perform
- `match_` — pattern matching exhaustiveness, nested patterns
- `collection_` — array/list/map operations
- `arith_` — arithmetic, comparisons, numeric edge cases
- `string_` — string operations, interpolation
- `tuple_` — tuple construction, field access
- `async_`, `json_`, `stream_` — the matching `Flow` modules

A fixture written for a known issue ends with its number, so searching for
the KI finds it: `json_malformed_input_messages_ki138.flx`.

## Inline metadata

Each fixture starts with a header comment block:

```
// parity: vm, llvm
// expect: success
// bug: <one-line description of the bug shape this fixture isolates>
```

Fields:
- `parity:` — which ways to compare (default: `vm, llvm`). `--ways` on the
  command line overrides it.
- `expect:` — `success` | `compile_error` | `runtime_error` | `any` (default:
  `success`). A failing expectation requires every way to fail in that phase.
  `any` is for diagnostic demos whose messages snapshots already pin: the
  program may compile or fail, but every way must do the same (same exit kind,
  and the same stdout when it succeeds), and none may crash.
- `expect-error:` — one diagnostic code, such as `E300`; repeat the line once per
  reported error, duplicates included. Non-strict ways must report exactly these
  codes. Strict ways must report all of them and may add their own, such as `E425`.

### Expected errors on their line

A new failing fixture marks each expected error on the line it is reported
on, instead of listing codes with `expect-error:`. Then the line is checked
too, not just the code (proposal 0191):

```
print(add("x", 2)) //~ E300
//~^ E034         the line above; `^^` is two lines above
//~v E034         the line below
f(1, 2, 3)         //~ E056 E300
```

- Non-strict ways must report exactly these (line, code) pairs; strict ways
  must report them all and may add more. A mismatch lists each missing and
  unexpected pair.
- Only the fixture's own lines can be annotated. A fixture whose error is
  located in a module it imports keeps `expect-error:`, and so does one whose
  error is on a line that leaves a string open, where a trailing comment
  would become part of the program.
- `flux parity-check <files> --annotate` moves a passing fixture from
  `expect-error:` lines to annotations, or says why it can't.
- A fixture uses one form or the other, never both.
- `--bless` rewrites the annotations from what the ways report, when they all
  agree. It never adds or removes a line: a line that already has a comment is
  annotated from the next free line with `//~^`.
- Rendered errors show a source line without its trailing comment, unless
  the error's span runs into the comment. So an annotation stays out of
  snapshots and stderr blocks, except where the error covers a whole block
  (a `class`, an `instance`, a `match`): its first line is shown to the end,
  annotation included.
- `bug:` — what regression this fixture guards against
- `root:` — a module root passed to every way as `--root <path>`. The examples
  snapshot harness reads it too.
- `flags: --strict` — compile in strict mode on every way, for a fixture whose
  error only strict mode reports. `--strict` is the only flag allowed, because
  the snapshot harness honours no other.
- `requires:` — `unix` or `windows`; elsewhere the fixture is reported as `SKIP`.
  `network` skips the fixture unless the run passes `--allow network`.
- `stdout: varies` — the program prints the time, timings or the host's core
  count, so its ways' exits are compared and their stdout is not.
- `census-skip:` — a reason to leave the file out of `cargo xtask census` only,
  for fixtures another harness drives (debugger scripts, replay recordings).
- `feature:` — `<tag>[, <tag>…]`, read by the support census (`flux features`).
  A tag adds to what the census infers from the program; `-tag` removes one it
  got wrong. The tags are listed in `src/ast/features.rs` (`TAGS`).
- `expect-reuse:` / `expect-allocs:` — what Aether must achieve in the
  fixture's own functions: its reuse sites, or its fresh constructor
  allocations. A count, `= n`, `>= n` or `<= n`; a bare number means `= n`.
  Once the ways pass, the fixture is compiled once more with the hidden
  `flux --metrics <file>`, and a count that does not hold is an
  `EXPECTED_OUTPUT_MISMATCH` naming the line and the count seen. These are the
  numbers `--dump-aether` prints per function, summed over the functions the
  file defines, not the library's. A pass change that loses a reuse then
  fails a fixture, though the program still prints the same.

  ```
  // expect-reuse: 3
  ```
- `skip:` — a reason to skip the whole fixture. It is still listed as `SKIP`.
  Use it only when no way can run the fixture, for example when it needs the
  network.
- `skip-way:` — `<way>[, <way>…]: <reason>`. It removes those ways from this
  fixture's run, even when `--ways` is given, and prints a `SKIP_WAY` line for each.
  The reason must name the open known issue that tracks the divergence (`KI-nnn`);
  a skip-way without one fails the fixture. Delete the line when the KI is fixed.
  If fewer than two ways are left and the fixture declares no expected outcome,
  it is skipped.

  ```
  // skip-way: vm_strict: KI-090 strict rejects the boundary the VM fails at run time
  ```

  Two flags keep skips honest; `cargo xtask verify` passes both.
  - `--known-issues docs/known_issues.md`: every KI a `skip:` or `skip-way:`
    reason names must be open there. One under `## Resolved`, or whose heading
    says FIXED, fails the fixture. A `skip:` that names no KI is not checked.
  - `--check-skips`: the skipped ways run too, outside the verdict. One that
    would now pass prints `UNEXPECTED_PASS <file> <way>: KI-nnn may be fixed`,
    and the summary counts `UnexpectedPass`. verify records it as a note, since
    the fix may be on another branch.

A `parity.dir` file gives the same directives, one per line without `//`, to
every fixture in its folder and the folders below; the nearest folder wins, and
a fixture's own header wins over any folder. A line `name.flx: <directive>`
(a path relative to the folder) applies to that file only. Use it where adding
a header line would move line numbers that snapshots or `// dbg:` scripts pin:

```
# examples/diagnostics/parity.dir
expect: any
```

```
# tests/parity_debug/parity.dir
exception_panic.flx: census-skip: stops the debugger on a panic; checked by debug parity
```

Expected output blocks may appear anywhere in the file. The first way's
normalized stdout must match the stdout block:

```
// parity-expected-stdout-begin
// "42"
// parity-expected-stdout-end
```

`parity-expected-stderr-begin` / `-end` does the same for stderr on a failing
fixture.

## Rules

1. Each file isolates exactly one bug shape.
2. Files must be small (under 30 lines preferred).
3. Output must be deterministic — no timestamps, random values, etc.
4. Every newly fixed parity bug should leave behind a fixture here.
5. Files must work with `fn main() with IO { ... }` or top-level expressions.

## Running

```bash
# Basic parity (vm vs llvm)
cargo run -- parity-check tests/parity
cargo run -- parity-check tests/parity/adt_option_equality.flx

# All ways
cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,vm_strict,llvm_strict,vm_opt,llvm_opt

# The optimizer: each opt way must match its plain way
cargo run -- parity-check tests/parity --ways vm,vm_opt,llvm,llvm_opt

# The debug heap: llvm_heap must match llvm; leaks are counted (LEAK lines)
cargo run -- parity-check tests/parity --ways llvm,llvm_heap

# With Core/Aether checkpoint capture
cargo run -- parity-check tests/parity --capture-core --capture-aether

# CI/release parity commands
cargo run -- parity-check tests/parity --ways vm,llvm
cargo run -- parity-check examples/guide --ways vm,llvm
cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,vm_strict,llvm_strict
```

## The support census

`cargo xtask census` runs every `.flx` file under `examples/`, `tests/` and
`lib/`, including those without `main`, on `vm`, `llvm`, `vm_opt` and
`llvm_opt`, and records one row per file and way in
`.verify/census_<tag>/` (proposal 0190 §16). A file's `// parity:`
line limits its ways there, by backend: `// parity: vm` rules out the native
ways, not `vm_opt`.

| class | meaning |
|---|---|
| `works` | succeeds with the expected stdout, or with what most ways print |
| `wrong` | succeeds with other output, or fails differently from what the header declares |
| `disagree` | the ways split evenly on stdout and the file declares none |
| `expected-error` | fails as declared, with the declared codes |
| `rejected` | a compile error on a file expected to succeed |
| `crashed` | a panic, an internal compiler error, an IR check failure, a native lowering or link failure, a signal or a timeout |
| `compile-only` | no `main`: compiled, nothing to run |
| `not-run` | skipped, outside the `// parity:` ways, or a `skip-way:` |

Every failure is matched to a known issue, from its skip reason or from an
entry in `docs/known_issues.md` that cites the file. Those with none are
listed in `unfiled.txt`. Each is either a missing or wrong header, fixed in the
file, or a bug, which gets a KI.

```bash
cargo xtask census --tag c1                    # everything, 4 files at a time
cargo xtask census --tag c2 --base c1          # and what changed since c1
cargo xtask census --tag fx examples/effects   # one area
flux features examples/effects/*.flx           # the feature tags alone
```

## How to debug a parity failure

Follow this workflow top-down. Each step narrows the problem further.

### 1. Confirm the mismatch

```bash
cargo run -- parity-check path/to/failing.flx
```

Note the classification: `stdout differs`, `exit_kind`, `stderr differs`.

### 2. Check Core IR (semantic IR)

```bash
cargo run -- parity-check path/to/failing.flx --capture-core
```

- **`core_mismatch` reported?** The bug is in the frontend — AST lowering, type inference, or Core passes produce different IR per binary. This should not happen; investigate `src/core/` and `src/ast/`.
- **No `core_mismatch`?** Core IR is identical. The bug is downstream.

### 3. Check Aether (ownership model)

```bash
cargo run -- parity-check path/to/failing.flx --capture-aether
```

- **`aether_mismatch` reported?** The Perceus dup/drop/reuse insertion differs. Investigate `src/aether/`.
- **No `aether_mismatch`?** Ownership is identical. The bug is in backend lowering.

### 4. Inspect backend-specific behavior

At this point Core and Aether are identical — the bug is in the backend:

- **VM path:** `src/cfg/`, `src/bytecode/`, `src/runtime/vm/`
- **LLVM path:** `src/lir/`, `src/llvm/`, `src/runtime/c/`

Useful flags:
```bash
# VM instruction trace
cargo run -- path/to/failing.flx --trace

# Dump LIR (native backend intermediate)
cargo run --features native -- path/to/failing.flx --native --dump-lir

# Dump LLVM IR text
cargo run --features native -- path/to/failing.flx --native --emit-llvm
```

### 5. For cache mismatches

```bash
cargo run -- parity-check path/to/failing.flx --ways vm,vm_cached
```

Compare the fresh run and cached run. Check `.fxc` and `.flxi` file contents.

### 6. For strict-mode mismatches

```bash
cargo run -- parity-check path/to/failing.flx --ways vm,vm_strict
```

Allowed: strict mode rejecting what normal mode accepts (additional diagnostics).
Not allowed: strict mode producing different output when both succeed.

### 7. For optimizer mismatches

First let the compiler name the rewrite. `bisect-opt` numbers every
optimizing rewrite (`--opt-bisect-limit`), binary-searches for the first one
that makes the opt way differ from its plain way, and shows that rewrite's
definition without it and with it:

```bash
cargo xtask bisect-opt path/to/failing.flx --way vm_opt   # or llvm_opt
```

If it reports that the difference remains with every counted rewrite
skipped, the cause is the simplifier's extra rounds, LLVM's own `-O2`
(`llvm_opt`), or a pass that is not counted. Then compare dumps:

```bash
cargo run -- parity-check path/to/failing.flx --ways vm,vm_opt
cargo run -- path/to/failing.flx --dump-core > plain.core
cargo run -- path/to/failing.flx --optimize --dump-core > opt.core
diff plain.core opt.core
```

An `opt_mode_mismatch` means `--optimize` changed what the program does: its
exit kind, its stdout, or its stderr when it fails. That is a miscompile. The
Core diff shows which optimization rewrote the program; the passes are in
`src/core/passes/`. When only `llvm_opt` differs and the Core matches, compare
`--native --dump-lir` with and without `--optimize`, since `--optimize` also
raises the LLVM optimization level.

### 8. For debug-heap failures

`llvm_heap` runs the native build with `--leak-check`: the debug heap keeps
every freed block poisoned, stops at the next dup or drop of one, and at the
end names the blocks nothing freed.

A `heap_fault` is a use after free: a value released while the program still
used it, which plain `llvm` may survive by luck. Rerun it to read both sites
and the Flux trace:

```bash
cargo run --features llvm -- path/to/failing.flx --native --debug-heap
```

The allocation site names the value; the free site is where ownership went
wrong. `--dump-aether=debug` shows the Dups and Drops Aether placed there.

A `heap_mode_mismatch` without a fault means the program behaves differently
on the debug heap, for example a timeout from the slower allocator.

`LEAK` lines do not fail a fixture. They count files with blocks still live
at exit, and the summary's `Leaks:` line totals them. Nearly every native
program leaks today ([KI-119](../../docs/known_issues.md#ki-119),
[KI-121](../../docs/known_issues.md#ki-121)); a fix for those should bring
the count down.

### 9. Leave a regression fixture

Once fixed, add a minimal `.flx` file to `tests/parity/` following the naming convention above.
