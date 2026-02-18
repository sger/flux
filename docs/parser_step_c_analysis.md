# Step C Analysis: Block Tail Values (Analysis Only)

## Scope
This document analyzes the impact of making block bodies yield a value via tail expression semantics, without implementing changes.

## 1) Current AST + Semantics for Blocks and Expression Statements

### AST shape
- `Block` is currently:
  - `Block { statements: Vec<Statement>, span: Span }`
  - Source: `src/syntax/block.rs:6`
- `Statement` has a distinct expression-statement variant:
  - `Statement::Expression { expression: Expression, span: Span }`
  - Source: `src/syntax/statement.rs:29`
- `Expression::If` and `Expression::Function` both embed `Block`:
  - `Expression::If { consequence: Block, alternative: Option<Block>, ... }`
  - `Expression::Function { body: Block, ... }`
  - Source: `src/syntax/expression.rs:101`, `src/syntax/expression.rs:107`

### Answers to requested questions
- A) Do expression-statements exist as distinct Statement variant?
  - Yes: `Statement::Expression` (`src/syntax/statement.rs:29`).
- B) Is there already a “return value of a block” concept downstream?
  - Yes, effectively:
    - Bytecode compiler has `compile_block_with_tail` and treats last `Statement::Expression`/`Return` as tail-eligible (`src/bytecode/compiler/statement.rs:471`).
    - `compile_if_expression` removes trailing `OpPop` from branches, producing branch values (`src/bytecode/compiler/expression.rs:506`, `src/bytecode/compiler/expression.rs:525`).
    - JIT `compile_block_expression` returns last statement value or `None` (`src/jit/compiler.rs:2110`, `src/jit/compiler.rs:2142`).
- C) Are blocks used in `if` and lambda only, or more broadly?
  - More broadly:
    - `if` branches (`src/syntax/parser/expression.rs:581`)
    - function literals (`src/syntax/parser/expression.rs:857`)
    - lambda block body (`src/syntax/parser/expression.rs:912`)
    - function declarations (`src/syntax/parser/statement.rs:100`)
    - module declarations (`src/syntax/parser/statement.rs:246`)

## 2) Block-as-Expression Call Sites Today

### Parser forms containing blocks
- `if` expression branches:
  - `parse_if_expression` uses `parse_block` for consequence/alternative
  - `src/syntax/parser/expression.rs:572`, `src/syntax/parser/expression.rs:581`, `src/syntax/parser/expression.rs:589`
- function literal body:
  - `parse_function_literal` uses `parse_block`
  - `src/syntax/parser/expression.rs:842`, `src/syntax/parser/expression.rs:857`
- lambda body:
  - block form uses `parse_block`; expression form is wrapped into synthetic `Block { statements: [Statement::Expression] }`
  - `src/syntax/parser/expression.rs:912`, `src/syntax/parser/expression.rs:920`
- function statement body:
  - `parse_function_statement` uses `parse_block`
  - `src/syntax/parser/statement.rs:78`, `src/syntax/parser/statement.rs:100`
- module statement body:
  - `parse_module_statement` uses `parse_block`
  - `src/syntax/parser/statement.rs:233`, `src/syntax/parser/statement.rs:246`

### Match arm bodies
- Match arms are expression-only today:
  - `let body = self.parse_expression(Precedence::Lowest)?;`
  - `src/syntax/parser/expression.rs:630`
- Important syntax constraint:
  - `LBrace` in expression-prefix position is currently parsed as **hash literal** (`parse_hash`), not block expression.
  - `src/syntax/parser/expression.rs:119`, `src/syntax/parser/expression.rs:536`

## 3) Downstream Pipeline Impact Map

### Parser
Impacted files:
- `src/syntax/parser/helpers.rs` (`parse_block`)
- `src/syntax/parser/statement.rs` (`parse_expression_statement` semicolon handling)
- `src/syntax/parser/expression.rs` (if/lambda/match call sites, potential block-expression entry)

Likely minimal changes needed:
- Detect/represent tail expression in block parse result.
- Preserve semicolon significance for last expression (`{ x }` vs `{ x; }`).
- If enabling standalone block expressions (`let y = { ... }`), resolve conflict with hash literal `{...}` parsing.

