# Proposal 059: Parser Error Experience — Keyword Aliases, Structural Messages, and Symbol Suggestions

**Status:** Implemented
**Date:** 2026-02-28
**Updated:** 2026-02-28
**Depends on:** `057_parser_diagnostics_with_inferred_types.md` (diagnostic builder pattern, `DiagnosticBuilder` trait)

---

## 1. Summary

The Flux parser currently recovers from most structural mistakes, but the error messages for common developer mistakes — especially those from users arriving from Python, JavaScript, Rust, Haskell, or Ruby — are generic, unhelpful, or misleading. This proposal introduces three targeted improvements:

1. **Keyword aliases** — detect common foreign keywords (`def`, `var`, `const`, `case`, `switch`, `elif`, `end`) and emit actionable suggestions rather than generic "unexpected identifier" errors.
2. **Contextual structural messages** — replace generic `expect_peek` failures for `{`, `=`, and `(` with messages that name the construct being parsed and show the correct syntax.
3. **Symbol suggestions** — detect `|` as a match arm separator and `=>` in match arms, and suggest the correct Flux syntax.

All three are purely diagnostic improvements with no semantic changes. No AST nodes, no runtime changes, no type system interaction.

---

## 2. Motivation

### 2.1 Keyword Aliases — Current Gaps

Flux recognizes `fun`/`function` as typos for `fn` (E030). No other common language-transfer mistakes are caught. All the cases below produce misleading or confusing messages.

**`def` (Python/Ruby → `fn`):**
```
def foo() { 1 }
```
Current:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Unexpected identifier `foo` after expression.

1 | def foo() { 1 }
  | ^^^
```
`def` is parsed as an expression. `foo` then looks like a juxtaposed identifier. The error points at the whole `def` span, which hides the actual problem entirely.

Proposed:
```
-- compiler error[E030]: UNKNOWN KEYWORD
Unknown keyword `def`. Flux uses `fn` for function declarations.

1 | def foo() { 1 }
  | ^^^

Help: Did you mean `fn foo() { ... }`?
```

---

**`var` / `const` / `val` (JS/Kotlin/Scala → `let`):**
```
var x = 1
```
Current:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Unexpected identifier `x` after expression.

1 | var x = 1
  | ^^^
```
Same issue: `var` is an expression, `x` is a juxtaposed identifier. The `let` replacement is never suggested.

Proposed:
```
-- compiler error[E030]: UNKNOWN KEYWORD
Unknown keyword `var`. Flux uses `let` for bindings.

1 | var x = 1
  | ^^^

Help: Did you mean `let x = 1`?
```

---

**`elif` / `elsif` (Python/Ruby → `else if`):**
```
if true { 1 } elif false { 2 } else { 3 }
```
Current:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Unexpected identifier `elif` after expression.

1 | if true { 1 } elif false { 2 } else { 3 }
  | ^^^^^^^^^^^^^
```
Points at the whole `if` expression span, not at `elif`. The suggestion to write `else if` is never made.

Proposed:
```
-- compiler error[E030]: UNKNOWN KEYWORD
Unknown keyword `elif`. Flux uses `else if` for chained conditionals.

1 | if true { 1 } elif false { 2 } else { 3 }
  |               ^^^^

Help: Replace `elif` with `else if`.
```

---

**`case` / `switch` (Haskell/Rust/Ruby → `match`):**
```
fn main() -> Unit { case 1 { 0 -> 0, _ -> 1 } }
```
Current:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Expected `:`, got `->`.

1 | fn main() -> Unit { case 1 { 0 -> 0, _ -> 1 } }
  |                                ^^
```
`case` parses as an identifier; `1 { 0 ... }` is misread as a hash literal where `:` is expected. The real problem (`case` is not a keyword) is never surfaced.

Proposed:
```
-- compiler error[E030]: UNKNOWN KEYWORD
Unknown keyword `case`. Flux uses `match` for pattern matching.

1 | fn main() -> Unit { case 1 { 0 -> 0, _ -> 1 } }
  |                     ^^^^

Help: Did you mean `match 1 { 0 -> 0, _ -> 1 }`?
```

