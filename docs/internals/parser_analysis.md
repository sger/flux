# Flux Parser Analysis (Phase 1, No Behavior Changes)

## 1) Parser Architecture Classification (with evidence)

### Classification
Flux currently uses a **hybrid parser**:
- **Top-level + statements + blocks**: hand-written **recursive descent**.
- **Expressions**: **Pratt/TDOP** style precedence parser.

### Evidence
- Program/statement descent entry:
  - `Parser::parse_program` loops over tokens and delegates to `parse_statement` (`src/syntax/parser/mod.rs:84`).
  - `Parser::parse_statement` dispatches by current token (`src/syntax/parser/statement.rs:9`).
- Pratt expression core:
  - `Parser::parse_expression(&mut self, precedence: Precedence)` (`src/syntax/parser/expression.rs:66`).
  - It parses a prefix expression first, then iterates while next operator precedence is higher, consuming infix/postfix (`src/syntax/parser/expression.rs:67`, `src/syntax/parser/expression.rs:69`, `src/syntax/parser/expression.rs:83`).
- Operator metadata is table-driven:
  - `OPERATOR_TABLE`, `infix_op`, `postfix_op`, `prefix_op`, `rhs_precedence_for_infix` (`src/syntax/precedence.rs:66`, `src/syntax/precedence.rs:245`, `src/syntax/precedence.rs:249`, `src/syntax/precedence.rs:254`, `src/syntax/precedence.rs:286`).

## 2) Call-Graph + Responsibilities of Key Parse Functions

### Top-level flow
1. `parse_program` (`src/syntax/parser/mod.rs:84`)
2. `parse_statement` (`src/syntax/parser/statement.rs:9`)
3. statement-specific parsers or expression statement
4. `parse_expression(Precedence::Lowest)` where needed

### Statement layer (recursive descent)
- `parse_statement`:
  - Handles `module/import/let/return/fn` statements and assignment statements.
  - Falls back to `parse_expression_statement` for everything else (`src/syntax/parser/statement.rs:10`).
- `parse_expression_statement`:
  - Parses one expression, optional `;` (`src/syntax/parser/statement.rs:58`).
- `parse_let_statement`:
  - Supports simple `let name = expr` and tuple destructuring `let (pattern) = expr` (`src/syntax/parser/statement.rs:136`).
- `parse_block`:
  - Parses `{ ... }` as `Vec<Statement>` only (`src/syntax/parser/helpers.rs:289`, `src/syntax/block.rs:7`).

### Expression layer (Pratt)
- `parse_expression` (`src/syntax/parser/expression.rs:66`)
  - Prefix phase: `parse_prefix` (`src/syntax/parser/expression.rs:89`).
  - Loop phase: consume postfix/infix based on precedence (`src/syntax/parser/expression.rs:69`).
