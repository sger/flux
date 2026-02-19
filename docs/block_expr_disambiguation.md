# Block Expression Disambiguation (Step C Follow-up, Analysis Only)

## Scope
This document analyzes disambiguation for block expressions in expression position while preserving existing hash literal behavior.

Target capability:
- `let y = { let x = 1; x + 1 }` (or an equivalent expression form)

Current implementation reality:
- `{ ... }` in expression-prefix position parses as hash literal.
- `do { ... }` already exists as an explicit block expression.
- block tail value semantics are semicolon-sensitive via `Statement::Expression { has_semicolon }`.

---

## 1) Current Meaning of `{ ... }` in Expression Prefix

### Prefix dispatch
In Pratt prefix parsing:
- `TokenType::LBrace => self.parse_hash()` (`src/syntax/parser/expression.rs:119`)
- `TokenType::Do => self.parse_do_block_expression()` (`src/syntax/parser/expression.rs:121`)

This means bare `{ ... }` in expression position is currently a hash literal, not a statement block.

### Hash literal grammar (`parse_hash`)
`parse_hash` is implemented in `src/syntax/parser/expression.rs:545`.

Observed grammar shape:
- starts at `{`
- zero or more entries until `}`
- each entry is:
  - `<expr>` key
  - `:`
  - `<expr>` value
- entries separated by `,`
- empty hash is `{}` (while-loop exits immediately when peek is `}`)

### Block grammar (`parse_block`)
`parse_block` is implemented in `src/syntax/parser/helpers.rs:337`.

Observed grammar shape:
- caller must already have consumed `{`
- parser advances into block and repeatedly parses statements until `}` or `EOF`
- returns `Block { statements: Vec<Statement>, span }`
- reports unclosed delimiter if EOF reached before `}` (`src/syntax/parser/helpers.rs:349`)

---

## 2) Current Block Contexts (`parse_block` Call Sites)

`parse_block` is currently used in these syntactic contexts:

- `if` consequence and `else` alternative:
  - `src/syntax/parser/expression.rs:588`
  - `src/syntax/parser/expression.rs:596`
- `do` block expression:
  - `src/syntax/parser/expression.rs:623`
- function literal body:
  - `src/syntax/parser/expression.rs:885`
- lambda block body (`\x -> { ... }`):
  - `src/syntax/parser/expression.rs:942`
- function statement body:
  - `src/syntax/parser/statement.rs:104`
- module statement body:
  - `src/syntax/parser/statement.rs:250`

Match arm bodies are expression-only:
- arm body uses `parse_expression(...)` (`src/syntax/parser/expression.rs:658`)
- so `{ ... }` there currently means hash literal unless prefixed with `do`.

---

## 3) Concrete Conflict Examples

| Snippet | Parsed today | Desired under bare block-expr design |
|---|---|---|
| `{}` | empty hash (`Expression::Hash`) | ambiguous: empty block expr vs empty hash |
| `{ a }` | parse error in hash grammar (missing `:`) | block expr with tail `a` |
| `{ a: 1 }` | hash with one pair | likely still hash |
| `let y = { let x = 1; x }` | parse as hash path, then fail around `let`/`:` expectations | block expression yielding `x` |
| `match 1 { 1 -> { 2 }, _ -> 0 }` | arm body `{ 2 }` interpreted as hash-like form, not block | arm body block value `2` |
| `fn f() { x }` | function body block (statement context), works | unchanged |

Key ambiguity:
- token `{` in expression position currently has a single meaning (hash).
- enabling bare block expressions would require either syntax split or disambiguation logic.

---

## 4) Disambiguation Strategies

### A) Keyword-introduced block expressions (`do { ... }` / `begin { ... }`)

Surface syntax:
- keep `{...}` as hash literal
- block expr requires keyword prefix, e.g. `do { ... }`

Pros:
- minimal ambiguity; parser remains simple
- zero break for existing hash literals
- explicit and readable at call sites

Cons:
- extra keyword noise for users
- two visual styles for blocks (`if {}`/`fn {}` vs `do {}` expression)

Backward compatibility:
- additive and low risk

Parser touchpoints:
- `parse_prefix` dispatch (`src/syntax/parser/expression.rs:89`)
- dedicated parser (`parse_do_block_expression`, `src/syntax/parser/expression.rs:609`)
- token keyword table (`src/syntax/token_type.rs`)

Edge cases:
- `do` without `{` should produce targeted error (already implemented)
- nested: `do { let x = do { ... }; x }`

---

### B) Change hash syntax (`#{...}` or `hash{...}`)

Surface syntax:
- blocks use `{...}` in expression position
- hashes move to `#{...}` or `hash { ... }`

Pros:
- bare `{...}` can become universal block expression
- aligns with many block-expression languages

Cons:
- high user migration cost
- broad snapshot/test churn
- lexer/parser and docs breakage across ecosystem

Backward compatibility:
- breaking unless transition supports both syntaxes for a deprecation period

Parser touchpoints:
- `parse_prefix` for `LBrace` and hash token path
- `parse_hash` entry conditions
- lexer tokenization for `#{` if chosen

Edge cases:
- interpolation already uses `#{...}` tokenization paths; collisions must be handled carefully

---

### C) Lookahead-based disambiguation inside `{ ... }`

Surface syntax:
- keep both bare hash and bare block using one delimiter pair

Typical heuristic ideas:
- if first top-level separator is `:` => hash
- if statement-only tokens (`let`, `return`, `fn module-form`) appear => block
- if semicolon appears at top level => likely block