---

**`end` (Ruby/ML → `}`):**
```
fn foo() { 1 end
```
Current:
```
-- compiler error[E076]: UNCLOSED DELIMITER
Expected a closing `}` to match this opening `{` in function `foo`.

1 | fn foo() { 1 end
  |          ^
```
`end` is silently consumed as an expression, and the block is reported as unclosed. While the unclosed message is correct, it doesn't tell the programmer why — that `end` was written instead of `}`.

Proposed (additional diagnostic before the unclosed error):
```
-- compiler error[E034]: UNEXPECTED TOKEN
`end` is not a keyword in Flux. Use `}` to close blocks.

1 | fn foo() { 1 end
  |              ^^^

Help: Replace `end` with `}`.
```

---

### 2.2 Structural Messages — Current Gaps

**Missing parameter list `()` in function declaration:**
```
fn foo -> Int { 1 }
```
Current:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Expected `(`, got `->`.

1 | fn foo -> Int { 1 }
  |        ^^
```
The error is accurate but terse. The programmer doesn't know what the complete correct syntax is.

Proposed:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Missing parameter list for function `foo`. Write `fn foo()` or `fn foo(x: Int)`.

1 | fn foo -> Int { 1 }
  |        ^^

Help: Function declarations require a parameter list: `fn foo(...) -> Type { ... }`
```

---

**Missing `{` to begin `if` body:**
```
if true 1 else 2
```
Current:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Expected `{`, got `INT`.

1 | if true 1 else 2
  |         ^