Tricky edges:
- `{ ... }` currently means hash literal in expression prefix.
- Match arm body currently expression-only and `{...}` is hash, so “block arm body” is not a parser-only toggle.

### AST definitions
Impacted files:
- `src/syntax/block.rs`
- potentially `src/syntax/statement.rs` (if semicolon-presence must be carried there)

Likely minimal changes needed:
- Option-dependent (see strategies): either add explicit block tail field, or carry expression-termination metadata.

Tricky edges:
- Existing visitors/folders/desugar/rename/free-vars/tail-analysis all destructure `Block` and `Statement` exhaustively.

### AST passes / analysis infra
Impacted files:
- `src/ast/visit.rs`
- `src/ast/fold.rs`
- `src/ast/free_vars.rs`
- `src/ast/complexity.rs`
- `src/ast/tail_position.rs`
- `src/ast/desugar.rs`, `src/ast/constant_fold.rs`, `src/ast/rename.rs`

Likely minimal changes needed:
- Traverse/analyze new block tail representation (if introduced).
- Update tail-position analysis rules if tail moves out of statement list.

Tricky edges:
- `tail_position` currently mirrors bytecode compiler’s “last statement” logic (`src/ast/tail_position.rs:33`).

### Typechecker
- No first-class static typechecker module found in current pipeline.
- Current runtime type checks are in builtins/runtime only (e.g. `src/runtime/builtins/type_check.rs`).

Likely minimal changes needed:
- N/A now, unless a future static type system (proposal 030) consumes block-tail semantics.

### Bytecode compiler / lowering
Impacted files:
- `src/bytecode/compiler/statement.rs`
- `src/bytecode/compiler/expression.rs`

Likely minimal changes needed:
- Align block compilation with new tail representation.
- Preserve existing branch-value behavior in `if` compilation (`src/bytecode/compiler/expression.rs:489`).
- Keep function/lambda return synthesis consistent (`src/bytecode/compiler/statement.rs:269`, `src/bytecode/compiler/expression.rs:411`).

Tricky edges:
- Current behavior derives value from last expression statement + pop removal.
- Semicolon-sensitive behavior is not representable today.

### VM execution semantics
Impacted files:
- Primarily compiler-emitted bytecode contracts; VM opcodes likely unchanged.
- Relevant opcodes/behavior: `OpPop`, `OpReturnValue`, `OpReturn`, branch flow in dispatch.
- `src/runtime/vm/dispatch.rs:127`, `src/runtime/vm/dispatch.rs:138`

Likely minimal changes needed:
- Usually none if compiler lowering stays stack-correct.

Tricky edges:
- If lowering changes stack discipline for block tails, VM stack invariants must still hold.

### JIT lowering
Impacted files:
- `src/jit/compiler.rs`

Likely minimal changes needed:
- Update `compile_block_expression` tail-value extraction logic (`src/jit/compiler.rs:2110`).
- Keep parity with bytecode backend for `if` branches and function/lambda returns.

Tricky edges:
- JIT currently treats any `Statement::Expression` as value-producing (`src/jit/compiler.rs:1006`).

### Pretty printing / formatter / diagnostics
Impacted files:
- AST display: `src/syntax/statement.rs`, `src/syntax/expression.rs`, `src/syntax/block.rs`
- source formatter: `src/syntax/formatter.rs` (textual only)
- parser diagnostics around semicolon hints and expression expectation

Likely minimal changes needed:
- If semicolon significance becomes semantic, printers and diagnostics should reflect `{ x }` vs `{ x; }` clearly.

Tricky edges:
- Existing statement display already omits semicolon for expression statements in display output.

## 4) Semicolon / Terminator Rules Today

### Current rule summary
- Semicolons are optional in many places:
  - expression statement: optional `;` (`src/syntax/parser/statement.rs:68`)
  - `let` / `return` / assignment: optional trailing `;` (`src/syntax/parser/statement.rs:126`, `src/syntax/parser/statement.rs:190`, `src/syntax/parser/statement.rs:222`)
