- Feature Name: Multi-line String Literals
- Start Date: 2026-02-20
- Proposal PR: 
- Flux Issue: 

# Proposal 0036: Multi-line String Literals

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Multi-line String Literals in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

Flux strings cannot span multiple lines. The lexer terminates any string at the first
newline character ([strings.rs:149](../../src/syntax/lexer/strings.rs)): Flux strings cannot span multiple lines. The lexer terminates any string at the first
newline character ([strings.rs:149](../../src/syntax/lexer/strings.rs)):

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Proposed Syntax

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

### Proposed Syntax

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Indentation Stripping:** When multi-line strings are embedded in indented code, the leading whitespace on each line is part of the content unless stripped. This proposal def...
- **Indentation Stripping:** When multi-line strings are embedded in indented code, the leading whitespace on each line is part of the content unless stripped. This proposal defines **automatic indentation...
- **Interpolation in Multi-line Strings:** Works identically to regular strings: ```flux let rows = 3 let summary = """ Input has #{rows} rows. Each row is space-separated integers. """ ```
- **Test fixture data:** ```flux fn test_parse_reports() { let input = """ 7 6 4 2 1 1 2 7 8 9 9 7 6 2 1 """ let lines = split(trim(input), "\n") |> filter(\l -> trim(l) != "") assert_eq(len(lines), 3)...
- **Parsing the multi-line input:** let input = """ 7 6 4 2 1 1 2 7 8 9 9 7 6 2 1 1 3 2 4 5 8 6 4 4 1 1 3 6 7 9 """
- **With interpolation:** ```flux let n = 42 let msg = """ Result: #{n} Done. """ ```

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- `src/syntax/lexer/strings.rs` — current string lexing
- `src/syntax/lexer/escape.rs` — escape sequence handling
- `src/syntax/parser/literal.rs` — `decode_string_escapes`, `parse_string`
- `src/syntax/token_type.rs` — `TokenType` enum
- Swift multi-line strings: [docs.swift.org](https://docs.swift.org/swift-book/documentation/the-swift-programming-language/stringsandcharacters/#Multiline-String-Literals)
- Kotlin multi-line strings: `trimIndent()` convention
- Python triple-quoted strings: `"""..."""`

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
