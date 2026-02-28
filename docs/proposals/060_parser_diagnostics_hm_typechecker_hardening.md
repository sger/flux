# Proposal 060: Parser Recovery Breadth, Diagnostic Precision, HM Hardening, and Type Checker Completeness

**Status:** Draft
**Date:** 2026-02-28
**Depends on:** `059_parser_error_experience.md` (contextual parser diagnostic patterns), `058_contextual_diagnostics_callsite_let_return.md` (dual-label infrastructure), `051_any_fallback_reduction.md` (Any fallback policy), `050_totality_and_exhaustiveness_hardening.md` (exhaustiveness contract), `047_adt_semantics_deepening.md` (ADT constructor typing)

---

## 1. Summary

This proposal is a focused hardening pass across four layers of the Flux compiler front-end, building directly on the infrastructure established in proposals 057–059. It does not introduce new language features or new AST nodes. Every item is a diagnostic improvement, a soundness tightening, or a recovery-breadth extension.

The work is organized into four tracks that can be executed independently:

| Track | Label | Theme |
|---|---|---|
| **P** | Parser | Broader contextual recovery and named-construct messages for remaining `expect_peek` gaps |
| **D** | Diagnostics | Span precision, cascade deduplication, machine-readable output |
| **H** | HM Inference | Any-fallback reduction at high-value sites, tuple projection, match scrutinee constraints |
| **T** | Type Checker | Bool exhaustiveness, guarded wildcard contract, ADT pattern arity, cross-module boundary |

---

## 2. Motivation

After 059 the parser produces targeted messages for keyword aliases, missing braces in `if`/`else`, missing `=` in `let`, missing `()` in `fn`, and `|`/`=>` in match arms. However approximately 35 `expect_peek` call sites in `expression.rs` and `statement.rs` still emit generic messages. Similarly, the HM engine contains ~35 `Any` fallback sites; several of them in high-visibility positions (branch disagreement, tuple projection, match scrutinee) are both easy to tighten and high-value for user-facing correctness.

The type checker's exhaustiveness coverage is complete for ADT constructor spaces and partially correct for lists, but Bool, guarded wildcards, and ADT constructor arity in patterns have known gaps documented in proposal 050.

---

## 3. Non-Goals

1. No new syntax or AST nodes.
2. No changes to the runtime value system or GC.
3. No changes to JIT compilation paths except where diagnostics feed through shared compiler infrastructure.
4. No higher-rank polymorphism or theorem-proving exhaustiveness.
5. No record-pattern totality (blocked on proposal 048).
6. No new effect checking rules (owned by 042/049).

---

## 4. Track P — Parser Recovery Breadth

### P1: Named-construct messages for remaining unclosed delimiter sites

**Problem.** After 059, the following `expect_peek` failure sites still produce the generic `"Expected X, got Y"` message with no construct name and no correct-syntax hint:

| Site | File | Current message |
|---|---|---|
| Hash literal missing `}` | `expression.rs:834` | `Expected }, got <token>` |
| Array literal `[| |]` missing `]` | `expression.rs:525` | `Expected ], got <token>` |
| Bar-array missing `|]` close | `expression.rs:547` | `Expected ], got <token>` |
| Lambda `\(x, y)` missing `)` | `expression.rs:338` | `Expected ), got <token>` |
| String interpolation missing `}` | `literal.rs` | generic unclosed-delimiter |
| List comprehension missing `]` | `expression.rs:691` | `Expected ], got <token>` |

**Proposed diagnostic constructors** (all reuse E034 `UNEXPECTED_TOKEN`):

```
missing_hash_close_brace(span) ->
  "Expected `}` to close hash literal `{ key: value, ... }`"
  Hint: "Add a closing `}` after the last key-value pair."

missing_array_close_bracket(span) ->
  "Expected `]` to close array literal `[| ... |]`"
  Hint: "Arrays use `[| ... |]` delimiters."

missing_lambda_close_paren(span) ->
  "Expected `)` to close lambda parameter list `\\(x, y)`"
  Hint: "Multi-parameter lambdas use `\\(a, b) -> body`."

missing_string_interpolation_close(span) ->
  "Expected `}` to close string interpolation `\\{expr}`"
  Hint: "String interpolation uses `\\{expression}`."

missing_comprehension_close_bracket(span) ->
  "Expected `]` to close list comprehension `[expr | var <- coll]`"
  Hint: "List comprehensions use `[expr | var <- collection, guard]`."
```

**Implementation.**
- Add five constructors to `src/diagnostics/compiler_errors.rs` (all delegating to `unexpected_token(...).with_hint_text(...)`).
- Replace the corresponding `expect_peek` calls in `expression.rs` and `literal.rs` with the new constructors.
- Add one fixture per case in `examples/type_system/failing/` (122–126).
- Add unit assertions in `tests/error_codes_registry_tests.rs`.

**Recovery.** No change to recovery behavior; the existing delimiter-recovery paths (`recover_to_matching_delimiter`) remain.

---

### P2: Contextual `->` arrow errors in match arms and lambdas

**Problem.** When `->` is missing in a match arm (not the `=>` case already handled by 059), the parser returns `None` with the generic message "Expected `->`, got `<token>`". The message does not name the enclosing construct or show the expected form.

Example:
```flux
match x { 0 1, _ 2 }
```
Current:
```
error[E034]: UNEXPECTED TOKEN
Expected `->`, got `1`.

1 | match x { 0 1, _ 2 }
  |             ^
```
Proposed:
```
error[E034]: UNEXPECTED TOKEN
Expected `->` after match arm pattern, found `1`.

1 | match x { 0 1, _ 2 }
  |             ^

Help: Match arms use `->`: `match x { pattern -> body, ... }`
```

Same gap exists in lambdas: `\x 1` (missing `->`) emits the generic form.

**Proposed constructors:**

```
missing_match_arrow(span, found_token: &str) ->
  "Expected `->` after match arm pattern, found `{found_token}`."
  Hint: "Match arms use `->`: `match x { pattern -> body, ... }`"

missing_lambda_arrow(span, found_token: &str) ->
  "Expected `->` in lambda body, found `{found_token}`."
  Hint: "Lambdas use `->`: `\\x -> body` or `\\(a, b) -> body`"
```

**Implementation.**
- Two new constructors in `compiler_errors.rs`.
- In `expression.rs`, the `parse_match_arm` path at the `expect_peek(Arrow)` call and the lambda parse path: inspect `self.current_token` on failure and emit the named constructor.
- Fixtures 127–128 in `examples/type_system/failing/`.

---

### P3: Orphan pattern diagnostic at statement level

**Problem.** A match constructor used at the top level (not inside a `match`) reads as an identifier call expression, and the subsequent content produces a cascade. Example:

```flux
Some(x)
```
Current produces a juxtaposed-identifier or unexpected-token error with no mention of `match`. Proposed:

