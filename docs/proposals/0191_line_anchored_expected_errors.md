- Feature Name: `line_anchored_expected_errors`
- Start Date: 2026-10-28
- Proposal PR:
- Flux Issue:
- Status: Draft
- Depends on: [0138](implemented/0138_flux_parity_ways.md) (parity ways and `flux parity-check`)
- Relates to: [0190](0190_verification_toolkit.md) (the verification toolkit, which runs `parity-check` in `cargo xtask verify`)

# Proposal 0191: Expected errors tied to a source line

## Summary
[summary]: #summary

A fixture that should fail marks each expected error on the line where it is
reported, with a trailing comment: `add("x", 2) //~ E300`. `flux parity-check`
then checks the error code *and* the line on every way, instead of only the
set of codes the header lists today. `--bless` writes and refreshes these
annotations. Existing fixtures move over one folder at a time, and a fixture
that has moved can drop its copy of the rendered error, because the line and
the code are what the parity check needs.

## Motivation
[motivation]: #motivation

A failing fixture today says what it expects in its header:

```flux
// expect: compile_error
// expect-error: E300
```

`parity-check` collects the `error[E…]` codes each way prints and compares
them with that list. A normal way must report exactly the listed codes; a
strict way may add more, as long as it reports every listed one
(`collect_diagnostic_code_details` in `src/parity/cli.rs`). Nothing ties a
code to a place in the program.

That gap is smaller than it sounds. Of the 444 fixtures that use
`expect-error:`, 443 live under `examples/`, and
`tests/examples/examples_fixtures_snapshots.rs` snapshots their full rendered
diagnostics, location included. 282 of them also carry a
`parity-expected-stderr` block: a second copy of the same rendered output,
kept in the fixture itself. So for nearly every fixture the line is already
checked, twice.

What the current form costs is elsewhere:

1. **The expectation is far from the code.** The header says "E300"; the
   reader has to run the program, or read a snapshot, to learn which line
   produces it. In a fixture with several errors, the header gives no hint of
   which line is which.
2. **Wording changes are expensive.** A change to an error's text, with no
   change to what is reported or where, re-blesses both the snapshot and the
   stderr block. The stderr block adds nothing the snapshot does not
   already pin, but it doubles the churn in every diagnostics change.
3. **A wrong line can pass.** For a fixture without a snapshot or a stderr
   block, such as new fixtures under `tests/parity/`, an error reported on
   the wrong line passes the code-only check.
4. **Names drift from what is reported.** [KI-137](../known_issues.md)
   records 20 fixtures whose file name names one error code while the CLI
   reports another. An annotation next to the offending line shows the real
   code where a reader looks, which makes that drift harder to miss.

The proposal is about readable fixtures and cheaper diagnostics changes. It
does not close a large hole in what is checked, and it should not be sold as
one.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Writing an expected error

Put `//~` and the error code at the end of the line the error is reported
on:

```flux
// expect: compile_error

fn add(a: Int, b: Int) -> Int {
    a + b
}

add("x", 2) //~ E300
```

Today the same fixture,
[`examples/type_system/failing/01_compile_type_mismatch.flx`](../../examples/type_system/failing/01_compile_type_mismatch.flx),
spends 31 lines on a header and a copy of the rendered error before its
first line of code. With the annotation, the header keeps only
`// expect: compile_error`.

`parity-check` reads the location the error is rendered with,
`examples/type_system/failing/01_compile_type_mismatch.flx:38:5`, and checks
that line 38 carries `//~ E300`. The column is not checked.

### When the line itself can't hold the comment

Some errors are reported on a line that cannot take a trailing comment, or
where one would change what is being tested: the end of the file, a line
inside a string, a parser error about a missing token. Two more forms point
at a neighbouring line:

- `//~^ E034` points at the line above. Each extra `^` goes one line further
  up: `//~^^ E034` is two lines above.
- `//~v E034` points at the line below, for an error reported at the end of
  the file, where there is no line to put the comment on.

These are rustc's forms; anyone who has written a rustc UI test reads them
the same way.

### More than one error

A line can carry several codes, and several lines can carry annotations:

```flux
let a: Int = "x"   //~ E300
let b = undefined  //~ E004
f(1, 2, 3)         //~ E056 E300
```

The fixture passes only when each way reports exactly these (code, line)
pairs: no missing one, no extra one. A strict way may report extra pairs, as
it may report extra codes today.

### Errors in another module

A fixture that imports a module can fail inside that module. The ki_076
fixture fails at run time in `Ki076Ord.flx`, a module it imports:

```text
error[E1000]: wrong number of arguments: want=3, got=2

  examples/generics/failing/runtime/Ki076Ord.flx:32:12
```

The annotation goes in the file the error names:

```flux
        if x <= y { y } else { x } //~ E1000
```

Runtime errors are matched the same way as compile errors: by the code and
the line in the rendered location.

### Blessing

`flux parity-check <fixture> --bless` already writes the expected stdout and
stderr blocks. With annotations it also writes or corrects the `//~`
comments from the first way's report, and leaves the rest of each line
untouched. Review the diff as you would a snapshot diff: a moved annotation
means an error moved.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Parsing

`src/parity/fixture.rs` already reads a fixture's header comments into
`FixtureMeta`. Annotations are read from every line of the file, not just
the header:

