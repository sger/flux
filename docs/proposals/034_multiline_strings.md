# Proposal 034: Multi-line String Literals

**Status:** Draft
**Date:** 2026-02-20
**Scope:** Lexer, parser (minimal), no runtime changes

---

## Motivation

Flux strings cannot span multiple lines. The lexer terminates any string at the first
newline character ([strings.rs:149](../../src/syntax/lexer/strings.rs)):

```rust
b'\n' | b'\r' => {
    // Strings cannot span lines.
    return (span_start, i, false, false);
}
```

This makes it awkward to embed structured text data directly in Flux source — for
example, AoC puzzle inputs for tests, CSV/TSV data for fixtures, SQL queries, or any
multi-line template. Current workarounds are verbose:

```flux
// Option 1: escape sequences — unreadable for long content
let data = "7 6 4 2 1\n1 2 7 8 9\n9 7 6 2 1\n1 3 2 4 5"

// Option 2: concatenation — tedious
let data = "7 6 4 2 1\n" ++
           "1 2 7 8 9\n" ++
           "9 7 6 2 1\n" ++
           "1 3 2 4 5"
```

This proposal adds **triple-quoted multi-line string literals** using `"""..."""` syntax.

---

## Proposed Syntax

```flux
let data = """
7 6 4 2 1
1 2 7 8 9
9 7 6 2 1
1 3 2 4 5
8 6 4 4 1
1 3 6 7 9
"""
```

Triple-quoted strings:
- Open with `"""`
- Close with `"""`
- Allow literal newlines in content
- Support the same escape sequences as regular strings (`\n`, `\t`, `\r`, `\\`, `\"`, `\#`)
- Support interpolation with `#{...}` (same as regular strings)

---

## Indentation Stripping

When multi-line strings are embedded in indented code, the leading whitespace on each
line is part of the content unless stripped. This proposal defines **automatic
indentation stripping** based on the column of the closing `"""`:

```flux
fn make_fixture() {
    """
    7 6 4 2 1
    1 2 7 8 9
    9 7 6 2 1
    """
}
```

The closing `"""` is at column 4 (4 spaces of indentation). The lexer strips exactly
4 spaces from the start of every non-empty content line. The result is:

```
7 6 4 2 1\n1 2 7 8 9\n9 7 6 2 1\n
```

Rules:
1. The first newline immediately after the opening `"""` is ignored.
2. The indentation of the closing `"""` defines the strip amount.
3. Lines with less indentation than the strip amount are an error.
4. Empty lines (whitespace only) are kept as empty lines regardless.

This mirrors Swift's multi-line string behaviour and is the most ergonomic approach —
the closing delimiter controls indentation, which is visible and predictable.

**No stripping** example (closing `"""` at column 0):

```flux
let data = """
7 6 4 2 1
1 2 7 8 9
"""
```

Result: `\n7 6 4 2 1\n1 2 7 8 9\n` — first newline (after `"""`) is kept since there's
no leading `"""` on the opening line to suppress it.

Actually, for simplicity: always skip the newline immediately following `"""` if it's
the very first character in the content. This is standard behaviour (Python, Kotlin,
Swift all do this).

---

## Interpolation in Multi-line Strings

Works identically to regular strings:

```flux
let rows = 3
let summary = """
Input has #{rows} rows.
Each row is space-separated integers.
"""
```

The token stream for an interpolated multi-line string is the same as for a regular
interpolated string: `InterpolationStart` → expression → `StringEnd`.

---

## Examples

### Test fixture data

```flux
fn test_parse_reports() {
    let input = """
    7 6 4 2 1
    1 2 7 8 9
    9 7 6 2 1
    """
    let lines = split(trim(input), "\n") |> filter(\l -> trim(l) != "")
    assert_eq(len(lines), 3)
}
```

### Parsing the multi-line input

```flux
fn parse_reports(raw) {
    split(trim(raw), "\n")
    |> filter(\line -> trim(line) != "")
    |> map(\line ->
        split(trim(line), " ")
        |> filter(\t -> trim(t) != "")
        |> map(\t -> parse_int(t))
    )
}

let input = """
7 6 4 2 1
1 2 7 8 9
9 7 6 2 1
1 3 2 4 5
8 6 4 4 1
1 3 6 7 9
"""

let reports = parse_reports(input)
// [[7,6,4,2,1],[1,2,7,8,9],[9,7,6,2,1],[1,3,2,4,5],[8,6,4,4,1],[1,3,6,7,9]]
print(len(reports))  // 6
```

### With interpolation

```flux
let n = 42
let msg = """
Result: #{n}
Done.
"""
```

---

## Implementation

### Phase 1 — Lexer: detect `"""`

In `src/syntax/lexer/mod.rs`, the one-byte dispatch currently handles `"` by calling
`read_string_start()`. Change it to peek ahead:

```rust
b'"' => {
    if self.peek_byte() == Some(b'"') && self.peek2_byte() == Some(b'"') {
        self.read_multiline_string_start()
    } else {
        self.read_string_start()
    }
}
```

### Phase 2 — Lexer: `read_multiline_string_start`

New method in `src/syntax/lexer/strings.rs`:

