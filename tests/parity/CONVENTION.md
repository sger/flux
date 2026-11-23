# Parity fixture convention

Each `.flx` file in this folder is a parity test that stands on its own. This
page says how to write one, and how to debug one that fails.
[`docs/internals/parity.md`](../../docs/internals/parity.md) is the reference
for `flux parity-check` itself.

## Naming

Name a file `<bug_class>_<short_description>.flx`. The bug classes:

- `toplevel_`: top-level declarations, constants, pure expressions
- `adt_`: building, matching and comparing ADTs
- `closure_`: lambda captures, higher-order functions
- `import_`: module imports, shadowing, qualified access
- `effect_`: algebraic effects, `handle` and `perform`
- `match_`: exhaustiveness, nested patterns
- `collection_`: array, list and map operations
- `arith_`: arithmetic, comparisons, numeric edge cases
- `string_`: string operations, interpolation
- `tuple_`: building tuples, reading their fields
- `async_`, `json_`, `stream_`: the matching `Flow` modules

A fixture written for a known issue ends with its number, so a search for the
KI finds it: `json_malformed_input_messages_ki138.flx`.

## Header lines

Each fixture starts with a block of comments:

```
// parity: vm, llvm
// expect: success
// bug: <one line on the bug this fixture isolates>
```

The lines a header can hold:

- `parity:`: the ways to compare. The default is `vm, llvm`. `--ways` on the
  command line replaces it.
- `expect:`: `success`, `compile_error`, `runtime_error` or `any`. The default
  is `success`. A failing expectation requires every way to fail in that
  phase. `any` is for diagnostic demos whose messages snapshots already pin.
  The program may compile or fail, but every way must do the same (the same
  exit kind, and the same stdout when it succeeds), and none may crash.
- `expect-error:`: one diagnostic code, such as `E300`. Write one line per
  reported error, duplicates included. Non-strict ways must report exactly
  these codes. Strict ways must report all of them and may add their own,
  such as `E425`. A new fixture marks its errors on their lines instead; see
  [Expected errors on their line](#expected-errors-on-their-line).
- `bug:`: the regression this fixture guards against.
- `root:`: a module root, passed to every way as `--root <path>`. The
  examples snapshot harness reads it too.
- `flags: --strict`: compile in strict mode on every way, for a fixture whose
  error only strict mode reports. `--strict` is the only flag allowed, because
  the snapshot harness honors no other.
- `requires:`: `unix` or `windows`. Anywhere else the fixture is reported as
  `SKIP`. `network` skips the fixture unless the run passes `--allow network`.
- `stdout: varies`: the program prints the time, timings or the host's core
  count, so its ways' exits are compared and their stdout is not.
- `census-skip:`: a reason to leave the file out of `cargo xtask census`
  only, for fixtures another harness drives (debugger scripts, replay
  recordings).
- `feature:`: `<tag>[, <tag>…]`, read by the support census
  (`flux features`). A tag adds to what the census infers from the program;
  `-tag` removes one it got wrong. The tags are listed in
  `src/ast/features.rs` (`TAGS`).
- `expect-reuse:` and `expect-allocs:`: what Aether must achieve in the
  fixture's own functions, as reuse sites or fresh constructor allocations.
  Write a count as `= n`, `>= n` or `<= n`; a bare number means `= n`. Once
  the ways pass, the fixture is compiled once more with the hidden
  `flux --metrics <file>`. A count that doesn't hold is an
  `EXPECTED_OUTPUT_MISMATCH` that names the line and the count seen. These are
  the numbers `--dump-aether` prints per function, summed over the functions
  the file defines, not the library's. So a pass change that loses a reuse
  fails a fixture, even though the program still prints the same.

  ```
  // expect-reuse: 3
  ```
- `expect-leak:`: `<way>[, <way>…] <= <n>: <reason>`. A heap way
  (`llvm_heap`) runs the program with `--leak-check`. A block still live at
  exit fails the fixture unless this line allows it: at most `n` blocks on
  that way. As with a skip, the reason must name the open known issue
  (`KI-nnn`). The bound keeps a second leak from hiding behind the first. A line in the file, or one for the file in `parity.dir`, that no
  longer leaks reports `UNEXPECTED_PASS` under `--check-skips`. An unscoped
  `parity.dir` line, which also covers fixtures that never leak, does not.

  ```
  json_derived_codecs.flx: expect-leak: llvm_heap <= 10: KI-209
  ```
- `skip:`: a reason to skip the whole fixture. It is still listed as `SKIP`.
  Use it only when no way can run the fixture, for example when it needs the
  network.
- `skip-way:`: `<way>[, <way>…]: <reason>`. It removes those ways from this
  fixture's run, even when `--ways` is given, and prints a `SKIP_WAY` line for
  each. The reason must name the open known issue that tracks the difference
  (`KI-nnn`); a skip-way without one fails the fixture. Delete the line when
  the KI is fixed. If fewer than two ways are left and the fixture declares no
  expected outcome, it is skipped.

  ```
  // skip-way: vm_strict: KI-090 strict rejects the boundary the VM fails at run time
  ```

Two flags keep skips honest, and `cargo xtask verify` passes both:

- `--known-issues docs/known_issues.md`: every KI that a `skip:`, `skip-way:`
  or `expect-leak:` reason names must be open there. One under `## Resolved`,
  or whose heading says FIXED, fails the fixture. A `skip:` that names no KI
  is not checked.
- `--check-skips`: the skipped ways run too, outside the verdict. One that
  would now pass prints `UNEXPECTED_PASS <file> <way>: KI-nnn may be fixed`,
  and the summary counts `UnexpectedPass`. `verify` records it as a note,
  since the fix may be on another branch.

### Expected errors on their line

A new failing fixture marks each expected error on the line it is reported
on, instead of listing codes with `expect-error:`. Then the line is checked
too, not only the code (proposal 0191):

```
print(add("x", 2)) //~ E300
//~^ E034         the line above; `^^` is two lines above
//~v E034         the line below; `vv` is two lines below
f(1, 2, 3)         //~ E056 E300
//~ Broken.flx:5 E400   an error in a module the fixture imports
```

- Non-strict ways must report exactly these (line, code) pairs. Strict ways
  must report them all and may add more. A mismatch lists each missing and
  each unexpected pair.
- An error in an imported module is named by file and line, from the
  fixture's folder, on a line of its own. The module itself carries no
  annotations, because other fixtures may import it too.
- A comment after a string left open becomes part of the string. The parity
  check rejects such an annotation; point at the line from a neighbor
  instead.
- `flux parity-check <files> --annotate` moves a passing fixture from
  `expect-error:` lines to annotations, or says why it can't.
- A fixture uses one form or the other, never both.
- `--bless` rewrites the annotations from what the ways report, when they all
  agree. It never adds or removes a line for an error in the fixture. A line
  that can't take the comment is annotated from the next free line with
  `//~^`, or from the nearest one above with `//~v`.
- A rendered error shows its source line without the trailing comment,
  unless the error's span runs into the comment. So an annotation stays out of
  snapshots and stderr blocks, except where the error covers a whole block (a
  `class`, an `instance`, a `match`). Then its first line is shown to the end,
  annotation included.

### Folder defaults: `parity.dir`

A `parity.dir` file gives the same lines, one per line without `//`, to every
fixture in its folder and the folders below. The nearest folder wins, and a
fixture's own header wins over any folder. A line `name.flx: <line>` (a path
relative to the folder) applies to that file only. Use it where a header line
would move line numbers that snapshots or `// dbg:` scripts depend on:

