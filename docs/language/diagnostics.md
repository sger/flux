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
--> examples/hint_demos/fn_keyword_error.flx
-- Compiler error: UNKNOWN KEYWORD [E101]

Flux uses `fun` for function declarations.

  --> examples/hint_demos/fn_keyword_error.flx:3:1
  |
3 | fn add(x, y) {
  | ^^
help: Replace 'fn' with 'fun'
   |
3 | fun add(x, y) {
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

## Summary Counts

If there is more than one diagnostic, or both errors and warnings are present, a summary
line is printed before all diagnostics:

```
Found 3 errors and 2 warnings.
```

## Max Errors

The CLI supports limiting the number of errors shown:

```
flux --max-errors 1 examples/hint_demos/fn_keyword_error.flx
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