```
Does not explain that Flux requires braces for all `if` bodies, or what the correct form looks like.

Proposed:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Expected `{` to begin the `if` body.

1 | if true 1 else 2
  |         ^

Help: Flux requires braces: `if condition { ... } else { ... }`
```

---

**Missing `=` in `let` binding:**
```
fn main() -> Unit { let x 1 }
```
Current:
```
-- compiler error[E076]: UNCLOSED DELIMITER
Expected a closing `}` to match this opening `{` in function `main`.

1 | fn main() -> Unit { let x 1 }
  |                   ^
```
Completely wrong error: the real problem is the missing `=`, but the parser recovers in a way that makes the block look unclosed.

Proposed (catch the missing `=` before it cascades):
```
-- compiler error[E034]: UNEXPECTED TOKEN
Expected `=` after `let x`. Did you mean `let x = 1`?

1 | fn main() -> Unit { let x 1 }
  |                           ^

Help: Let bindings require `=`: `let name = value`
```

---

### 2.3 Symbol Suggestions — Current Gaps

**`|` as match arm separator (Haskell style):**
```
match 1 { 0 -> zero | _ -> other }
```
Current:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Expected `,` or `}` after match arm, got |.

1 | match 1 { 0 -> zero | _ -> other }
  |                     ^
```
The message is accurate but offers no hint about the Haskell/Elm habit of using `|`.

Proposed (extend the match arm separator detection, similar to the `;` → `,` path):
```
-- compiler error[E034]: UNEXPECTED TOKEN
Match arms are separated by `,` in Flux, not `|`.

1 | match 1 { 0 -> zero | _ -> other }
  |                     ^

Help: Replace `|` with `,`.
```
Recovery: treat `|` as `,` and continue parsing (same recovery as the `;` path today).

---

**`=>` in match arm (Rust/JavaScript style):**
```
match 1 { 0 => zero, _ => other }
```
Current:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Expected `->`, got `=`.

1 | match 1 { 0 => zero, _ => other }
  |             ^
```
The error shows `=` (because `=>` is lexed as `Assign` + `>`), not `=>`. The message is accurate but doesn't explain the correct syntax or name the token properly.

Proposed:
```
-- compiler error[E034]: UNEXPECTED TOKEN
Expected `->` in match arm, found `=>`. Flux uses `->` not `=>`.

1 | match 1 { 0 => zero, _ => other }
  |             ^^

Help: Replace `=>` with `->`: `match x { pattern -> body, ... }`
```

---

## 3. Goals

1. Intercept common foreign-keyword patterns (`def`, `var`, `const`, `val`, `case`, `switch`, `when`, `elif`, `elsif`, `end`) and emit named suggestions pointing at the aliased Flux keyword.
2. Replace generic `expect_peek` failure messages for `{` after `if`/`else` and `(` after function name with context-specific messages naming the construct and showing the correct form.
3. Detect `=` missing from `let` bindings before the missing-`=` causes a cascade error.
4. Detect `|` as a match arm separator and recover identically to the existing `;` path (treat as `,`, continue).
5. Improve the `=>` match arm message to name both tokens (`=>` → `->`) rather than showing only `=`.
6. No changes to AST, runtime, type system, or PrimOps.
7. No regressions on existing parser snapshot tests.

---

## 4. Non-Goals

1. No changes to the lexer token set. Keywords remain as-is; detection is purely syntactic pattern matching in the parser.
2. No `end` keyword support — the diagnostic fires and parsing continues looking for `}`.
3. No detection of `then` (ML-style) as an alternative to `{` in `if`.
4. No multi-language `where`-clause keyword aliasing (already a Flux feature).
5. No changes to error codes — all new diagnostics remain E030 (unknown keyword) or E034 (unexpected token) as appropriate.

---

## 5. Detailed Design

### 5.1 Keyword Aliases (E030)

**Detection site:** `parse_statement()` in `syntax/parser/statement.rs`, immediately before the `_ => parse_expression_statement()` fallthrough (lines 64–89).

**Pattern:** The current `fun`/`function` → `fn` arm (lines 68–83) serves as the template. Each new alias arm matches on `TokenType::Ident` with a literal check:

```rust
// def → fn
TokenType::Ident
    if matches!(self.current_token.literal.as_ref(), "def")
        && self.is_peek_token(TokenType::Ident) =>
{
    self.errors.push(
        unknown_keyword(self.current_token.span(), "def", None)
            .with_message("Unknown keyword `def`. Flux uses `fn` for function declarations.")
            .with_hint_text("Did you mean `fn`?"),
    );
    None
}

// var / const / val → let
TokenType::Ident
    if matches!(self.current_token.literal.as_ref(), "var" | "const" | "val")
        && self.is_peek_token(TokenType::Ident) =>
{
    let kw = self.current_token.literal.to_string();
    self.errors.push(
        unknown_keyword(self.current_token.span(), &kw, None)
            .with_message(format!("Unknown keyword `{kw}`. Flux uses `let` for bindings."))
            .with_hint_text("Did you mean `let`?"),
    );
    None
}

// case / switch / when → match
TokenType::Ident
    if matches!(self.current_token.literal.as_ref(), "case" | "switch" | "when") =>
{
    let kw = self.current_token.literal.to_string();
    self.errors.push(
        unknown_keyword(self.current_token.span(), &kw, None)
            .with_message(format!(
                "Unknown keyword `{kw}`. Flux uses `match` for pattern matching."
            ))
            .with_hint_text("Did you mean `match`?"),
    );
    None
}
```

**`elif` / `elsif` detection:** These appear as identifiers after an expression (after the closing `}` of an `if` body). They are caught by `parse_expression_statement()` via the juxtaposed-identifier path. A better interception point is after `parse_if_expression()` returns in the expression parser — check if `peek_token` is an `Ident` with literal `elif` or `elsif`, and emit there before falling through to the generic juxtaposed path.

Alternative: add a post-expression check in `parse_expression_statement()` (lines 114–116) that specifically recognizes `elif`/`elsif` as a juxtaposed identifier and emits the alias diagnostic before the generic one.

**`end` detection:** `end` appears as an expression-starting identifier inside blocks. Detection site: `parse_expression_statement()`, add a check before parsing the expression:

```rust
TokenType::Ident if self.current_token.literal == "end" => {
    self.errors.push(
        unexpected_token(self.current_token.span(),
            "`end` is not a keyword in Flux. Use `}` to close blocks.")
            .with_hint_text("Replace `end` with `}`.")
    );
    return None;  // let synchronize find the real `}`
}
```