```
# examples/diagnostics/parity.dir
expect: any
```

```
# tests/parity_debug/parity.dir
exception_panic.flx: census-skip: stops the debugger on a panic; checked by debug parity
```

### Expected output

An expected output block may appear anywhere in the file. The first way's
normalized stdout must match the stdout block:

```
// parity-expected-stdout-begin
// "42"
// parity-expected-stdout-end
```

`parity-expected-stderr-begin` and `-end` do the same for the stderr of a
failing fixture. `flux parity-check <file> --bless` writes either block from
what the ways printed, when they agree.

## Rules

1. Each file isolates one bug.
2. Keep files small, under 30 lines if you can.
3. Output must be the same on every run: no timestamps, no random values.
4. Every parity bug that gets fixed should leave a fixture here.
5. A file uses `fn main() with IO { ... }` or top-level expressions.

## Running

```bash
cargo run -- parity-check tests/parity
cargo run -- parity-check tests/parity/adt_option_equality.flx
cargo run --features llvm -- parity-check tests/parity --ways vm,vm_opt,llvm,llvm_opt
```

[`docs/internals/parity.md`](../../docs/internals/parity.md#ways) lists every
way and what each is compared with.

## The support census

`cargo xtask census` runs every `.flx` file under `examples/`, `tests/` and
`lib/`, including those without `main`, on `vm`, `llvm`, `vm_opt` and
`llvm_opt`. It records one row per file and way in `.verify/census_<tag>/`
(proposal 0190 §16). A file's `// parity:` line limits its ways there by
backend: `// parity: vm` rules out the native ways, not `vm_opt`.

| Class | Meaning |
|---|---|
| `works` | succeeds with the expected stdout, or with what most ways print |
| `wrong` | succeeds with other output, or fails differently from what the header declares |
| `disagree` | the ways split evenly on stdout, and the file declares none |
| `expected-error` | fails as declared, with the declared codes |
| `rejected` | a compile error on a file expected to succeed |
| `crashed` | a panic, an internal compiler error, a failed IR check, a native lowering or link failure, a signal or a timeout |
| `compile-only` | no `main`: compiled, nothing to run |
| `not-run` | skipped, outside the `// parity:` ways, or removed by a `skip-way:` |

Every failure is matched to a known issue, from its skip reason or from an
entry in `docs/known_issues.md` that cites the file. Failures with none are
listed in `unfiled.txt`. Each is either a missing or wrong header, which you
fix in the file, or a bug, which gets a KI.

```bash
cargo xtask census --tag c1                    # everything, 4 files at a time
cargo xtask census --tag c2 --base c1          # and what changed since c1
cargo xtask census --tag fx examples/effects   # one area
flux features examples/effects/*.flx           # the feature tags alone
```

## How to debug a parity failure

Work from the top down. Each step narrows the problem.

### 1. Confirm the mismatch

```bash
cargo run -- parity-check path/to/failing.flx
```

Note how it is classified: `stdout differs`, `exit_kind` or `stderr differs`.

### 2. Check Core IR

```bash
cargo run -- parity-check path/to/failing.flx --capture-core
```

If it reports `core_mismatch`, the two compilers built different Core for the
same file. That should never happen: the bug is in the frontend (AST
lowering, type inference or a Core pass), in `src/core/` and `src/ast/`. If it
doesn't, Core is the same on both, and the bug is further down.

### 3. Check Aether

```bash
cargo run -- parity-check path/to/failing.flx --capture-aether
```

If it reports `aether_mismatch`, the dup, drop and reuse placement differs;
look in `src/aether/`. If it doesn't, ownership is the same on both, and the
bug is in a backend.

### 4. Look at the backend

Core and Aether are the same, so the bug is in one backend:

- the VM path: `src/cfg/`, `src/bytecode/`, `src/vm/`
- the native path: `src/lir/`, `src/llvm/`, `runtime/c/`

Useful flags:

```bash
# VM instruction trace
cargo run -- path/to/failing.flx --trace

# LIR, the native backend's IR
cargo run --features llvm -- path/to/failing.flx --native --dump-lir

# LLVM IR text
cargo run --features llvm -- path/to/failing.flx --native --emit-llvm
```

### 5. For cache mismatches

```bash
cargo run -- parity-check path/to/failing.flx --ways vm,vm_cached
```

Compare the fresh run with the cached one, and look at the `.fxc` and `.flxi`
files the cache holds.

### 6. For strict-mode mismatches

```bash
cargo run -- parity-check path/to/failing.flx --ways vm,vm_strict
```

Strict mode may reject what normal mode accepts, since it reports more. It
may not print different output when both succeed.

### 7. For optimizer mismatches

First let the compiler name the rewrite. `bisect-opt` numbers every
optimizing rewrite (`--opt-bisect-limit`) and searches for the first one that
makes the opt way differ from its plain way. It then shows that rewrite's
function without it and with it:

```bash
cargo xtask bisect-opt path/to/failing.flx --way vm_opt   # or llvm_opt
```

If the difference remains with every counted rewrite skipped, the cause is
the simplifier's extra rounds, LLVM's own `-O2` (for `llvm_opt`), or a pass
that isn't counted. Then compare dumps:

```bash
cargo run -- parity-check path/to/failing.flx --ways vm,vm_opt
cargo run -- path/to/failing.flx --dump-core > plain.core
cargo run -- path/to/failing.flx --optimize --dump-core > opt.core
diff plain.core opt.core
```

An `opt_mode_mismatch` means `--optimize` changed what the program does: its
exit kind, its stdout, or its stderr when it fails. That is a miscompile. The
Core diff shows which optimization rewrote the program; the passes are in
`src/core/passes/`. When only `llvm_opt` differs and the Core matches,
compare `--native --dump-lir` with and without `--optimize`, since
`--optimize` also raises the LLVM optimization level.

### 8. For debug-heap failures

`llvm_heap` runs the native build with `--leak-check`. The debug heap keeps
every freed block poisoned, stops at the next dup or drop of one, and at the
end names the blocks nothing freed.

A `heap_fault` is a use after free: a value released while the program still
used it, which plain `llvm` may survive by luck. Rerun it to read both sites
and the Flux trace:

```bash
cargo run --features llvm -- path/to/failing.flx --native --debug-heap
```

The allocation site names the value, and the free site is where ownership
went wrong. `--dump-aether=debug` shows the dups and drops Aether placed
there.

A `heap_mode_mismatch` without a fault means the program behaves differently
on the debug heap, for example a timeout from the slower allocator.

A leak prints a `LEAK` line with the first block's allocation site. It fails
the fixture unless an `expect-leak:` line allows it, and the summary's
`Leaks:` line counts every file that leaked. Fix the leak, or file a known
issue and add an `expect-leak:` line that cites it, usually in
`tests/parity/parity.dir`.

### 9. For IR-check failures

`vm_verify` and `llvm_verify` run the optimized build with `--verify-each`
(proposal 0190 §14), which checks each IR after every pass instead of at a
few points. Core lint (`E998`) and Aether's contract (`E999`) run after each
Core and Aether pass. The CFG validator (`E997`) runs after each CFG pass, and
`lir::verify` (`E997`) after LIR lowering and each LIR pass. The error names
the pass that ran last, which is where to look:

```bash
cargo run --features llvm -- path/to/failing.flx --native --optimize --verify-each
```

A verify way is compared with its opt way, so a fixture that skips `vm_opt`
or `llvm_opt` skips the verify way too, with the same KI.

### 10. Leave a regression fixture

Once the bug is fixed, add a small `.flx` file to `tests/parity/`, named as
described above.