- `parse_prefix` handles literals/keywords/special forms (`src/syntax/parser/expression.rs:90`):
  - `if`, `match`, `fn`, lambda (`\`), grouped/tuple, list/array/hash, option/either constructors, unary prefix ops.
  - Unknown prefix => `no_prefix_parse_error` (E031) (`src/syntax/parser/helpers.rs:610`).
- `parse_infix` (`src/syntax/parser/expression.rs:131`)
  - Postfix special forms: call/index/member (`(`, `[`, `.`).
  - Pipe operator special transform (`|>`).
  - Generic binary via `parse_infix_expression`.

### Complex forms
- `parse_if_expression` (`src/syntax/parser/expression.rs:570`)
  - Condition via `parse_expression`.
  - Branches via `parse_block` only.
- `parse_match_expression` (`src/syntax/parser/expression.rs:600`)
  - Scrutinee via `parse_expression`.
  - Arm pattern via `parse_pattern`; optional guard via `parse_expression`; arm body via `parse_expression`.
- `parse_lambda` (`src/syntax/parser/expression.rs:864`)
  - Parameters parsed from `\x` or `\(...)`.
  - Body either block (`{...}`) or single expression wrapped into a one-statement `Block`.

## 3) Grammar “Shape” Notes (blocks / if / match / lambda / let)

### Blocks
- A `Block` stores `statements: Vec<Statement>`; no dedicated tail-expression field (`src/syntax/block.rs:7`).
- Parser consumes repeated statements until `}` or EOF (`src/syntax/parser/helpers.rs:294`).

### `if`
- Shape: `if <expr> { <statements> } [else { <statements> }]`.
- `else if` is not a dedicated parser path; `else` requires `{` directly (`src/syntax/parser/expression.rs:581`, `src/syntax/parser/expression.rs:584`).

### `match`
- Shape: `match <expr> { <pattern> [if <expr>] -> <expr> (, ... ) }`.
- Arm body is expression-only (not statement list) (`src/syntax/parser/expression.rs:628`).

### Lambda
- Shape: `\x -> <expr>` or `\(...) -> <expr>` or block body `\x -> { ... }`.
- Block bodies are parsed as statement blocks.
- Expression body is wrapped into `Block { statements: [Statement::Expression] }` (`src/syntax/parser/expression.rs:918`).

### `let`
- `let` is parsed only at statement level (`parse_statement -> parse_let_statement`) (`src/syntax/parser/statement.rs:13`, `src/syntax/parser/statement.rs:136`).
- `let` is **not** a prefix expression token in `parse_prefix`; in expression position it falls to E031 (`src/syntax/parser/expression.rs:90`, `src/syntax/parser/helpers.rs:610`).

## 4) Error Handling + Recovery Behavior (current)

### Primary parser diagnostics used
- `E031 EXPECTED_EXPRESSION` via `no_prefix_parse_error` (`src/syntax/parser/helpers.rs:610`, `src/diagnostics/compiler_errors.rs:259`).
- `E034 UNEXPECTED_TOKEN` via `peek_error` and custom unexpected-token messages (`src/syntax/parser/helpers.rs:654`, `src/diagnostics/compiler_errors.rs:283`).
- `E073 MISSING_COMMA` for adjacent-expression list heuristics (`src/syntax/parser/helpers.rs:409`, `src/diagnostics/compiler_errors.rs:635`).
- `E076 UNCLOSED_DELIMITER` for EOF-inside-block (`src/syntax/parser/helpers.rs:301`, `src/diagnostics/compiler_errors.rs:651`).
- `E036` lambda syntax errors (`src/syntax/parser/expression.rs:876`, `src/diagnostics/compiler_errors.rs:301`).

### Recovery model
- **Panic-mode synchronization** with context-specific boundaries:
  - `synchronize(SyncMode::Expr|Stmt|Block)` (`src/syntax/parser/helpers.rs:161`).
- Statement-level fallback:
  - If `parse_statement` returns `None`, parser syncs in `Stmt` mode (`src/syntax/parser/statement.rs:51`).
- List-specific recovery:
  - `parse_expression_list_core` has missing-comma detection + delimiter recovery + bounded error count (`src/syntax/parser/helpers.rs:337`).
  - Recovery helpers: `recover_expression_list_to_delimiter`, `sync_to_list_end` (`src/syntax/parser/helpers.rs:537`, `src/syntax/parser/helpers.rs:581`).

### Why cascades happen
- A single delimiter error can leave parser state at a token that is syntactically valid in another context; later parse attempts then emit generic E031/E034 far from root cause.
- This is visible when a missing close delimiter causes later tokens to be parsed as continuation of expression lists or nested expressions before sync boundaries are reached.

## 5) Repro Cases for Known Symptoms

### Symptom 1: E031 when `let` appears in expression positions
Likely reproductions (given current grammar):

```flux
let x = if true { let y = 1; y } else { 0 }
```

```flux
let f = \x -> let y = x + 1
```

```flux
match 1 { 1 -> let y = 2, _ -> 0 }
```

Why: `let` is statement-only and not accepted by `parse_prefix`; expression slots call `parse_expression`, which emits E031 on `let`.
- Relevant: `src/syntax/parser/statement.rs:13`, `src/syntax/parser/expression.rs:89`, `src/syntax/parser/helpers.rs:610`.

### Symptom 2: Cascading errors after one missing delimiter
Repros:

```flux
print(point.
print(point.1)
```

```flux
let point = (1, 2, 3
let single = (42,)
```

```flux
print((1 + 2
let after = 3
```

Why: missing close delimiter/member tail can shift parser context; recovery may resume at statement boundaries later, producing secondary diagnostics.
- Relevant: `parse_grouped_expression`, `parse_member_access`, `parse_expression_list_core`, `synchronize`.
  - `src/syntax/parser/expression.rs:274`
  - `src/syntax/parser/expression.rs:365`
  - `src/syntax/parser/helpers.rs:337`
  - `src/syntax/parser/helpers.rs:161`

### Symptom 3: Parser “shape sensitivity”
(Equivalent intent succeeds/fails depending on whether syntax is statement-block or expression form.)

```flux
let f = \x -> x + 1        // works (expression body)
let g = \x -> { let y = x; y }  // works (statement block body)
let h = \x -> let y = x     // fails (let in expression slot)
```

```flux
if cond { let y = 1; y } else { 0 }  // block statement style
if cond { let y = 1 } else { 0 }      // no explicit tail expression concept in Block AST
```

Why: parser API is split by context (`parse_statement` vs `parse_expression`), and blocks are statement lists without dedicated tail-expression representation.
- Relevant: `src/syntax/parser/helpers.rs:289`, `src/syntax/block.rs:7`, `src/syntax/parser/expression.rs:864`, `src/syntax/parser/statement.rs:9`.

## Appendix: Entry Points Summary
- Main parser entry: `Parser::parse_program` (`src/syntax/parser/mod.rs:84`).
- Alternate module-graph parse entry wrappers:
  - `parse_program(path)` (`src/syntax/module_graph/module_resolution.rs:22`)
  - `parse_program_with_interner(path, interner)` (`src/syntax/module_graph/module_resolution.rs:49`).