- Newlines are not tokens; lexer skips `\n` as whitespace:
  - `skip_ascii_whitespace` consumes `b'\n'` (`src/syntax/lexer/reader.rs:225`)
- `parse_block` determines boundaries by repeatedly calling `parse_statement` until `}`/EOF (`src/syntax/parser/helpers.rs:337`).

### Implications for block-tail semantics
- Today `{ x }` and `{ x; }` are not distinguished in AST semantics for expression statements.
- Current lowerings already treat last expression statement as block value in key contexts.
- If Step C requires semicolon-sensitive semantics:
  - `{ x }` => yields `x`
  - `{ x; }` => yields `None`/unit
  - then parser/AST must capture termination intent explicitly (not currently modeled).

### `let y = { ... }` under current grammar
- Not currently valid as block expression syntax because `{...}` in expression position parses as hash literal prefix.
- Enabling this requires syntax disambiguation or a new expression form.

## 5) Implementation Strategies (2–3)

## Option 1: AST change — `Block` has explicit tail expression
Example shape:
- `Block { statements: Vec<Statement>, tail: Option<Expression>, span: Span }`

Touchpoints:
- Parser: `parse_block` (`src/syntax/parser/helpers.rs`)
- AST: `src/syntax/block.rs`
- Traversal/folds: `src/ast/visit.rs`, `src/ast/fold.rs`, `src/ast/free_vars.rs`, etc.
- Bytecode/JIT: `compile_block`, `compile_block_with_tail`, `compile_block_expression`

Pros:
- Semantically explicit and robust.
- Cleanly supports `{ x }` vs `{ x; }`.
- Makes tail-value intent first-class for future type system.

Cons:
- Medium-to-high churn across AST passes and both backends.
- Still does not alone solve `{ ... }` vs hash-literal syntax conflict in general expression position.

Compatibility risks:
- If semicolon-sensitive behavior is introduced, existing code relying on `{ x; }` yielding `x` may break.

If/lambda/match interaction:
- If/lambda straightforward: use block tail when present.
- Match arms remain expression-only unless separately extended.

## Option 2: No `Block` shape change — treat last `Statement::Expression` specially
Definition:
- Keep `Block { statements }`.
- Block value comes from last expression statement.
- Need a way to know whether last expression had a semicolon.

How to represent semicolon intent:
- Minimal addition required somewhere (currently not represented):
  - e.g., add a termination flag on `Statement::Expression`, or equivalent parser metadata.

Touchpoints:
- Parser statement parsing (`src/syntax/parser/statement.rs`) to retain termination info.
- Bytecode/JIT block compilers to respect termination info.
- AST traversals if statement shape changes.

Pros:
- Lower churn than reworking `Block` shape everywhere.
- Aligns with current compiler/JIT “last expression statement yields value” model.

Cons:
- Without added metadata, semicolon-sensitive semantics are impossible.
- If metadata is attached to `Statement::Expression`, still touches many exhaustive matches.
- More implicit than Option 1.

Compatibility risks:
- Same semicolon behavior change risk if `{ x; }` stops yielding `x`.

If/lambda/match interaction:
- If/lambda easy to adapt.
- Match still limited by expression grammar and `{...}` hash conflict.

## Option 3: Introduce expression-only sequencing (`Expr::Let` or `Expr::Seq`)
Definition:
- Keep statement blocks unchanged.
- Add expression forms to encode let-in-expression directly, e.g.:
  - `Expr::Let(name, value, body)` or
  - `Expr::Seq(Vec<Statement>, tail_expr)`

Touchpoints:
- AST expression enum (`src/syntax/expression.rs`)
- Pratt parser (`src/syntax/parser/expression.rs`) for new expression forms
- Compiler/JIT lowering for new expression variants
- AST passes exhaustive matches

Pros:
- Solves let-in-expression directly, independent of block syntax.
- Avoids `{...}`/hash ambiguity for the core let-in-expression need.

Cons:
- Larger language-surface change.
- Heavier churn than Option 1/2 across parser + all consumers.
- Less aligned with “blocks yield value” mental model.