- `//~` followed by optional `^`s or one `v`, then one or more codes
  `E` + digits, separated by spaces.
- The annotation is the last comment on the line. Text after the codes is an
  error, so a typo cannot silently turn into a comment.
- A `//~` inside a string literal is not an annotation. The parser only needs
  to skip string literals, not lex Flux fully; fixtures that need anything
  more unusual can use `//~^` from the next line.
- A malformed annotation goes into `meta_errors`, as a malformed header does
  today, and fails the fixture.

Each annotation becomes an `ExpectedError { file, line, code }`. The file is
the fixture itself, or the module file the annotation is written in. Module
files are found the way `parity-check` already finds them, through the
fixture's module roots.

### Matching

For each way, the reported pairs come from the rendered stderr: every
`error[E…]` header and the `file:line:col` location under it. The location is
already printed, so no new compiler output is needed.

- A normal way must report exactly the annotated pairs.
- A strict way must report at least the annotated pairs.
- A mismatch is reported per way, listing missing and unexpected pairs, as
  `MismatchDetail::DiagnosticCodes` does for codes today.

### Coexistence with `expect-error:`

A fixture uses one or the other. A fixture with both is a metadata error,
because two lists of the same expectation would drift. `expect: compile_error`
and `expect: runtime_error` stay: they say how the program fails, which an
annotation does not.

### Snapshots and stderr blocks

The annotation is part of the source, so it appears in the source excerpts
of every rendered error, and therefore in snapshots and stderr blocks. rustc's
UI tests show their `//~` comments in their `.stderr` files the same way.

### Other readers of the metadata

`cargo xtask census` (`src/parity/census.rs`) classifies a run as
`ExpectedError` when the expected codes are present; it moves to pairs too.
`cargo xtask verify` runs `parity-check`, so it needs no change.

## Drawbacks
[drawbacks]: #drawbacks

- **Migration churn.** Adding a comment to a line changes the source excerpt
  of every error rendered from it. Moving the 443 `examples/` fixtures
  re-blesses up to 443 snapshots, once.
- **Longer lines.** A trailing annotation can push a line past the width
  `flux fmt` keeps. If fixtures are ever formatted, annotated lines need to
  be left alone.
- **Two forms for a while.** Until the migration ends, a reader meets both
  `expect-error:` and `//~`.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Keep checking codes only.** This costs nothing now and leaves the four
  problems in the motivation as they are.
- **Line numbers in the header,** such as `// expect-error: E300 @38`. This is
  less churn in the excerpts, but every inserted line breaks every
  expectation below it, and the reader still has to count lines.
- **Check the column too.** Columns move with harmless edits, such as renaming
  a variable. The line is enough to tell two errors apart in practice; the
  snapshot still pins the exact span where one exists.
- **Check message text, as Swift's `-verify` does.** The snapshots already
  pin the full text, so a second copy in the fixture would bring back the
  churn this proposal removes.

## Prior art
[prior-art]: #prior-art

- **rustc's compiletest.** `//~ ERROR`, `//~^`, `//~|` (same line as the
  previous annotation) and `//~v`, matched by line and by a substring of the
  message; `--bless` rewrites the `.stderr` goldens. See the UI tests chapter
  of the rustc dev guide and `src/tools/compiletest/` in rust-lang/rust.
- **Go.** `cmd/internal/testdir` (formerly `test/run.go`) reads
  `// ERROR "regexp"` on the offending line and matches the message by
  regular expression.
- **Swift.** `// expected-error {{message}}` with `-verify`, and `@+1`/`@-1`
  for neighbouring lines.
- **GHC.** The opposite choice: a test's full `.stderr` is a golden file, and
  nothing in the source marks where an error is expected. Flux's stderr
  blocks and snapshots work this way today.

Flux takes rustc's syntax, because it is the closest to Flux's comment
style and is familiar to contributors who write Rust, and Go's choice of not
checking message text, because Flux's snapshots already do that.

## Migration
[migration]: #migration

1. **Syntax and checking.** Parse annotations, match pairs, make `--bless`
   write them, and teach census the pairs. Tests in `src/parity/` for each
   form and each mismatch.
2. **New fixtures.** Fixtures added under `tests/parity/` and `examples/` use
   annotations. `tests/parity/CONVENTION.md` says so.
3. **Existing fixtures, one folder per commit.** `--bless` writes the
   annotations, the `expect-error:` line goes, and the snapshots are
   re-blessed. Where a snapshot pins the rendered text, the
   `parity-expected-stderr` block goes too.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Should stderr blocks go for every migrated fixture, or only where a
  snapshot covers the same output? This proposal suggests the second.
- An error rendered without a `file:line:col` location could not be
  annotated. All 817 errors in today's snapshots have one (even an import
  cycle points at the import), but if one turns up it needs a form not tied
  to a line, perhaps `//~? E…` anywhere in the file.
- Should `flux --test` fixtures in `tests/flux/` (one uses `expect-error:`
  today, `tests/flux/parse_error.flx`) take the same syntax?

## Future possibilities
[future-possibilities]: #future-possibilities

- Warnings and notes: `//~ W…` once warnings have stable codes.
- An editor action that inserts the annotation for a diagnostic under the
  cursor, through the language server.