```
error[E034]: UNEXPECTED TOKEN
`Some(...)` looks like a match pattern but appears outside a `match` expression.

1 | Some(x)
  | ^^^^

Help: Did you mean `match val { Some(x) -> ... }`?
```

**Scope.** Only fire when the token is one of the built-in constructor keywords (`Some`, `None`, `Left`, `Right`) and the following token is `(` or the statement ends immediately. Do not attempt this for user-defined constructors (indistinguishable from function calls at parse time).

**Implementation.**
- New constructor `orphan_constructor_pattern(span, name: &str)` in `compiler_errors.rs`.
- In `parse_expression_statement`, check for `Token::Some | Token::None | Token::Left | Token::Right` followed by `(` and emit the diagnostic before returning `None`.
- Fixture 129.

---

### P4: `do` block missing braces

**Problem.** `do expr` (missing `{`) silently falls through to a generic error.

Proposed:
```
error[E034]: UNEXPECTED TOKEN
Expected `{` to begin `do` block.

1 | do 1 + 2
  |    ^

Help: `do` blocks require braces: `do { expr1; expr2 }`
```

**Implementation.**
- New constructor `missing_do_block_brace(span)` in `compiler_errors.rs`.
- In `parse_do_expression` in `expression.rs`, replace `expect_peek(LBrace)` with an explicit check that emits the named message.
- Fixture 130.

---

## 5. Track D — Diagnostic Precision

### D1: Span precision for multi-token construct labels

**Problem.** Several diagnostics in `hm_expr_typer.rs` and `type_infer.rs` attach the error label to the whole enclosing expression rather than the specific subterm that caused the mismatch. Examples:

- `foo(42)` where `42` is wrong: label covers `foo(42)` instead of `42`.
- `let x: Int = foo(y)` where `y` is wrong: label covers `foo(y)` instead of `y`.
- `if a { b } else { c }` branch mismatch: label covers the `else { c }` block rather than the return expression `c` alone.

**Goal.** For each E300 diagnostic produced by `infer_call`, `infer_if`, and `infer_let`, narrow the primary span to the smallest subterm that is concretely wrong. Secondary label points to the annotation or definition as today.

**Approach.**
- `infer_call`: when `UnifyErrorDetail::FunParamMismatch` fires on argument `i`, use `args[i].span()` as the primary label rather than the whole call expression span.
- `infer_if`: when branch disagreement fires, use the span of the return expression of the mismatching branch rather than the whole block span. (The `Block` node's last expression carries a span.)
- `infer_let`: when a let initializer mismatches, use the initializer expression span rather than the whole `let` statement span.

**Files.** `src/ast/type_infer.rs`, `src/bytecode/compiler/hm_expr_typer.rs`.

**Evidence gates.**
- Existing fixtures 92–121 must not regress (snapshot review after change).
- New fixtures 131–133: `let`, `call`, `if` variants with narrow span assertions in the rendered output.

---

### D2: Cascade E300 deduplication

**Problem.** When a single type error propagates through a pipeline (`a |> f |> g |> h`), each call site that cannot unify emits its own E300. The user sees 3–4 E300s for what is one root cause.

**Proposed policy.** In the diagnostic aggregator (`src/diagnostics/aggregator.rs`): when two E300 diagnostics share the same source file and their primary label spans overlap or are on adjacent lines (±2), suppress the later one and append a note to the first: `"(+N related type error(s) suppressed — fix this first)"`.

**Non-goals.** Do not suppress diagnostics from different source locations. Do not change E300 message text.

**Implementation.**
- In `aggregator.rs`, after deduplication and sorting, add a post-pass that groups overlapping-span E300s and collapses them.
- Gate: only apply when ≥2 E300s share the same file and overlap predicate.
- Add `tests/diagnostics_aggregator_tests.rs` case covering a 3-error cascade.

---

### D3: Machine-readable diagnostic output (`--format json`)

**Motivation.** Editor integrations (LSP, CI tooling) need structured output. Currently the only output is the ANSI-rendered text.

**Proposed flag.** `--format json` (alongside existing implicit `--format text` default). When set, `render_diagnostics()` emits a JSON array instead of ANSI text:

```json
[
  {
    "code": "E300",
    "severity": "error",
    "message": "Type mismatch: expected Int, found String",
    "file": "src/main.flx",
    "line": 5,
    "column": 12,
    "end_line": 5,
    "end_column": 19,
    "hints": ["Did you mean to convert with `to_string`?"]
  }
]
```

**Implementation.**
- Add `--format <fmt>` flag in `src/main.rs` accepting `text` (default) and `json`.
- Add `render_diagnostics_json(diagnostics, sources) -> String` in `src/diagnostics/rendering/formatter.rs` (or a new `rendering/json.rs`).
- The JSON serialization should not require `serde` — a hand-written formatter is sufficient given the flat structure.
- No change to existing text rendering path.
- Integration test in `tests/diagnostic_render_tests.rs`: parse a known-bad file, assert JSON output is valid and contains expected fields.

---

## 6. Track H — HM Inference Hardening

### H1: `if`/`else` branch disagreement — concrete E300 instead of `Any`

**Problem.** In `type_infer.rs:237–248`, when both branches of an `if` expression produce concrete types that do not unify, `join_types()` silently returns `Any`. This means:

```flux
if true { 1 } else { "a" }
```
Infers as `Any` with no diagnostic, and the mismatch is only caught if the result is used in a typed context.

**Proposed behavior.** When `join_types(a, b)` is called with two concrete types (`a` and `b` are not `Any`, not `Var`) that do not unify, emit E300 immediately with dual labels pointing at the then-body expression and the else-body expression, matching the dual-label format established in proposal 057.

```
error[E300]: TYPE MISMATCH
The branches of this `if` expression produce different types.

2 | if true { 1 } else { "a" }
  |           ^         ^^^
  |           |         this is String
  |           this is Int

Help: Both branches must produce the same type.
```

**Implementation.**
- `join_types` in `type_infer.rs` receives spans for both branch expressions (pass as parameters from `infer_if`).
- When both are concrete and disagree, push E300 via `ReportContext` and return `Any` (gradual — do not abort inference).
- Must not fire when either side is `Any` or `Var` (intentional gradual escape).
- Fixtures 134–135: `if`-disagreement fires E300; `if`-with-`Any`-branch does not.

---

### H2: Tuple projection typing

**Problem.** `tuple.0` / `tuple.1` on a known `Tuple([Int, String])` shape returns `Any` in `hm_expr_typer.rs`. This means:

```flux
let t: (Int, String) = (1, "a")
let x: String = t.0   -- should be E300 (Int, not String)
```
Currently passes without a diagnostic.

**Proposed behavior.** In `hm_expr_type_strict_path` (and in `type_infer.rs`'s tuple field access path), when the base expression's inferred type is `Tuple([t0, t1, ...])` and the field index is a literal integer, return `t_i` rather than `Any`.

**Implementation.**
- In `type_infer.rs`, in the `Expression::TupleFieldAccess` match arm (currently returning `Any`), resolve `base_ty` via substitution; if it is `InferType::App(Tuple, args)` or `InferType::Tuple(args)`, extract `args[index]`.
- Update `hm_expr_typer.rs`'s `hm_expr_type_strict_path` similarly.
- Fixtures 136–137: `t.0` typed correctly; `t.1` typed correctly; mismatch produces E300.

---

### H3: Match scrutinee ↔ arm pattern constraint propagation

**Problem.** HM currently infers arm bodies but does not propagate pattern shape constraints back to the scrutinee's type. Example:

```flux
fn f(x) {
    match x {
        Some(n) -> n + 1,
        None    -> 0
    }
}
```
`x` remains `Any` in the env after inference because the `Some(n)` pattern does not constrain `x: Option<Int>`. Downstream uses of `x` lose the inferred type.

**Proposed behavior.** When all arms share a consistent constructor family (e.g. `Some`/`None` → `Option<T>`), unify the scrutinee type with the inferred constructor type and update the env. Do not attempt this when arms are heterogeneous or use wildcard-only patterns (keep as `Any` for gradual).

**Implementation.**
- In `infer_match` in `type_infer.rs`, after processing all arms, collect the constructor types used in patterns. If they all belong to a single `TypeConstructor` family (Option, Either, or a user ADT), call `unify(scrutinee_ty, inferred_family_ty)` and apply the resulting substitution to the env.
- Must not regress existing wildcard-only match tests.
- Fixtures 138–139: scrutinee constrained by `Some`/`None` arms; subsequent use of `x` reflects `Option<Int>`.

---

### H4: Recursive self-reference type propagation

**Problem.** In an unannotated recursive function, the self-reference call site falls back to `Any` before the body type is known. Example:

```flux
fn sum(xs) {
    match xs {
        [h | t] -> h + sum(t),   -- sum(t) infers as Any
        _       -> 0
    }
}
```
The result type of `sum` infers as `Any` even though `h + sum(t)` should constrain both to `Int`.

**Proposed behavior.** Use a two-step fixpoint for recursive functions: first infer with a fresh type variable for the self-reference, then unify the inferred return type with that variable. This is the standard Algorithm W treatment of `let rec`.

**Implementation.**
- In `infer_function` in `type_infer.rs`, when the function name is in scope as a `Var`, run a second unification pass after the body is inferred (already partially done for annotated functions; extend to unannotated ones).
- This is a subset of `051_any_fallback_reduction.md` scoped to the recursive self-reference site.
- Fixtures 140–141: recursive `sum` infers `Int`; recursive `map` infers `Array<B>` given body constraints.

---

## 7. Track T — Type Checker Completeness

### T1: Exhaustiveness over `Bool`

**Problem.** `match b { true -> 1 }` is non-exhaustive (missing `false`) but does not emit E015.

**Proposed behavior.** When the match scrutinee's inferred type is `Bool`, require both `true` and `false` arms (or a wildcard). If either is missing, emit E015 with a suggestion showing the missing arm.

```
error[E015]: NON-EXHAUSTIVE MATCH
This match on a Bool value is missing the `false` arm.

1 | match b { true -> 1 }
  | ^^^^^^^^^^^^^^^^^^^^^

Help: Add a `false -> ...` arm or a wildcard `_ -> ...`.
```

**Implementation.**
- In the match exhaustiveness checker (in `bytecode/compiler/mod.rs` or wherever ADT exhaustiveness lives), add a `Bool` domain case: collect seen `true`/`false` literals; flag if either is missing and no wildcard is present.
- Fixtures 142–143: missing `false` → E015; missing `true` → E015; both present → no error; wildcard → no error.

---

### T2: Guarded wildcard exhaustiveness contract

**Problem.** `match x { _ if pred(x) -> 1 }` is treated as non-exhaustive because the guard can fail at runtime. The current diagnostic is the generic E015 "non-exhaustive match" message.  The user may not understand why a wildcard pattern was rejected.

**Proposed behavior.** Detect the guarded-wildcard case specifically and emit a targeted message:

```
error[E015]: NON-EXHAUSTIVE MATCH
A guarded wildcard `_ if ...` does not guarantee exhaustiveness — the guard may fail.

1 | match x { _ if pred(x) -> 1 }
  |           ^^^^^^^^^^^^^^^^^^^

Help: Add a bare `_ -> ...` arm to handle the case when the guard fails.
```

**Implementation.**
- In the exhaustiveness checker, when the only remaining arm is a guarded wildcard, emit this targeted message instead of the generic one.
- New diagnostic constructor `guarded_wildcard_non_exhaustive(span)` in `compiler_errors.rs`.
- Fixtures 144–145: guarded wildcard alone → targeted E015; guarded wildcard + bare wildcard → no error.

---

### T3: ADT constructor pattern arity mismatch

**Problem.** `match opt { Some(a, b) -> 0, None -> 1 }` — `Some` takes exactly 1 argument, but the pattern destructs 2. Currently the compiler either silently ignores the extra binding or produces a confusing type error downstream.

**Proposed behavior.** At the pattern-binding site, when a constructor pattern provides the wrong number of sub-patterns, emit a dedicated error distinct from E083 (non-exhaustive match):

```
error[E085]: CONSTRUCTOR PATTERN ARITY MISMATCH
Constructor `Some` expects 1 field but this pattern has 2.

1 | match opt { Some(a, b) -> 0, None -> 1 }
  |             ^^^^^^^^^^

Help: `Some` wraps a single value: `Some(x)`.
```

**Implementation.**
- New error code `E085` in `compiler_errors.rs`.
- New diagnostic constructor `constructor_pattern_arity_mismatch(span, name: &str, expected: usize, found: usize)`.
- In the pattern-matching compilation path (likely `bytecode/compiler/mod.rs` or wherever constructor patterns are lowered), check arity before emitting bind opcodes.
- Fixtures 146–148: `Some(a, b)` → E085; `None(x)` → E085; `Left(a, b, c)` → E085.

---

### T4: Cross-module ADT constructor boundary diagnostic

**Problem.** The CLAUDE.md boundary policy states: "Cross-module ADT usage should go through module `public fn` factories/accessors. Direct module-qualified constructor calls are not part of the stable public boundary."  Currently, when code outside a module directly constructs or matches on an ADT constructor that belongs to another module's internal type, no diagnostic is emitted.

**Proposed behavior.** When in `--strict` mode, detect direct cross-module constructor use and emit E086:

```
error[E086]: CROSS-MODULE CONSTRUCTOR ACCESS
Constructor `Tree.Node` is an internal constructor of module `Tree`.
Access it through the module's public API instead.

3 | let t = Tree.Node(1, Tree.Leaf)
  |         ^^^^^^^^^

Help: Use the factory function provided by module `Tree`.
```

**Non-strict mode.** Emit as a warning (`W200` class) rather than an error, to avoid breaking existing code.

**Implementation.**
- New error code `E086` in `compiler_errors.rs`.
- New warning code `W201` for non-strict mode.
- In PASS 2 (`expression.rs`), when resolving a qualified constructor call (`Module.Constructor(...)`), check whether the constructor is declared `public` in the module's symbol table. If not public and not in the current module, emit E086 (strict) or W201 (default).
- Fixtures 149–150: cross-module constructor → E086 in strict; cross-module constructor → W201 in non-strict.

---

## 8. Fixture Matrix

| Fixture | Track | Description |
|---|---|---|
| 122 | P1 | Hash literal missing `}` |
| 123 | P1 | Array literal `[| |]` missing `]` |
| 124 | P1 | Lambda `\(x, y)` missing `)` |
| 125 | P1 | String interpolation missing `}` |
| 126 | P1 | List comprehension missing `]` |
| 127 | P2 | Match arm missing `->` |
| 128 | P2 | Lambda missing `->` |
| 129 | P3 | Orphan constructor pattern at statement level |
| 130 | P4 | `do` block missing `{` |
| 131 | D1 | Narrow span: call-site argument |
| 132 | D1 | Narrow span: `let` initializer |
| 133 | D1 | Narrow span: `if` branch return expression |
| 134 | H1 | `if`/`else` concrete-type disagreement → E300 |
| 135 | H1 | `if`/`else` with `Any` branch → no E300 |
| 136 | H2 | Tuple `.0` typed correctly from known shape |
| 137 | H2 | Tuple `.1` mismatch → E300 |
| 138 | H3 | Match scrutinee constrained by `Some`/`None` arms |
| 139 | H3 | Subsequent use of scrutinee reflects `Option<Int>` |
| 140 | H4 | Recursive `sum` infers `Int` without annotation |
| 141 | H4 | Recursive `map` infers element type from body |
| 142 | T1 | `match b { true -> 1 }` → E015 (missing false) |
| 143 | T1 | `match b { false -> 0 }` → E015 (missing true) |
| 144 | T2 | Guarded wildcard alone → targeted E015 |
| 145 | T2 | Guarded wildcard + bare wildcard → no error |
| 146 | T3 | `Some(a, b)` → E085 |
| 147 | T3 | `None(x)` → E085 |
| 148 | T3 | `Left(a, b, c)` → E085 |
| 149 | T4 | Cross-module constructor → E086 (strict) |
| 150 | T4 | Cross-module constructor → W201 (non-strict) |

---

## 9. New Error Codes

| Code | Title | Track | Notes |
|---|---|---|---|
| `E085` | CONSTRUCTOR PATTERN ARITY MISMATCH | T3 | Distinct from E083 (non-exhaustive) |
| `E086` | CROSS-MODULE CONSTRUCTOR ACCESS | T4 | Strict mode only |
| `W201` | CROSS-MODULE CONSTRUCTOR ACCESS | T4 | Warning in non-strict mode |

All other diagnostics reuse existing codes (`E034`, `E015`, `E300`).

---

## 10. Implementation Sequencing

Tracks are independent. Recommended order within the 0.0.4 gate:

| Priority | Item | Rationale |
|---|---|---|
| 1 | H1 (branch disagreement → E300) | Highest user-facing impact; unblocks clean if-branch error stories |
| 2 | H2 (tuple projection) | Low effort, high signal; enables downstream E300 for typed tuple use |
| 3 | T1 (Bool exhaustiveness) | Week-3 gate in proposal 054 |
| 4 | T2 (guarded wildcard) | Week-3 gate in proposal 054; small scope |
| 5 | T3 (ADT pattern arity) | Correctness gap; targeted new code |
| 6 | P1 (unclosed delimiters) | Polish; independent of type system |
| 7 | P2 (arrow errors) | Polish; independent |
| 8 | H3 (scrutinee constraints) | Medium effort; improves downstream inference quality |
| 9 | H4 (recursive self-reference) | Highest HM complexity; do after H1–H3 stable |
| 10 | D1 (span precision) | Requires snapshot review; do after H items stabilize spans |
| 11 | D2 (cascade deduplication) | Aggregator change; low risk, medium payoff |
| 12 | T4 (cross-module boundary) | Depends on module symbol-table public-flag plumbing |
| 13 | D3 (JSON output) | Tooling; independent; lowest urgency |
| 14 | P3, P4 | Minor parser polish; anytime |

---

## 11. Gates and Regression Policy

- All 369 existing tests must pass before and after each item.
- Snapshot tests must be reviewed with `cargo insta review` after any change to `type_infer.rs`, `hm_expr_typer.rs`, or `aggregator.rs`.
- VM/JIT parity check required for T1–T4 (exhaustiveness and boundary changes affect both backends).
- Each item ships with at minimum one passing and one failing fixture in `examples/type_system/`.
- The `E085`/`E086`/`W201` codes must be registered in `registry.rs` before their constructors are used.

## 12. Execution Plan (Task Breakdown)

### T0 — Baseline & Guardrails (No behavior change)
- **Goal:** Freeze baseline behavior before 060 changes.
- **Files:** `docs/proposals/060_parser_diagnostics_hm_typechecker_hardening.md`.
- **Changes:** Lock constraints:
  - no syntax/AST/runtime feature additions
  - diagnostics-first hardening only
  - one-task-per-PR
  - intentional snapshot policy
  - unexpected snapshot churn blocks task merge until triaged
- **Tests:**
  - `cargo check --all --all-features`
  - `cargo test --test parser_tests`
  - `cargo test --test parser_recovery`
  - `cargo test --test type_inference_tests`
  - `cargo test --test compiler_rules_tests`
  - `cargo test --test snapshot_parser`
  - `cargo test --test snapshot_diagnostics`
  - `cargo test --all --all-features purity_vm_jit_parity_snapshots`
- **Fixtures:** None.
- **Risk:** Low.
- **Done When:** Baseline and guardrails are documented with command outcomes.

#### T0 Evidence (Baseline Run)
- **Baseline Timestamp (UTC):** `20260228T083951Z`
- **Git SHA:** `a2998e067261bea19f551282aa0c664f9d9204db`
- **Branch:** `feature/improve-parser`
- **Artifact Path(s):**
  - `perf_logs/060_t0_baseline_20260228T083951Z/context.txt`
  - `perf_logs/060_t0_baseline_20260228T083951Z/commands.list`
  - `perf_logs/060_t0_baseline_20260228T083951Z/commands.log`
  - `perf_logs/060_t0_baseline_20260228T083951Z/results.tsv`
  - `perf_logs/060_t0_baseline_20260228T083951Z/cmd_*.log`
- **Results:**
  - `cargo check --all --all-features` -> PASS
  - `cargo test --test parser_tests` -> PASS
  - `cargo test --test parser_recovery` -> PASS
  - `cargo test --test type_inference_tests` -> PASS
  - `cargo test --test compiler_rules_tests` -> PASS
  - `cargo test --test snapshot_parser` -> PASS
  - `cargo test --test snapshot_diagnostics` -> PASS
  - `cargo test --all --all-features purity_vm_jit_parity_snapshots` -> PASS
- **Snapshot Status:** No snapshot updates were required in T0.
- **Parity Status:** `purity_vm_jit_parity_snapshots` passed.
- **Notes/Flakes:** None observed in this run.

### T1 — P1 Named Delimiter Diagnostics
- **Goal:** Replace remaining generic delimiter errors with named construct diagnostics.
- **Files:**
  - `src/diagnostics/compiler_errors.rs`
  - `src/syntax/parser/expression.rs`
  - `src/syntax/parser/literal.rs`
- **Changes:** Add/use constructors for hash/array/lambda/interpolation/comprehension closing delimiters.
- **Tests:**
  - `tests/error_codes_registry_tests.rs` constructor shape checks
  - `tests/parser_tests.rs` message assertions
  - `tests/parser_recovery.rs` continuation checks
- **Fixtures:**
  - `122` to `126` as listed in proposal.
- **Risk:** Low.
- **Done When:** All 5 cases emit named E034 with hints and stable recovery.

### T2 — P2 Arrow Diagnostics in Match/Lambda
- **Goal:** Emit contextual missing-arrow diagnostics for match arms and lambdas.
- **Files:**
  - `src/diagnostics/compiler_errors.rs`
  - `src/syntax/parser/expression.rs`
- **Changes:**
  - add/use `missing_match_arrow`, `missing_lambda_arrow`
  - preserve `=>` path from 059
- **Tests:**
  - parser message assertions
  - recovery continuation
- **Fixtures:**
  - `127`, `128`
- **Risk:** Low.
- **Done When:** Missing `->` paths use contextual E034 instead of generic token errors.

### T3 — P3 Orphan Constructor Pattern Statement Diagnostic
- **Goal:** Detect built-in constructor-like patterns outside `match`.
- **Files:**
  - `src/diagnostics/compiler_errors.rs`
  - `src/syntax/parser/statement.rs`
- **Changes:**
  - add/use `orphan_constructor_pattern`
  - scope only to `Some/None/Left/Right` with safe token-shape checks
- **Tests:**
  - positive parser diagnostic
  - negative non-misfire tests for valid function/member calls
- **Fixtures:**
  - `129`
- **Risk:** Medium (false-positive risk).
- **Done When:** Targeted diagnostic appears only in intended statement contexts.

### T4 — P4 `do` Block Missing Brace Diagnostic
- **Goal:** Emit contextual message for missing `do { ... }`.
- **Files:**
  - `src/diagnostics/compiler_errors.rs`
  - `src/syntax/parser/expression.rs`
- **Changes:** add/use `missing_do_block_brace`.
- **Tests:** parser message + recovery checks.
- **Fixtures:** `130`.
- **Risk:** Low.
- **Done When:** `do` missing-brace case emits clear E034 with form hint.

### T5 — D1 Span Precision Narrowing
- **Goal:** Narrow E300 primary spans to subterms (arg expr, let initializer, branch return expr).
- **Files:**
  - `src/ast/type_infer.rs`
  - `src/bytecode/compiler/hm_expr_typer.rs`
- **Changes:**
  - call-site arg primary span = `args[i].span()`
  - let mismatch primary span = initializer expr span
  - if mismatch primary span = mismatching return-expression span
- **Tests:**
  - `tests/type_inference_tests.rs`
  - `tests/compiler_rules_tests.rs`
  - rendered diagnostics assertions for span accuracy
- **Fixtures:**
  - `131`, `132`, `133`
- **Risk:** Medium (snapshot churn).
- **Done When:** Narrow spans are deterministic and existing contextual fixtures remain coherent.

### T6 — D2 E300 Cascade Dedup
- **Goal:** Suppress nearby duplicate E300 cascades while retaining root-cause visibility.
- **Files:**
  - `src/diagnostics/aggregator.rs`
  - `tests/diagnostics_aggregator_tests.rs`
- **Changes:**
  - overlap/adjacent-line grouping rule for E300
  - suppression-note annotation on retained diagnostic
- **Tests:**
  - new aggregator unit tests
  - no suppression across unrelated spans/files
- **Fixtures:** optional synthetic diagnostic fixture in tests only.
- **Risk:** Medium (over-suppression).
- **Done When:** Cascades collapse predictably without hiding independent errors.

### T7 — D3 JSON Diagnostic Output
- **Goal:** Add structured diagnostics output mode.
- **Files:**
  - `src/main.rs`
  - `src/diagnostics/rendering/*` (new json renderer if needed)
  - `tests/diagnostic_render_tests.rs`
- **Changes:**
  - `--format text|json`
  - JSON array output with stable fields
- **Tests:**
  - rendering tests for valid JSON and required fields
  - text mode unchanged
- **Fixtures:** reuse existing bad-input diagnostics cases.
- **Risk:** Medium (CLI compatibility).
- **Done When:** JSON mode works and text mode remains default behavior.

### T8 — H1 If/Else Concrete Disagreement E300
- **Goal:** Emit immediate contextual E300 for concrete branch disagreement.
- **Files:**
  - `src/ast/type_infer.rs`
- **Changes:**
  - in branch-join path, emit contextual mismatch when both sides concrete and non-Any
  - keep gradual suppression for Any/Var paths
- **Tests:**
  - positive disagreement
  - negative Any/Var suppression
- **Fixtures:**
  - `134`, `135`
- **Risk:** Medium (false positives if guards are shallow).
- **Done When:** Concrete disagreements report; gradual cases remain suppressed.

#### T8 Closure Note (Regression Lock)
- Core behavior is already implemented in `src/ast/type_infer.rs` via:
  - `ReportContext::IfBranch` routing in `unify_with_context(...)`
  - concrete-only + deep-`Any` suppression guard (`is_concrete()` and `!contains_any()`).
- T8 close-out adds:
  - fixture `134_if_concrete_branch_mismatch.flx` (positive contextual mismatch path),
  - fixture `135_if_any_branch_suppressed.flx` (nested-`Any` suppression path with an independent failing diagnostic),
  - compiler pipeline fixture assertions that lock both positive and negative behavior.
- Evidence commands:
  - `cargo test --test type_inference_tests`
  - `cargo test --test compiler_rules_tests`
  - `cargo test --test examples_fixtures_snapshots`
  - `cargo test --test snapshot_diagnostics`
- Gate policy:
  - `examples_fixtures_snapshots` is recorded but non-blocking for T8 when failure is attributable to unrelated snapshot churn from other tasks (currently E300 cascade-dedup snapshot drift).
  - Such failures must be explicitly logged under "Known External Churn" with snapshot path(s) and owning task.
- Validation/evidence recording for T8:
  - `type_inference_tests` -> required `PASS`
  - `compiler_rules_tests` -> required `PASS`
  - `snapshot_diagnostics` -> required `PASS`
  - `examples_fixtures_snapshots` -> informational; record as `PASS`, `FAIL (non-blocking, unrelated)`, or `FAIL (blocking)`
- Known External Churn:
  - `tests/snapshots/examples_fixtures/basics__array_hash_combo.snap.new` is out-of-scope for T8 and owned by E300 dedup/snapshot governance work.

### T9 — H2 Tuple Projection Typing
- **Goal:** Return precise projected type for known tuple shapes.
- **Files:**
  - `src/ast/type_infer.rs`
  - `src/bytecode/compiler/hm_expr_typer.rs`
- **Changes:**
  - `tuple.i` on known tuple returns `t_i` instead of Any
- **Tests:**
  - typed projection success/mismatch
  - unresolved tuple paths unchanged
- **Fixtures:**
  - `136`, `137`
- **Risk:** Medium.
- **Done When:** Projection participates in strict typed mismatch checking reliably.

#### T9 Closure Note (Regression Lock)
- Core behavior is already implemented in `src/ast/type_infer.rs`:
  - `Expression::TupleFieldAccess` resolves `tuple.i` to `t_i` for known tuple shapes.
  - unresolved/non-tuple sources keep existing fallback behavior.
- T9 close-out adds:
  - fixture `136_tuple_projection_precise_mismatch.flx` (known tuple projection mismatch path),
  - fixture `137_tuple_projection_unresolved_path_unchanged.flx` (unresolved-path guard; strict-path assertion uses `E425`),
  - compiler pipeline fixture assertions that lock both precise and unresolved behaviors.
- Evidence commands:
  - `cargo test --test type_inference_tests`
  - `cargo test --test compiler_rules_tests`
  - `cargo test --test snapshot_diagnostics`
  - `cargo test --test examples_fixtures_snapshots`
- Gate policy:
  - `examples_fixtures_snapshots` is recorded but non-blocking for T9 when failure is attributable to unrelated snapshot churn from other tasks.
  - Such failures must be explicitly logged under "Known External Churn" with snapshot path(s) and owning task.
- Known External Churn:
  - `tests/snapshots/examples_fixtures/basics__array_hash_combo.snap.new` is out-of-scope for T9 and owned by E300 dedup/snapshot governance work.

### T10 — H3 Match Scrutinee Constraint Propagation
- **Goal:** Propagate constructor-family constraints from match arms to scrutinee type.
- **Files:**
  - `src/ast/type_infer.rs`
- **Changes:**
  - unify scrutinee with family type when arm family is consistent
  - no propagation for wildcard-only/heterogeneous families
- **Tests:**
  - constrained scrutinee follow-up typing
  - non-propagation guard tests
- **Fixtures:**
  - `138`, `139`
- **Risk:** High (inference interaction).
- **Done When:** Scrutinee type updates only in safe, deterministic cases.

#### T10 Closure Note (Regression Lock)
- T10 implementation adds a family-consistency propagation pass in `Expression::Match` inference:
  - constraining arm patterns are classified into constructor families (built-ins + ADT),
  - scrutinee type is unified with one family type only when all constraining arms are family-consistent.
- Safety guards are preserved:
  - no propagation for wildcard-only/literal-only/identifier-only arms,
  - no propagation for heterogeneous families, mixed tuple arities, or mixed ADT families.
- T10 close-out adds:
  - fixture `138_match_scrutinee_constraint_propagates.flx` (positive propagation behavior),
  - fixture `139_match_scrutinee_constraint_no_propagation_mixed_family.flx` (mixed-family guard behavior),
  - HM + compiler pipeline assertions that lock both propagation and non-propagation cases.
- Evidence commands:
  - `cargo test --test type_inference_tests`
  - `cargo test --test compiler_rules_tests`
  - `cargo test --test snapshot_diagnostics`
  - `cargo test --test examples_fixtures_snapshots`
- Gate policy:
  - `examples_fixtures_snapshots` is recorded but non-blocking for T10 when failure is attributable to unrelated snapshot churn from other tasks.
  - Such failures must be explicitly logged under "Known External Churn" with snapshot path(s) and owning task.
- Known External Churn:
  - `tests/snapshots/examples_fixtures/basics__array_hash_combo.snap.new` is out-of-scope for T10 and owned by E300 dedup/snapshot governance work.

### T11 — H4 Recursive Self-Reference Propagation
- **Goal:** Improve unannotated recursive inference via fixpoint-style self-unification.
- **Files:**
  - `src/ast/type_infer.rs`
- **Changes:**
  - second-step unification for recursive self references in unannotated functions
- **Tests:**
  - recursive sum/map inference paths
  - no regressions on existing recursion tests
- **Fixtures:**
  - `140`, `141`
- **Risk:** High.
- **Done When:** Recursive return typing no longer collapses to Any in targeted cases.

#### T11 Closure Note (Regression Lock)
- T11 implementation adds a bounded self-recursive refinement pass in unannotated function inference:
  - detect direct self-call usage in function body,
  - run one additional body inference pass with temporary self binding,
  - unify second-pass body type with a fresh return slot and fold back into final return type.
- Scope is intentionally limited:
  - self recursion only,
  - no SCC/mutual-recursion fixpoint in T11.
- Safety guards:
  - no second pass for annotated return functions,
  - no second pass for non-self-recursive functions.
- T11 close-out adds:
  - fixture `140_recursive_self_reference_return_precision.flx` (positive recursive refinement behavior),
  - fixture `141_recursive_self_reference_negative_guard.flx` (guard regression behavior),
  - HM + compiler pipeline assertions for refinement and non-regression.
- Evidence commands:
  - `cargo test --test type_inference_tests`
  - `cargo test --test compiler_rules_tests`
  - `cargo test --test snapshot_diagnostics`
  - `cargo test --test examples_fixtures_snapshots`
- Gate policy:
  - `examples_fixtures_snapshots` is recorded but non-blocking for T11 when failure is attributable to unrelated snapshot churn from other tasks.
  - Such failures must be explicitly logged under "Known External Churn" with snapshot path(s) and owning task.
- Known External Churn:
  - `tests/snapshots/examples_fixtures/basics__array_hash_combo.snap.new` is out-of-scope for T11 and owned by E300 dedup/snapshot governance work.

### T12 — T1 Bool Exhaustiveness
- **Goal:** Enforce bool-domain exhaustiveness (`true`/`false` or wildcard).
- **Files:**
  - exhaustiveness checker path in compiler
  - `src/diagnostics/compiler_errors.rs` if message helper needed
- **Changes:** bool-domain missing-arm E015 with clear missing-arm hint.
- **Tests:**
  - bool missing `true`
  - bool missing `false`
  - both present
  - wildcard fallback
- **Fixtures:**
  - `142`, `143`
- **Risk:** Medium.
- **Done When:** Bool exhaustiveness is deterministic and message-specific.

#### T12 Closure Note (Regression Lock)
- Core behavior is already implemented in `src/bytecode/compiler/expression.rs` via:
  - `check_general_match_exhaustiveness(...)` bool-domain branch,
  - `GeneralCoverageDomain::Bool` domain classification.
- T12 close-out adds:
  - fixture `142_match_bool_missing_true.flx` (missing-`true` path),
  - fixture `143_match_bool_missing_false.flx` (missing-`false` path),
  - compiler/pattern-validation assertions that lock message-level `E015` behavior.
- Evidence commands:
  - `cargo test --test compiler_rules_tests`
  - `cargo test --test pattern_validation`
  - `cargo test --test snapshot_diagnostics`
  - `cargo test --test examples_fixtures_snapshots`
- Gate policy:
  - `examples_fixtures_snapshots` is recorded but non-blocking for T12 when failure is attributable to unrelated snapshot churn from other tasks.
  - Such failures must be explicitly logged under "Known External Churn" with snapshot path(s) and owning task.
- Known External Churn:
  - `tests/snapshots/examples_fixtures/basics__array_hash_combo.snap.new` is out-of-scope for T12 and owned by E300 dedup/snapshot governance work.

### T13 — T2 Guarded Wildcard Contract
- **Goal:** Targeted E015 for guarded wildcard non-exhaustiveness.
- **Files:**
  - exhaustiveness checker path
  - `src/diagnostics/compiler_errors.rs`
- **Changes:** add `guarded_wildcard_non_exhaustive` and route guarded-only wildcard case.
- **Tests:**
  - guarded wildcard only fails with targeted message
  - guarded + bare wildcard passes
- **Fixtures:**
  - `144`, `145`
- **Risk:** Low.
- **Done When:** Guarded wildcard semantics are explicit and stable.

#### T13 Closure Note (Regression Lock)
- T13 routes a targeted `E015` message for guarded-wildcard-only non-exhaustive matches via:
  - `guarded_wildcard_non_exhaustive(...)` diagnostic constructor,
  - guarded-catchall detection in general match exhaustiveness routing.
- T13 close-out adds:
  - fixture `144_guarded_wildcard_only_non_exhaustive_targeted.flx` (targeted diagnostic path),
  - fixture `145_guarded_wildcard_with_fallback_ok.flx` (guarded + bare wildcard exhaustive pass path),
  - compiler + pattern-validation assertions locking targeted message behavior.
- Evidence commands:
  - `cargo test --test compiler_rules_tests`
  - `cargo test --test pattern_validation`
  - `cargo test --test snapshot_diagnostics`
  - `cargo test --test examples_fixtures_snapshots`
- Gate policy:
  - `examples_fixtures_snapshots` is recorded but non-blocking for T13 when failure is attributable to unrelated snapshot churn from other tasks.
  - Such failures must be explicitly logged under "Known External Churn" with snapshot path(s) and owning task.
- Known External Churn:
  - `tests/snapshots/examples_fixtures/basics__array_hash_combo.snap.new` is out-of-scope for T13 and owned by E300 dedup/snapshot governance work.

### T14 — T3/T4 ADT Pattern Arity + Cross-Module Constructor Boundary
- **Goal:**
  - add constructor-pattern arity mismatch (`E085`)
  - add strict/non-strict cross-module constructor boundary diagnostics (`E086`/`W201`)
- **Files:**
  - pattern checking/lowering path
  - module boundary resolution path
  - `src/diagnostics/compiler_errors.rs`
  - `src/diagnostics/registry.rs`
- **Changes:**
  - register and emit new codes
  - strict: error, non-strict: warning behavior for boundary rule
- **Tests:**
  - arity mismatch cases
  - strict/non-strict boundary cases
  - VM/JIT parity tuple lock for diagnostic classes/messages
- **Fixtures:**
  - `146` to `150`
- **Risk:** High (new code-family surface + boundary policy).
- **Done When:** E085/E086/W201 behavior is deterministic, registered, and fully covered.

#### T14 Closure Note (Global Recode + Boundary Split)
- T14 introduces and registers:
  - `E085` (`CONSTRUCTOR PATTERN ARITY MISMATCH`) for pattern-only constructor arity violations,
  - `E086` (`CROSS-MODULE CONSTRUCTOR ACCESS`) for strict-mode boundary enforcement,
  - `W201` (`CROSS-MODULE CONSTRUCTOR ACCESS`) for non-strict warning-only boundary reporting.
- Recode policy:
  - pattern constructor arity checks are routed to `E085`,
  - constructor call arity checks remain `E082`,
  - prior strict boundary `E084` usage is replaced by `E086`/`W201` split.
- T14 close-out fixtures:
  - `146_constructor_pattern_arity_some_too_many.flx`
  - `147_constructor_pattern_arity_none_too_many.flx`
  - `148_constructor_pattern_arity_left_too_many.flx`
  - `149_cross_module_constructor_access_strict.flx`
  - `150_cross_module_constructor_access_nonstrict_warning.flx`
- Evidence commands:
  - `cargo test --test error_codes_registry_tests`
  - `cargo test --test compiler_rules_tests`
  - `cargo test --test snapshot_diagnostics`
  - `cargo test --test examples_fixtures_snapshots`
  - `cargo test --all --all-features purity_vm_jit_parity_snapshots`

#### T14 Known Open Items (Harness-Limited, Non-Semantic)
- `examples_fixtures_snapshots` entries for `149` and `150` currently show `E018` (`IMPORT NOT FOUND`) because the fixture snapshot harness resolves module roots to fixture parent + `src/` and does not pass `--root examples/type_system`.
  - This matches existing harness behavior for other module-root-dependent fixtures (`33`, `62`, `63`, `66`, `79`, `90`) and is not a T14 semantic regression.
- `examples/type_system/TypeSystem/BoundaryCtor.flx` fixture snapshot currently shows `E024` (`MODULE PATH MISMATCH`) under the same generic harness invocation model.
  - This is expected for module file snapshots in the generic examples harness and is not a T14 semantic regression.
- `E084` (`MODULE ADT CONSTRUCTOR NOT EXPORTED`) remains defined and registered for compatibility/history, but has no active emit sites after T14 recode by design.

#### T14 Evidence Outcome Classification
- `cargo test --test error_codes_registry_tests` -> PASS (blocking)
- `cargo test --test compiler_rules_tests` -> PASS (blocking)
- `cargo test --test snapshot_diagnostics` -> PASS (blocking)
- `cargo test --all --all-features --test purity_vm_jit_parity_snapshots` -> PASS (blocking)
- `cargo test --all --all-features --test runtime_vm_jit_parity_release` -> PASS (blocking)
- `cargo test --test examples_fixtures_snapshots` -> FAIL/DRIFT (non-blocking for T14 known harness limitation)

#### T14 Non-Goals Lock
- No harness changes in T14 closure.
- Harness root broadening/exclusion policy is deferred to a dedicated infrastructure task.

## 13. Task Dependencies and Merge Strategy

### Locked dependency graph
- `T0 -> T1 -> (T2,T3,T4) -> T5 -> T6 -> T7`
- `T1 -> (T8,T9,T10,T11)`
- `(T8,T9) -> T10 -> T11`
- `(T12,T13) independent after T0`
- `T14` depends on `T12/T13` stability for exhaustiveness/boundary message policy
- Final closure requires all tasks complete.

### Merge policy
1. One task per PR.
2. No mixed-feature PRs.
3. Snapshot updates only in task PR that caused them.
4. Revert only the regressing task PR if needed.

## 14. Validation Gates and Evidence Contract

### Per-task minimum gate
- `cargo check --all --all-features`
- task-local test suites
- relevant fixture spot-runs

### Full closure gate
- `cargo check --all --all-features`
- `cargo test --test parser_tests`
- `cargo test --test parser_recovery`
- `cargo test --test type_inference_tests`
- `cargo test --test compiler_rules_tests`
- `cargo test --test diagnostics_aggregator_tests`
- `cargo test --test diagnostic_render_tests`
- `cargo test --test snapshot_parser`
- `cargo test --test snapshot_diagnostics`
- `cargo test --all --all-features purity_vm_jit_parity_snapshots`
- `cargo test --all --all-features --test runtime_vm_jit_parity_release`

### Evidence contract (required in every task PR)
1. Task ID (`Tn`).
2. Files changed.
3. Commands run + outcomes.
4. Fixtures added/updated.
5. Snapshot changes (yes/no + rationale).
6. VM/JIT parity status.
7. Rollback note for the task.

## 15. Acceptance Criteria (Task Completion)

1. Parser alias/structural/symbol cases (`122..130`) produce targeted E030/E034 and continue parsing where intended.
2. Span precision (`131..133`) shows subterm-focused primary labels.
3. HM hardening (`134..141`) improves concrete diagnostics while preserving Any/Var guard suppression.
4. Exhaustiveness/pattern boundary (`142..150`) emits deterministic E015/E085/E086/W201 behavior.
5. Snapshot/parity churn is intentional-only and documented task-by-task.
6. Fixture IDs `122..150` are present, referenced, and reproducible via documented commands.
7. All tasks `T0..T14` are complete with auditable evidence.

### T15 — P5 Remaining `expect_peek` Contextual Recovery (Broad Sweep)
- **Goal:** Replace remaining generic `expect_peek` parser diagnostics with construct-aware E034 messages and hints.
- **Files:**
  - `src/syntax/parser/helpers.rs`
  - `src/syntax/parser/expression.rs`
  - `src/syntax/parser/statement.rs`
  - `src/syntax/parser/literal.rs`
- **Changes:**
  - add parser-local contextual helpers (`expect_peek_context`, `expect_peek_contextf`)
  - route expression/statement/literal/helper `expect_peek` callsites to construct-aware messaging
  - preserve synchronization and recovery behavior
- **Tests:**
  - `tests/parser_tests.rs` table-driven contextual-message assertions
  - `tests/parser_recovery.rs` fixture-based recovery continuation checks
- **Fixtures:**
  - `173` to `184`
- **Risk:** Medium (message and snapshot churn across parser transcripts).
- **Done When:** Broad parser structure sites no longer emit bare generic `Expected \`X\`, got Y.` diagnostics.

#### T15 Closure Note (Snapshot Policy Lock)
- Intentional parser-message drift from P5 contextualization is accepted and snapshotted.
  - Accepted drift includes:
    - `tests/snapshots/examples_fixtures/type_system__failing__182_list_comprehension_missing_left_arrow.snap`
    - `tests/snapshots/examples_fixtures/type_system__failing__184_type_expr_missing_close_paren.snap`
    - `tests/snapshots/examples_fixtures/type_system__failing__85_hm_function_effect_mismatch.snap`
  - Change rationale: parser diagnostics moved from generic token text to contextual `E034` wording with construct-specific hints.
- Gate policy for T15:
  - Blocking: `parser_tests`, `parser_recovery`, `snapshot_parser`, `snapshot_diagnostics`, `cargo check --all --all-features`.
  - Informational: `examples_fixtures_snapshots` only when failure is explicitly unrelated to P5.
- `examples_fixtures_snapshots` classification rules:
  - P5-related contextual parser message drift is intentional and should be accepted.
  - Non-blocking external churn must be logged with snapshot path, reason, and owning task.

#### T15 Evidence Commands
- `cargo test --test parser_tests`
- `cargo test --test parser_recovery`
- `cargo test --test snapshot_parser`
- `cargo test --test snapshot_diagnostics`
- `cargo check --all --all-features`
- `cargo test --test examples_fixtures_snapshots`

#### T15 Evidence Outcomes
- `cargo test --test parser_tests` -> PASS (blocking)
- `cargo test --test parser_recovery` -> PASS (blocking)
- `cargo test --test snapshot_parser` -> PASS (blocking)
- `cargo test --test snapshot_diagnostics` -> PASS (blocking)
- `cargo check --all --all-features` -> PASS (blocking)
- `cargo test --test examples_fixtures_snapshots` -> PASS (informational)

#### T15 Known External Churn
- None in this closure run.
- If future unrelated drift appears in `examples_fixtures_snapshots`, record: snapshot path, unrelated reason, and owning task.

### T16 — P6 Parser Cascade Suppression (Construct-Scoped)
- **Goal:** Reduce secondary parser noise by suppressing follow-up delimiter/shape diagnostics once a primary structural root error is emitted in the same construct.
- **Files:**
  - `src/syntax/parser/mod.rs`
  - `src/syntax/parser/helpers.rs`
  - `src/syntax/parser/expression.rs`
  - `src/syntax/parser/statement.rs`
- **Changes:**
  - add construct-local diagnostic checkpoint helpers
  - suppress follow-up parser diagnostics when a structural root (`E034`/`E076`) already exists in the active construct
  - keep recovery behavior local and deterministic; no global diagnostic suppression
- **Tests:**
  - `tests/parser_tests.rs` cascade suppression assertion for malformed signature fixture `184`
  - `tests/parser_recovery.rs` recovery coverage includes fixture `184`
- **Fixtures:**
  - lock `184` as the signature-cascade sentinel
- **Risk:** Medium (over-suppression if guard is too broad).
- **Done When:** malformed-signature/root-delimiter cases emit one root parser error with minimal actionable follow-ups.
- **Evidence commands:**
  - `cargo test --test parser_tests`
  - `cargo test --test parser_recovery`
  - `cargo test --test snapshot_parser`
  - `cargo test --test snapshot_diagnostics`
  - `cargo check --all --all-features`
  - `cargo test --test examples_fixtures_snapshots` (informational)
- **Known External Churn:**
  - `tests/snapshots/examples_fixtures/type_system__failing__185_runtime_boundary_arg_e1004.snap.new`
    - out-of-scope for T16 parser-cascade semantics; reflects examples snapshot harness root policy for module-qualified fixtures (Owner: runtime E1004 parity lane / proposal 043).
