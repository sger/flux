- Feature Name: `module_scoped_compile_failures`
- Start Date: 2026-11-12
- Proposal PR:
- Flux Issue:
- Status: Draft
- Relates to: [0061](implemented/0061_stage_aware_diagnostic_pipeline.md) (the Parse > Type > Effect cascade, per file; kept as it is), [0081](implemented/0081_diagnostic_taxonomy.md) (categorises the `Module Skipped` note this proposal reuses), [0139](implemented/0139_incremental_module_caching.md) (the `module: skipped` log for an upstream error), [0059](implemented/0059_parser_error_experience.md) and [0060](implemented/0060_parser_diagnostics_hm_typechecker_hardening.md) (recovery inside one file; out of scope here), [0190](0190_verification_toolkit.md) (S16's open question on fixture 207)

# Proposal 0203: Module-scoped compile failures

## Summary
[summary]: #summary

A parse error in one module stops type checking of that module and of every module that imports
it, and nothing else. Every other module is type-checked and reports its own errors. Only a
broken module graph (an import that doesn't resolve, an import cycle, a bad manifest) stops the
whole compile. GHC, Go and Rust all work this way. Flux already has the pieces: per-file stage
filtering and a module loop that skips failed modules. But `run_file` exits on any frontend error
in any file before that loop runs.

No language change. Nothing runs when any error exists, and exit codes stay as they are.

## Motivation
[motivation]: #motivation

### One parse error hides every type error

`examples/type_system/failing/207_stage_parse_suppresses_type.flx` has an unterminated string in
its entry and imports `Stage150Type`, which has an E300. Today the run prints only the E071. The
import is never type-checked, so its error is never found. Fixing the string, then running again,
is the only way to learn about the second error.

The same happens when the parse error is in an import. Take an entry that imports `ParseBad`
(unterminated string) and an unrelated `TypeBad` (`let x: Bool = 42`). It reports `ParseBad`'s
E071 and nothing about `TypeBad`. The two modules share nothing, but one is held back by the
other.

### The fixture's comment says something else

207's comment says "Entry is skipped, but independent imports still compile, so real suppression
is exercised." Neither the driver nor the snapshot harness does this. Its name, "parse suppresses
type", describes a filter that never sees the import's error. The c3 triage of 0190 S16 left this
as an open question, "per-file parse suppression (fixture comment) or global (0061)".

### Flux already filters per file

`DiagnosticsAggregator::process_with_metadata` (`src/diagnostics/aggregator.rs:497-530`) groups
diagnostics by file and runs `apply_stage_filtering` once per group. A parse error in `a.flx`
doesn't hide an E300 in `b.flx`, and `aggregator_stage_filtering_is_scoped_per_file`
(`tests/diagnostics/diagnostics_aggregator_tests.rs:510`) pins that. The global stop isn't in the
filter.

### The driver stops globally, and the module loop never runs

`run_file` (`src/driver/pipeline/program.rs:414`) and `run_from_source` (485) do this before
compiling:

```rust
if has_error_diagnostics(&ctx.all_diagnostics) {
    emit_compile_diagnostics_or_exit(&ctx, request);
}
```

`compile_modules` (`src/driver/run_program/modules.rs:56-81`) can do the module-scoped part:

- it puts the entry in `failed` when `entry_has_errors` and skips it;
- it starts from the graph's `failed_modules`;
- it skips any module with a failed import, adds it to `failed`, and pushes `module_skipped_note`.

With a clean frontend this already works. An import with a type error is reported, and the entry
that imports it gets "I skipped module `f3.flx` because its dependency `UsesParseBad` already has
errors." After a parse error, the exit above means the loop is never reached.

### How it got this way

- `057d5d78` (2026-02-14, "unify cross-module diagnostic reporting") added `entry_has_errors`,
  `failed_modules` and the `MODULE SKIPPED` note, for the behaviour this proposal asks for.
- `475948fe` (2026-04-26) added the exit. Its changelog entry
  (`changes/2026-04-26-import-cycle-diagnostics.md`) says "Stopped module-resolution failures
  from cascading into later type and backend diagnostics."
- The target was a broken graph. The check it used, any error-severity diagnostic, also caught
  parse errors.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

A Flux program's unit of failure is the module. When `flux run`, `flux check` or `flux build`
finds errors:

- A module with a parse error is skipped. Its parse errors are reported, and its type errors are
  not looked for. This is 0061's rule, per file, unchanged.
- A module that imports a skipped module is skipped too, with a note naming the dependency.
- Every other module is type-checked and reports its own errors.
- If the import graph itself is broken, only the graph errors are reported. That covers an
  import that resolves to no file, an import cycle, and a manifest error.
- Nothing runs while any error exists.

The scenarios from the motivation:

| Program | Reported today | Reported with this proposal |
|---|---|---|
| entry: parse error; import `TypeBad`: type error (207) | entry's E071 | entry's E071, `TypeBad`'s E300 |
| entry OK; imports `ParseBad` (parse error) and `TypeBad` (type error) | `ParseBad`'s E071 | `ParseBad`'s E071, `TypeBad`'s E300, entry skipped |
| `UsesParseBad` imports `ParseBad`; `UsesParseBad` also has a type error | `ParseBad`'s E071 | `ParseBad`'s E071; `UsesParseBad` and the entry skipped |
| entry imports `NoSuchModule` and `TypeBad` | E018 | E018 (unchanged) |
| one file with a parse error and a type error | the parse error | the parse error (unchanged) |

For compiler contributors: the driver decides whether the *graph* is usable, not whether
*anything* failed. A failed module is ordinary input to the module loop, which already knows how
to skip it.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### §1 Graph completeness

`ModuleGraph::build_with_prelude` (`src/syntax/module_graph/mod.rs:158-257`) already separates
the two kinds of failure, though it doesn't say so in its result:

- An import that fails to parse (`parse_program_with_interner` returns `None`), or fails
  `validate_file_kind` (for example E024), goes into `failed_modules` and is left out of
  `nodes`. This is a **module failure**.
- An import that doesn't resolve (`resolve_imports` returns `Err`, for example E018) still gets a
  node, with no edges for that import. An import cycle gives an empty `order`, so nothing would
  compile. These are **graph failures**.
- A manifest error (`manifest_diagnostic`, `src/driver/run_program/frontend.rs`) is a graph
  failure.

`GraphBuildResult` gets a field that records whether a graph failure happened, set where those
diagnostics are pushed. The driver's rule is then:

1. Graph failure: emit the diagnostics and exit, as `475948fe` intended. No type checking.
2. Otherwise go on to `compile_modules_for_run`, whatever module failures there were.

### §2 The entry

`entry_has_errors` (`frontend.rs`) is set from the parser's errors only. Today an entry that
imports `Flow.Primops` (E083, `validate_no_primops_import`, tagged `Parse`) relies on the global
exit to stop. Under this proposal E083 has to set `entry_has_errors` too, or the entry would be
compiled with that error present. The rule: **any error-severity diagnostic attributed to the
entry file before type checking makes the entry a failed module**.

### §3 The module loop

No change to `compile_modules`. It already:

- puts a failed entry into `failed` and skips it;
- starts `failed` from `failed_modules`;
- skips modules whose imports are in `failed`, adds them, and pushes `module_skipped_note`
  (`src/diagnostics/quality.rs:366`);
- type-checks the rest in topological order, stdlib first.

### §4 After the loop

`run_file` already calls `emit_compile_diagnostics_or_exit` after `compile_modules_for_run`, and
`has_error_diagnostics` before native dispatch. Every frontend error is still in
`ctx.all_diagnostics`, so both stop as before. Execution, `--dump-*` output and native dispatch
never see a failed compile.

The parallel VM fast path (`try_run_parallel_vm_fast_path`) runs the program, so it is taken only
when there are no frontend errors.

### §5 What stays the same

- Per-file stage filtering (0061, `apply_stage_filtering`) and `--all-errors`.
- Exit codes: any error exits non-zero.
- The REPL paths (`program.rs:525`, `534`, `638`, `643`): a REPL entry is one unit with nothing
  to keep going to.
- `flux test` already compiles past an import with a parse error (`src/driver/run_tests.rs:381-497`).
  Its entry parse error still exits at once (line 84); see Unresolved questions.

### §6 The snapshot harness

`tests/support/examples_snapshot.rs` copies the driver's behaviour, and has its own stops:

- `failed (parse)` for any entry parse error (355);
- `failed (module)` for any graph diagnostic (418);
- a `break` at the first failing module (410).

It takes the same rule as the driver, so a snapshot shows what `flux check` prints.

### §7 Fixtures

- 207 is renamed to say what it shows, `207_stage_parse_skips_only_its_module.flx`, and its
  snapshot shows E071 and `Stage150Type`'s E300.
- 210's comment says the `--all-errors` boundary is the module, not the run.
- `Stage150Type.flx`'s comment, "Helper module for stage-aware fixture 150/153", names fixtures
  that don't use it.
- New fixtures cover the scenarios in the guide-level table: an import with a parse error next
  to an independent import with a type error, a dependent of a module with a parse error, and an
  unresolved import next to a type error.

## Drawbacks
[drawbacks]: #drawbacks

- Type checking after a parse error does work that the user may throw away. In practice it costs
  one module loop over modules that would be checked on the next run anyway.
- A module skipped because of its dependency reports nothing of its own. The note says why, but
  its own errors wait for the next run. GHC, Go and Cargo all accept this.
- More output on a broken build. Each extra error is from a module the user will have to fix.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Keep the global stop.** One error at a time, the simplest output. It contradicts 207's
  comment, leaves `compile_modules`' skip logic unused, and differs from every compiler compared
  below. Fixing the comments would be the whole change.
- **Type-check everything that parsed, including modules whose imports failed.** It would report
  errors that come from the missing import (unknown names, wrong types) rather than from the
  module. Skipping dependents is what makes the extra output trustworthy.
- **Recover inside a module, as rustc does for a `mod` file.** It would need dummy declarations
  and error types that silence follow-on errors. It is a much larger change to the parser and
  the type checker, and outside this proposal; see Future possibilities.
- **Stop globally on a header parse error, as GHC and Go do.** Flux parses whole files and builds
  the graph from the parsed program. An entry whose import lines didn't parse leaves imports
  out, and an entry with errors is skipped anyway. A missing module then means fewer modules
  are checked, never a wrong error. So a separate header rule isn't needed.

## Prior art
[prior-art]: #prior-art

Compared in detail, with source citations and experiments, in the artifact
[Compile Failure Scope](https://claude.ai/artifact/Y9d1t78dzCEzmbGEzGXYd5). In short:

- **GHC** (unit: module). The downsweep parses only module headers. A header error empties the
  graph (`depanalE`, `compiler/GHC/Driver/Make.hs:195-200`), and nothing is typechecked. The
  upsweep builds imports first and runs a module only if its dependencies succeeded
  (`runPipelines`, `compiler/GHC/Driver/MakeAction.hs:144-158`). Note [Upsweep] says that
  "any modules which didn't depend on the failure can still be compiled". `-fkeep-going` has
  done nothing since `5f0d2dab9b` (2021); the driver always keeps going. Inside a module there is
  no parser recovery, and renamer errors stop typechecking.
- **Go** (unit: package). `noder.LoadPackage` reports the syntax errors of every file, then
  `checkFiles` exits before types2 (`cmd/compile/internal/noder/irgen.go:29-32`). `cmd/go`
  passes a failed action on to its dependents and keeps building independent packages
  (`cmd/go/internal/work/exec.go:144, 189-204`). An import-block syntax error fails at load,
  before any compile. There is a 10-error limit, lifted with `-e`.
- **Rust** (unit: crate). A crate-root parse error is fatal. A `mod` file that fails to parse
  becomes a dummy module, and other modules' type errors are still reported (UI test
  `tests/ui/resolve/parse-error-resolve.rs`). An unclosed delimiter is fatal for the crate.
  Cargo doesn't build crates that depend on a failed one; `--keep-going` builds every
  independent crate.

The rule this proposal adopts is the part all three share: a failure is scoped to its unit and
the units that depend on it, and only a broken graph stops everything. GHC is the closest match,
because its unit is the module and it doesn't recover inside one.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- `flux test` exits at once on an entry parse error (`run_tests.rs:84`), but keeps going past an
  import with a parse error. Should its entry follow §2, so a test file with a parse error still
  shows its imports' errors? Do it in the same change if it's small; otherwise in a follow-up.
- Should the summary line ("Found 1 error and 1 note across 2 modules") also say how many modules
  were skipped, or are the per-module notes enough?

## Future possibilities
[future-possibilities]: #future-possibilities

- Recovery inside a module (Rust-style dummy declarations and silenced follow-on errors), so a
  module with a parse error can still report type errors in its well-formed declarations. It
  builds on 0059 and 0060.
- An `import` written inside a `module { }` body is accepted without a diagnostic and does
  nothing; a later use of the module fails with E013. It was found while checking this proposal
  and is not filed yet.
- The LSP could use the same rule to check open files whose imports are broken.
