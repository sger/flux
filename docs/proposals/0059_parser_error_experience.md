- Feature Name: Parser Error Experience — Keyword Aliases, Structural Messages, and Symbol Suggestions
- Start Date: 2026-02-28
- Proposal PR: 
- Flux Issue: 

# Proposal 0059: Parser Error Experience — Keyword Aliases, Structural Messages, and Symbol Suggestions

## Summary
[summary]: #summary

The Flux parser currently recovers from most structural mistakes, but the error messages for common developer mistakes — especially those from users arriving from Python, JavaScript, Rust, Haskell, or Ruby — are generic, unhelpful, or misleading. This proposal introduces three targeted improvements: The Flux parser currently recovers from most structural mistakes, but the error messages for common developer mistakes — especially those from users arriving from Python, JavaScript, Rust, Haskell, or Ruby — are generic, unhelpful, or misleading. This proposal introduces three targeted improvements:

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Intercept common foreign-keyword patterns (`def`, `var`, `const`, `val`, `case`, `switch`, `when`, `elif`, `elsif`, `end`) and emit named suggestions pointing at the aliased Flux keyword.
2. Replace generic `expect_peek` failure messages for `{` after `if`/`else` and `(` after function name with context-specific messages naming the construct and showing the correct form.
3. Detect `=` missing from `let` bindings before the missing-`=` causes a cascade error.
4. Detect `|` as a match arm separator and recover identically to the existing `;` path (treat as `,`, continue).
5. Improve the `=>` match arm message to name both tokens (`=>` → `->`) rather than showing only `=`.
6. No changes to AST, runtime, type system, or PrimOps.
7. No regressions on existing parser snapshot tests.

### 4. Non-Goals

1. No changes to the lexer token set. Keywords remain as-is; detection is purely syntactic pattern matching in the parser.
2. No `end` keyword support — the diagnostic fires and parsing continues looking for `}`.
3. No detection of `then` (ML-style) as an alternative to `{` in `if`.
4. No multi-language `where`-clause keyword aliasing (already a Flux feature).
5. No changes to error codes — all new diagnostics remain E030 (unknown keyword) or E034 (unexpected token) as appropriate.

### 3. Goals

### 4. Non-Goals

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **2.1 Keyword Aliases — Current Gaps:** Flux recognizes `fun`/`function` as typos for `fn` (E030). No other common language-transfer mistakes are caught. All the cases below p...
- **2.1 Keyword Aliases — Current Gaps:** Flux recognizes `fun`/`function` as typos for `fn` (E030). No other common language-transfer mistakes are caught. All the cases below produce misleading or confusing messages.
- **2.2 Structural Messages — Current Gaps:** **Missing parameter list `()` in function declaration:** ``` fn foo -> Int { 1 } ``` Current: ``` -- compiler error[E034]: UNEXPECTED TOKEN Expected `(`, got `->`.
- **2.3 Symbol Suggestions — Current Gaps:** **`|` as match arm separator (Haskell style):** ``` match 1 { 0 -> zero | _ -> other } ``` Current: ``` -- compiler error[E034]: UNEXPECTED TOKEN Expected `,` or `}` after match...
- **5.1 Keyword Aliases (E030):** **Detection site:** `parse_statement()` in `syntax/parser/statement.rs`, immediately before the `_ => parse_expression_statement()` fallthrough (lines 64–89).
- **5.2 Contextual Structural Messages:** In `parse_function_statement()` (`statement.rs`), the current call is: ```rust if !self.expect_peek(TokenType::LParen) { return None; } ```

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. No changes to the lexer token set. Keywords remain as-is; detection is purely syntactic pattern matching in the parser.
2. No `end` keyword support — the diagnostic fires and parsing continues looking for `}`.
3. No detection of `then` (ML-style) as an alternative to `{` in `if`.
4. No multi-language `where`-clause keyword aliasing (already a Flux feature).
5. No changes to error codes — all new diagnostics remain E030 (unknown keyword) or E034 (unexpected token) as appropriate.

### 4. Non-Goals

### 4. Non-Goals

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