```rust
pub(super) fn read_multiline_string_start(&mut self) -> Token {
    let cursor = self.cursor_position();
    let line = cursor.line;
    let column = cursor.column;

    self.read_char(); // skip first  "
    self.read_char(); // skip second "
    self.read_char(); // skip third  "

    // Skip the immediately following newline (if any)
    if self.current_byte() == Some(b'\n') {
        self.read_char();
    } else if self.current_byte() == Some(b'\r') {
        self.read_char();
        if self.current_byte() == Some(b'\n') {
            self.read_char();
        }
    }

    let (content_start, content_end, ended, has_interpolation) =
        self.read_multiline_string_content();

    // Token emission mirrors read_string_start
    if has_interpolation {
        self.enter_interpolated_string();
        self.string_token_with_cursor_end(
            TokenType::InterpolationStart,
            content_start, content_end, line, column,
        )
    } else if !ended {
        self.string_token_with_cursor_end(
            TokenType::UnterminatedString,
            content_start, content_end, line, column,
        )
    } else {
        self.string_token_with_cursor_end(
            TokenType::String,
            content_start, content_end, line, column,
        )
    }
}
```

### Phase 3 — Lexer: `read_multiline_string_content`

Mirrors `read_string_content` but:
- Does **not** break on `\n` or `\r`
- Terminates on `"""`

```rust
fn read_multiline_string_content(&mut self) -> (usize, usize, bool, bool) {
    let span_start = self.current_index();
    let len = self.reader.source_len();
    let mut i = span_start;

    loop {
        while i < len {
            let b  = self.reader.byte_at(i).unwrap_or_default();
            let b1 = self.reader.byte_at(i + 1);
            let b2 = self.reader.byte_at(i + 2);

            // Check for closing """
            if b == b'"' && b1 == Some(b'"') && b2 == Some(b'"') {
                let end = i;
                self.reader.seek_to(i + 3); // consume """
                return (span_start, end, true, false);
            }

            // Check for interpolation #{
            if b == b'#' && b1 == Some(b'{') {
                let end = i;
                self.reader.seek_to(i + 2); // consume #{
                return (span_start, end, false, true);
            }

            // Allow newlines — don't break
            if b == b'\\' || b >= 0x80 {
                break; // handle escape or unicode in slow path below
            }

            i += 1;
        }

        if i >= len {
            self.reader.seek_to(i);
            return (span_start, i, false, false); // unterminated
        }

        // Slow path: escape sequences and unicode (same as regular strings)
        match self.reader.byte_at(i).unwrap_or_default() {
            b'\\' => {
                self.reader.seek_to(i);
                self.consume_escape_sequence();
                i = self.current_index();
            }
            _ => {
                self.reader.seek_to(i);
                self.read_char();
                i = self.current_index();
            }
        }
    }
}
```

### Phase 4 — Indentation Stripping

The closing `"""` column is known at lex time. The stripped content is computed in the
lexer before building the token literal, by scanning the content and removing the
leading `N` spaces from each line (where `N` = closing column).

This can be implemented as a post-processing step on the content string before returning
from `read_multiline_string_content`:

```rust
fn strip_indentation(content: &str, indent: usize) -> String {
    content
        .lines()
        .map(|line| {
            if line.len() >= indent && line[..indent].chars().all(|c| c == ' ') {
                &line[indent..]
            } else {
                line.trim_start() // fallback for under-indented lines
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

### Phase 5 — Parser

**No changes needed.** The parser already handles `TokenType::String` and
`TokenType::InterpolationStart`. The multi-line string lexes into the same token types
with content that happens to include `\n` characters. `decode_string_escapes` in
[literal.rs](../../src/syntax/parser/literal.rs) passes literal newlines through
unchanged.

### Phase 6 — Token Type

**No new token types needed.** The content of a multi-line string is just a `String`
token whose literal happens to contain newline characters. The existing token type
hierarchy is sufficient.

---

## No-Change Zones

| Component | Change needed |
|---|---|
| `token_type.rs` | No — reuse `String`, `InterpolationStart`, `StringEnd` |
| `parser/literal.rs` | No — `decode_string_escapes` already handles `\n` |
| `parser/expression.rs` | No |
| `ast/` | No |
| `bytecode/` | No |
| `runtime/` | No |
| JIT | No |

---

## Error Cases

### Unterminated multi-line string

```flux
let x = """
hello
// EOF without closing """
```

→ `UnterminatedString` token, same error as for regular strings.

### Under-indented content line

```flux
fn f() {
    let x = """
    line1
  line2      ← only 2 spaces, but closing """ has 4
    """
}
```

→ Warning or error: content line has less indentation than the closing `"""`. Emit a
warning and treat the line as having zero extra indentation (trim what's available).

---

## Snapshot Tests

New snapshot test cases needed in `tests/`:

- Lexer snapshot: triple-quoted string tokenisation
- Parser snapshot: `Expression::String` with embedded newlines
- VM test: multi-line string values at runtime
- Indentation stripping: various closing `"""` column positions
- Interpolation inside multi-line string
- Unterminated multi-line string error

---

## Files to Change

| File | Change |
|---|---|
| `src/syntax/lexer/mod.rs` | Detect `"""` in `"` dispatch, call `read_multiline_string_start` |
| `src/syntax/lexer/strings.rs` | Add `read_multiline_string_start`, `read_multiline_string_content`, `strip_indentation` |
| `tests/` | New snapshot + VM tests |

---

## References

- `src/syntax/lexer/strings.rs` — current string lexing
- `src/syntax/lexer/escape.rs` — escape sequence handling
- `src/syntax/parser/literal.rs` — `decode_string_escapes`, `parse_string`
- `src/syntax/token_type.rs` — `TokenType` enum
- Swift multi-line strings: [docs.swift.org](https://docs.swift.org/swift-book/documentation/the-swift-programming-language/stringsandcharacters/#Multiline-String-Literals)
- Kotlin multi-line strings: `trimIndent()` convention
- Python triple-quoted strings: `"""..."""`