Compatibility risks:
- New syntax precedence/associativity interactions.
- Potential parser ambiguity with existing statements.

If/lambda/match interaction:
- Could enable expression bodies directly in lambda/match/if conditions if desired.
- But this is a broader design shift than Step C.

## 6) Recommendation (Minimal Churn for Current Architecture)

Recommended path: **Option 2 as an incremental step**, followed by optional Option 1 cleanup later.

Rationale:
- The current bytecode and JIT backends already implement block-value behavior via the final expression statement (`src/bytecode/compiler/statement.rs:471`, `src/jit/compiler.rs:2110`).
- This means minimal-churn evolution can reuse existing control-flow and stack logic.
- The immediate gap is representation of semicolon intent, not branch/lambda return mechanics.

Concrete touchpoints for the recommended path:
- Parser:
  - `src/syntax/parser/statement.rs:58` (capture whether `;` was consumed for expression statements)
  - `src/syntax/parser/helpers.rs:337` (block parse remains statement-list based)
- AST:
  - statement expression metadata addition (smallest possible)
- Bytecode:
  - `src/bytecode/compiler/statement.rs:462`, `src/bytecode/compiler/statement.rs:471`
- JIT:
  - `src/jit/compiler.rs:962`, `src/jit/compiler.rs:2110`
- AST analyses:
  - `src/ast/tail_position.rs:34` and peers for new termination rule

Compatibility notes:
- If Step C adopts `{ x; }` => `None` instead of `x`, this is a semantic break for some existing code.
- Consider a migration period with warning/lint before hard behavior switch.

Critical note for Step C goal wording:
- `let y = { let x = 1; x + 1 }` is not only a tail-value problem.
- It also needs expression-level block syntax disambiguation because `{...}` currently means hash-literal in expression prefix (`src/syntax/parser/expression.rs:119`).
- If that exact syntax is required, parser strategy for `{...}` must be addressed explicitly in a follow-up step.

## 7) Future Acceptance Tests (to add when implementing Step C)

1. `let y = { let x = 1; x + 1 }; y` evaluates to `2`.
2. `let y = { let x = 1; x + 1; }; y` evaluates to `None`/unit (if semicolon-sensitive design chosen).
3. `if true { let x = 1; x } else { 0 }` evaluates to `1`.
4. `if false { let x = 1; x } else { let z = 2; z + 1 }` evaluates to `3`.
5. `let f = \x -> { let y = x + 1; y * 2 }; f(3)` evaluates to `8`.
6. Nested blocks: `let y = { let a = { let b = 2; b + 1 }; a + 4 }; y` evaluates to `7`.
7. Block with no tail expression: `{ let x = 1; }` evaluates to `None`/unit.
8. Function body tail remains consistent: `fn f() { let x = 1; x + 2 } f()` evaluates to `3`.
9. `match` arm using block body (only if enabled in design): `match 1 { 1 -> { let x = 2; x }, _ -> 0 }` evaluates to `2`.
10. `match` arm semicolon behavior (if block arms + semicolon-sensitive): `match 1 { 1 -> { 2; }, _ -> 0 }` returns `None`/unit for first arm.
11. Regression: hash literal still parses unambiguously where intended.
12. Regression: parser diagnostics for missing delimiters stay bounded after Step C changes.

## Appendix: High-Impact Files Checklist
- Parser: `src/syntax/parser/helpers.rs`, `src/syntax/parser/statement.rs`, `src/syntax/parser/expression.rs`
- AST: `src/syntax/block.rs`, `src/syntax/statement.rs`, `src/syntax/expression.rs`
- AST passes: `src/ast/visit.rs`, `src/ast/fold.rs`, `src/ast/free_vars.rs`, `src/ast/tail_position.rs`, `src/ast/complexity.rs`
- Bytecode: `src/bytecode/compiler/statement.rs`, `src/bytecode/compiler/expression.rs`
- JIT: `src/jit/compiler.rs`
- Diagnostics/printing: `src/syntax/statement.rs`, `src/syntax/block.rs`, parser diagnostics in `src/syntax/parser/*`