---

### 5.2 Contextual Structural Messages

**Missing `(` in function declaration:**

In `parse_function_statement()` (`statement.rs`), the current call is:
```rust
if !self.expect_peek(TokenType::LParen) { return None; }
```

Replace with a contextual check when the next token is `Arrow` (indicating the programmer wrote `fn name -> Type`):

```rust
if self.is_peek_token(TokenType::Arrow) {
    // `fn name -> Type` — missing parameter list
    let fn_name = self.current_token.literal.to_string();
    self.errors.push(unexpected_token(
        self.peek_token.span(),
        format!("Missing parameter list for function `{fn_name}`. Write `fn {fn_name}()` or `fn {fn_name}(x: Type)`.")
    ).with_hint_text("Function declarations require a parameter list: `fn name(...) -> Type { ... }`"));
    // Synthesize empty params and continue parsing to avoid cascade
    return self.parse_function_statement_body(name, span, ...);
} else if !self.expect_peek(TokenType::LParen) {
    return None;
}
```

**Missing `{` after `if` condition:**

In `parse_if_expression()` (`expression.rs`), replace:
```rust
if !self.expect_peek(TokenType::LBrace) { return None; }
```
with:
```rust
if !self.is_peek_token(TokenType::LBrace) {
    self.errors.push(unexpected_token(
        self.peek_token.span(),
        "Expected `{` to begin the `if` body."
    ).with_hint_text("Flux requires braces: `if condition { ... } else { ... }`"));
    return None;
}
self.next_token();
```

Same for the `else` body (`expect_peek(TokenType::LBrace)` on line 867).

**Missing `=` in `let` binding:**

In `parse_let_statement()` (`statement.rs`), the current `expect_peek(TokenType::Assign)` call fires a generic "unexpected token (expected `=`)" message. Replace with a contextual check:

```rust
if !self.is_peek_token(TokenType::Assign) {
    let binding_name = /* already resolved name */;
    self.errors.push(unexpected_token(
        self.peek_token.span(),
        format!("Expected `=` after `let {binding_name}`. Did you mean `let {binding_name} = ...`?")
    ).with_hint_text("Let bindings require `=`: `let name = value`"));
    return None;
}
self.next_token(); // consume `=`
```

---

### 5.3 Match Arm Symbol Suggestions

**`|` as arm separator:**

In `parse_match_expression()` (`expression.rs`), the `_` arm of the separator `match` block (lines 1108–1115) currently emits "Expected `,` or `}` after match arm, got |." when `peek_token` is `Pipe`. Extend with a specific arm before `_`:

```rust
TokenType::Pipe => {
    // Haskell-style `|` separator — recover like `;`
    self.errors.push(unexpected_token(
        self.peek_token.span(),
        "Match arms are separated by `,` in Flux, not `|`."
    ).with_hint_text("Replace `|` with `,`."));
    self.next_token(); // treat `|` as `,` and continue
}
```

**`=>` in match arm:**

When `expect_peek(TokenType::Arrow)` fails because the next token is `Assign` (the `=` of `=>`), detect the lookahead: if `current` is `Assign` and `peek` is `Greater`, emit a specific message naming `=>`:

In `parse_match_expression()` at the `expect_peek(TokenType::Arrow)` site (line 1075), add a pre-check:

```rust
if self.is_peek_token(TokenType::Assign) && self.peek2_token.token_type == TokenType::Gt {
    self.errors.push(unexpected_token(
        self.peek_token.span(),
        "Expected `->` in match arm, found `=>`. Flux uses `->` not `=>`."
    ).with_hint_text("Replace `=>` with `->`: `match x { pattern -> body, ... }`"));
    self.next_token(); // consume `=`
    self.next_token(); // consume `>`
    // continue as if we found `->`
} else if !self.expect_peek(TokenType::Arrow) {
    return None;
}
```

---

## 6. New Diagnostic Constructors

No new error codes are needed. The improvements use existing codes with better message strings. For readability, add named constructor helpers in `compiler_errors.rs` (following the pattern of `missing_function_body_brace`, `emit_match_semicolon_separator_diagnostic`):

| Helper | Code | Context |
|---|---|---|
| `unknown_keyword_alias(span, found, suggestion)` | E030 | Generic template for all keyword alias errors |
| `missing_if_body_brace(span)` | E034 | `if condition expr` without `{` |
| `missing_else_body_brace(span)` | E034 | `else expr` without `{` |
| `missing_let_assign(span, name)` | E034 | `let x 1` without `=` |
| `missing_fn_param_list(span, fn_name)` | E034 | `fn foo ->` without `()` |
| `match_pipe_separator(span)` | E034 | `|` used as match arm separator |
| `match_fat_arrow(span)` | E034 | `=>` used instead of `->` in match arm |

---

## 7. Files Modified

| File | Change |
|---|---|
| `src/syntax/parser/statement.rs` | Add keyword alias arms before `_ => parse_expression_statement()` fallthrough; improve `expect_peek(Assign)` in `parse_let_statement`; improve `expect_peek(LParen)` in `parse_function_statement` |
| `src/syntax/parser/expression.rs` | Improve `expect_peek(LBrace)` in `parse_if_expression`; add `Pipe` and `=>` arms to match arm separator dispatch |
| `src/diagnostics/compiler_errors.rs` | Add constructor helpers listed in §6 |

---

## 8. Execution Plan (T0–T7)

### T0 — Baseline & Guardrails (No behavior change)
- **Goal:** Freeze parser baseline and lock non-regression policy.
- **Changes:** lock scope to parser diagnostics/recovery (`E030`/`E034`) with no grammar/token/semantic changes.
- **Tests:** `cargo check --all --all-features`, `parser_tests`, `parser_recovery`, `snapshot_parser`, `snapshot_diagnostics`.
- **Done When:** baseline outcomes are recorded and policy is explicit.

### T1 — Diagnostic Constructor Foundation
- **Goal:** Add parser-focused constructor helpers before callsite wiring.
- **Files:** `src/diagnostics/compiler_errors.rs`, `tests/error_codes_registry_tests.rs`.
- **Changes:** add helpers:
  - `unknown_keyword_alias`
  - `missing_if_body_brace`
  - `missing_else_body_brace`
  - `missing_let_assign`
  - `missing_fn_param_list`
  - `match_pipe_separator`
  - `match_fat_arrow`
  - `unexpected_end_keyword`
- **Done When:** constructor shape tests pass and no parser behavior changes are required.

### T2 — Keyword Alias Detection at Statement Boundary
- **Goal:** Emit targeted `E030` for statement-level foreign keywords.
- **Files:** `src/syntax/parser/statement.rs`, `src/diagnostics/compiler_errors.rs`.
- **Changes:** detect `def`, `var`/`const`/`val`, `case`/`switch`/`when` in statement dispatch.
- **Fixtures:** `112`, `113`, `114`.
- **Done When:** aliases no longer fall through to generic cascade diagnostics.

### T3 — `elif`/`elsif` + `end` Experience
- **Goal:** Improve chained-conditional and block-closure keyword mistakes.
- **Files:** `src/syntax/parser/statement.rs`, `src/syntax/parser/expression.rs`.
- **Changes:** detect `elif`/`elsif` in juxtaposed conditional context; detect `end` at expression-statement start.
- **Fixtures:** `115`, `116`.
- **Done When:** targeted diagnostics are emitted and parser continuation is preserved.

### T4 — Structural Context Messages (`(`, `{`, `=`)
- **Goal:** Replace generic structural failures with construct-aware diagnostics.
- **Files:** `src/syntax/parser/statement.rs`, `src/syntax/parser/expression.rs`.
- **Changes:** contextual messages for:
  - missing function parameter list (`fn foo -> Int`)
  - missing `{` after `if` / `else`
  - missing `=` in `let` binding
- **Fixtures:** `117`, `118`, `119`.
- **Done When:** all three emit targeted messages with syntax-form hints.

### T5 — Match Symbol Suggestions and Recovery
- **Goal:** Improve match arm diagnostics/recovery for `|` and `=>`.
- **Files:** `src/syntax/parser/expression.rs`, `src/diagnostics/compiler_errors.rs`.
- **Changes:** treat `|` as separator recovery like `;`; detect/report `=>` and suggest `->`.
- **Fixtures:** `120`, `121`.
- **Done When:** targeted diagnostics appear and parser continues deterministically.

### T6 — False-Positive Hardening + Snapshot Lock
- **Goal:** Ensure diagnostics only trigger in intended contexts.
- **Files:** `tests/parser_tests.rs`, `tests/parser_recovery.rs`, snapshot suites.
- **Changes:** add negative tests for identifier/member-access non-misfires and duplicate/cascade control.
- **Done When:** parser and snapshot tests pass with intentional-only diffs.

### T7 — Docs/Fixture/Evidence Closure
- **Goal:** Make 059 auditable and merge-ready.
- **Files:** `docs/proposals/059_parser_error_experience.md`, `examples/type_system/failing/README.md`.
- **Changes:** document fixture matrix `112..121`, command evidence, and closure status.
- **Done When:** docs and fixture command lists are aligned with implementation.

---

## 9. Task Dependencies

```
T0 -> T1 -> T2 -> T3 -> T4 -> T5 -> T6 -> T7
```

Notes:
- `T2` and `T4` can run in parallel only after `T1` is merged and constructor contracts are stable.
- `T6`/`T7` are closure gates and always last.

---

## 10. Guardrails

1. No parser grammar/token-set changes.
2. No AST/type/runtime behavior changes.
3. Diagnostics stay in `E030`/`E034` families for 059 scope.
4. One task per PR; no mixed-feature PRs.
5. Snapshot updates accepted only when intentional and task-attributed.

---

## 11. Fixtures

### Failing (error expected)
- `examples/type_system/failing/112_keyword_alias_def.flx`
- `examples/type_system/failing/113_keyword_alias_var.flx`
- `examples/type_system/failing/114_keyword_alias_case.flx`
- `examples/type_system/failing/115_keyword_alias_elif.flx`
- `examples/type_system/failing/116_keyword_alias_end.flx`
- `examples/type_system/failing/117_if_missing_brace.flx`
- `examples/type_system/failing/118_let_missing_eq.flx`
- `examples/type_system/failing/119_fn_missing_parens.flx`
- `examples/type_system/failing/120_match_pipe_separator.flx`
- `examples/type_system/failing/121_match_fat_arrow.flx`

### Recovery fixture
- `tests/fixtures/recovery/059_parser_error_recovery.flx`

---

## 12. Validation Gate

```bash
cargo check --all --all-features
cargo test --test error_codes_registry_tests
cargo test --test parser_tests
cargo test --test parser_recovery
cargo test --test snapshot_parser
cargo test --test snapshot_diagnostics
cargo test --all --all-features purity_vm_jit_parity_snapshots
```

Focused evidence:
```bash
cargo run -- --no-cache examples/type_system/failing/112_keyword_alias_def.flx
cargo run -- --no-cache examples/type_system/failing/115_keyword_alias_elif.flx
cargo run -- --no-cache examples/type_system/failing/117_if_missing_brace.flx
cargo run -- --no-cache examples/type_system/failing/120_match_pipe_separator.flx
```

---

## 13. Acceptance Criteria

1. Keyword aliases (`def`, `var`/`const`/`val`, `case`/`switch`/`when`) emit targeted `E030`.
2. `elif`/`elsif` and `end` emit targeted contextual diagnostics with parser continuation.
3. Structural cases (`if` brace, `let` assign, function parameter list) emit construct-aware `E034`.
4. Match `|` and `=>` cases emit targeted suggestions and preserve recovery.
5. Negative tests confirm no false positives for identifier/member-access contexts.
6. Parser/snapshot/parity gates pass with intentional-only snapshot diffs.
