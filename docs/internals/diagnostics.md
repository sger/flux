# Diagnostics Output

Flux diagnostics aim to be readable, consistent, and actionable. This document describes
how diagnostics are rendered and what to expect when the compiler/linter reports issues.

## Overview

Diagnostics are rendered in a structured format:
- A file header (when grouped output is used)
- A diagnostic header (severity, title, error code)
- Optional message body
- Source location and snippet with caret
- Optional inline suggestions
- Optional hints and related notes/help

Output is produced by the diagnostics aggregator, which provides:
- Sorting by file, line, column, severity, and message
- Grouping by file header
- Summary counts
- Max error limiting with truncation message
- Deduplication of identical diagnostics

## Example

```
--> examples/diagnostics/hint_demos/fn_keyword_error.flx
-- Compiler error: UNKNOWN KEYWORD [E101]

Flux uses `fn` for function declarations.

  --> examples/diagnostics/hint_demos/fn_keyword_error.flx:3:1
  |
3 | fn add(x, y) {
  | ^^
help: Replace 'function' with 'fn'
   |
3 | fn add(x, y) {
  | ~~~
```

## File Grouping

When diagnostics span multiple files, the renderer prints a file header:

```
--> path/to/file.flx
```

Diagnostics are grouped under the matching header and sorted by:
1. File path (lexicographic)
2. Line number (ascending)
3. Column number (ascending)
4. Severity (errors, warnings, notes, help)
5. Message (lexicographic for stability)

## Stage Filtering and Failed Modules

Two rules decide which errors a multi-module compile reports.

**Per file: the stage cascade** (proposal 0061). Within one file, a parse error hides that
file's type and effect errors, and a type error hides its effect errors.
`DiagnosticsAggregator::process_with_metadata` (`src/diagnostics/aggregator.rs`) groups the
diagnostics by file and runs `apply_stage_filtering` on each group. A parse error in one file
never hides an error in another. `--all-errors` turns the filter off.

**Per module: failure scope** (proposal 0203). A module that fails to parse or validate is
skipped, and so is every module that imports it, each with a `Module Skipped` note naming the
dependency. Every other module is type-checked in the same run (`compile_modules`,
`src/driver/run_program/modules.rs`). Only a graph that couldn't be built stops before type
checking: an unresolved import, an import cycle, or a manifest error (`graph_failed` on
`GraphBuildResult`). The entry counts as failed for any error found before type checking,
including E083. Once a module has failed, stdlib modules that no remaining user module imports
are not compiled.

Nothing runs, dumps or reaches the native backend while any error exists, and the exit code is
non-zero. The REPL and `flux test` keep their own loops (see 0203's unresolved questions).

## Summary Counts

If there is more than one diagnostic, or both errors and warnings are present, a summary
line is printed before all diagnostics:

```
Found 3 errors and 2 warnings.
```

## Max Errors

The CLI supports limiting the number of errors shown:

```
flux --max-errors 1 examples/diagnostics/hint_demos/fn_keyword_error.flx
```

If errors are truncated, a footer appears:

```
... and 2 more errors not shown (use --max-errors to increase).
```

Warnings, notes, and help are still rendered even when errors are truncated.

## Deduplication

Identical diagnostics are deduplicated by:
- File path
- Span (start/end line + column)
- Severity
- Error code
- Title
- Message
- Related diagnostics (if any)

Diagnostics with different related entries are treated as distinct.

## Related Diagnostics

Diagnostics can include related entries that provide additional context:
- `note:` informational context
- `help:` guidance
- `related:` other relevant locations

Related diagnostics render after the primary diagnostic and may include their own span:

```
note: previous definition here
  --> src/main.flx:12:5
  |
12 | let x = 1;
  |     ^
```

## Plain Rendering

Some tests or internal paths use the lower-level `render_diagnostics` function, which
does not group by file or include summaries. Use the aggregator for full output.