Pros:
- no new syntax for users
- can enable requested form directly

Cons:
- high parser complexity and brittle edge cases
- expensive speculative parse / rollback paths
- error reporting quality can degrade under malformed input

Backward compatibility:
- medium risk: previously invalid forms may become valid differently
- some ambiguous constructs may parse differently than today

Parser touchpoints:
- `parse_prefix` LBrace branch (`src/syntax/parser/expression.rs:119`)
- likely new probe/disambiguation helper in parser helpers
- `parse_hash` + block parse integration

Edge cases:
- `{ a }`, `{ a, b }`, `{ a: b + c }`, nested braces in keys/values
- malformed delimiters could increase cascades

---

### D) Contextual disambiguation (only in selected expression contexts)

Surface syntax:
- allow bare block expression only in some contexts (e.g. let initializer, match arm)

Pros:
- can constrain ambiguity locally
- incremental rollout possible

Cons:
- grammar becomes non-uniform and surprising
- `parse_expression` is reused broadly; context threading is invasive
- still fails intuitive portability across contexts

Backward compatibility:
- medium risk and potential user confusion

Parser touchpoints:
- call sites that invoke `parse_expression` (let initializers, arms, args, etc.)
- context flags through Pratt functions

Requirement fit (`let y = { ... }`):
- possible if let-initializer context is explicitly enabled
- but this strategy does not generalize cleanly and adds long-term complexity

---

### E) Keep `do` as canonical block expression (status quo + polish)

Surface syntax:
- `do { ... }` for expression blocks
- `{ ... }` remains hash

Pros:
- already implemented; lowest churn path
- clear separation of meanings
- no ambiguity heuristics needed

Cons:
- does not satisfy preference for bare `{...}` expression directly

Backward compatibility:
- excellent (purely additive)

---

## 5) Semicolon Intent Modeling (Minimal-Churn Planning)

Current state:
- `Statement::Expression` carries `has_semicolon: bool` (`src/syntax/statement.rs:29`)
- parser sets it in `parse_expression_statement` (`src/syntax/parser/statement.rs:58`)
- tail semantics consume it in:
  - bytecode block-tail checks (`src/bytecode/compiler/statement.rs:481`)
  - JIT block evaluation (`src/jit/compiler.rs:2140`)
  - tail-position analysis (`src/ast/tail_position.rs:33`)

### Option 1: `has_semicolon: bool` on `Statement::Expression` (current)

Pros:
- minimal AST shape change (single field)
- simple to thread through existing exhaustive matches
- easy to reason about in compiler/JIT tail checks

Cons:
- semicolon state only exists on expression statements (not globally encoded elsewhere)

Match-impact estimate:
- around 19 `Statement::Expression` match sites in `src` currently (`rg` count)

Downstream impact:
- bytecode compiler tail eligibility
- JIT `StmtOutcome` and block value rules
- AST tail analysis
- Display/formatter behavior

### Option 2: split variants (`ExprNoSemi` / `ExprSemi`)

Pros:
- stronger type-level distinction
- no boolean interpretation ambiguity

Cons:
- larger exhaustive-match churn than a boolean field
- more branching noise in visitors/folds/compiler
- likely no net gain for current language size

Match-impact estimate:
- all current `Statement::Expression` matches split into two-variant handling

Downstream impact:
- same components as Option 1, but broader pattern updates

---

## 6) Recommendation

Recommended disambiguation strategy:
- **Keyword block expressions (`do { ... }`)** as the stable path.

Recommended semicolon strategy:
- **`has_semicolon: bool` on `Statement::Expression`** (already in place).

Why this pairing fits Flux best:
- minimal parser/AST churn
- zero ambiguity with existing hash literals
- strong backward compatibility
- predictable diagnostics and recovery behavior
- consistent with current compiler/JIT/tail-analysis architecture

If bare `{...}` expression support is still desired later, treat it as a separate language evolution with either:
- a hash syntax migration plan, or
- carefully-bounded lookahead with explicit compatibility policy.

---

## 7) Future Implementation Checklist (If Bare `{...}` Is Pursued)

Parser:
- `src/syntax/parser/expression.rs`
  - `parse_prefix` LBrace dispatch
  - add disambiguation/probe path
  - integrate with `parse_hash` and block parse
- `src/syntax/parser/helpers.rs`
  - speculative parse/recovery helpers if needed

AST:
- likely no additional changes if using existing `Block` + `has_semicolon`
- if representation changes, update `src/syntax/block.rs` / `src/syntax/statement.rs`

Bytecode compiler:
- `src/bytecode/compiler/statement.rs` (`compile_block_with_tail`, tail checks)
- `src/bytecode/compiler/expression.rs` (if/match/expr-block lowering)

JIT:
- `src/jit/compiler.rs` (`compile_statement`, `compile_block_expression`)

Analysis passes:
- `src/ast/tail_position.rs`
- `src/ast/visit.rs`, `src/ast/fold.rs`, and other exhaustive matches

Diagnostics:
- targeted ambiguity diagnostics at `{` entry
- recovery heuristics to avoid delimiter cascades

Tests to add:
- parser ambiguity matrix: `{}`, `{a}`, `{a:1}`, nested forms
- runtime: `{ x }` vs `{ x; }` semantics in expression contexts
- regression: hash literals and interpolation remain stable
- snapshot updates for parser/bytecode output
