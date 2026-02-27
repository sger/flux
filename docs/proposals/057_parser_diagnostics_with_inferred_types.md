# Proposal 057: Parser Diagnostics Improvement with Inferred Types

**Status:** Draft
**Date:** 2026-02-27
**Depends on:** `032_type_system_with_effects.md`, `046_typed_ast_hm_architecture.md`

---

## 1. Summary

Improve Flux diagnostics by leveraging HM-inferred type information to produce richer, more actionable error messages. This covers six areas: compile-time arity checks, if/else branch mismatch detection, multi-error continuation, function-type mismatch decomposition, type annotation error recovery, and match arm type consistency.

---

## 2. Motivation

With HM inference fully wired (Iter 1–5), the compiler now knows the inferred type of every expression. Several diagnostic gaps remain where this information is available but unused:

1. **Arity mismatches are runtime-only** — `add(1, 2, 3)` where `fn add(a: Int, b: Int)` panics at E1000 instead of being caught at compile time.
2. **If/else branch type mismatches are silent** — `if true { 42 } else { "nope" }` compiles and runs, returning whichever branch is taken, even when an explicit return type annotation exists.
3. **First-error-stops-all for type errors** — `greet(42)` on line 6 prevents reporting `let x: Bool = greet("world")` on line 7. The compiler should report all independent type errors.
4. **Function type mismatches are opaque** — `Cannot unify (Int) -> String with (Int) -> Int` doesn't tell the user which part mismatched (params vs. return).
5. **Type annotation parse failures lose context** — `let x: List<Int, = 42` causes `parse_type_expr` to return `None` without recovery, losing the rest of the statement.
6. **Match arm type consistency is unchecked** — different match arms can return different types without a compile-time diagnostic.

---

## 3. Goals

1. Catch arity mismatches at compile time using function type information from HM inference.
2. Detect if/else branch type mismatches when both branches have concrete inferred types.
3. Allow the compiler to continue past type errors to report multiple independent diagnostics per file.
4. Decompose function-type mismatches into specific sub-errors (parameter position, return type).
5. Improve type annotation parse error recovery so the parser can continue past malformed annotations.
6. Detect match arm type inconsistencies at compile time.

---

## 4. Non-Goals

1. No new syntax or grammar changes.
2. No runtime type checking changes.
3. No changes to the HM inference algorithm itself.
4. No effect system enforcement (separate proposal track).
5. No typed-AST migration (proposal 046).

---

## 5. Detailed Design

### 5.1 Compile-Time Arity Checking

**Current behavior:** `add(1, 2, 3)` compiles, then panics at runtime with `E1000: wrong number of arguments: want=2, got=3`.

**Proposed:** During PASS 2 in `compile_call_expression`, when the callee resolves to a known function with a `FunctionContract` or HM-inferred `Fun` type, compare the call argument count against the expected parameter count. Emit a new E-code:

```
--> compiler error[E056]: WRONG NUMBER OF ARGUMENTS

Function `add` expects 2 arguments, but 3 were provided.

  --> test.flx:6:11
  |
6 |     print(add(1, 2, 3))
  |           ^^^^^^^^^^^^
  |           ------------ expected 2 arguments

  --> test.flx:1:1
  |
1 | fn add(a: Int, b: Int) -> Int {
  |    --- function `add` defined here with 2 parameters

Help:
  Remove the extra argument(s): `add(1, 2)`
```

**Files:** `src/bytecode/compiler/expression.rs`, `src/diagnostics/compiler_errors.rs`

**Constraints:**
- Only fire when the callee is statically known (named function, not a dynamic closure argument).
- Variadic-like patterns (rest params, if added later) should suppress this check.
- HM inference already unifies `Fun` types with the right arity; this is a compiler-side check with better diagnostics.

### 5.2 If/Else Branch Type Mismatch Detection

**Current behavior:** `if true { 42 } else { "nope" }` compiles and runs without error, returning whichever branch value at runtime.

**Proposed:** After inferring both branch types via HM, when both are concrete and different, emit a diagnostic. Two sub-cases:

**Case A — No return type annotation (pure inference):**
HM `join_types` already unifies branches. If both are concrete and unification fails, `unify_reporting` should fire. Currently it may not fire because of gradual-typing fallback. Fix: ensure `join_types` calls `unify_reporting` (not just `unify_propagate`) when both branches are fully concrete.

**Case B — With return type annotation:**
The annotated return type may mask the mismatch (one branch unifies, the other doesn't). The compiler should check each branch individually against the return type annotation and point to the specific branch that fails:

```
--> compiler error[E300]: TYPE UNIFICATION ERROR

Cannot unify Int with String.

  --> test.flx:5:9
  |
3 |     if true {
  |     -- `if` and `else` branches must have the same type
4 |         42
  |         -- this branch has type `Int`
5 |         "nope"
  |         ^^^^^^ expected Int, found String
  |         ------ this branch has type `String`
```

**Files:** `src/ast/type_infer.rs` (`infer_if`), `src/bytecode/compiler/expression.rs`

### 5.3 Multi-Error Continuation

**Current behavior:** When `greet(42)` fails type checking on line 6, the error on `let x: Bool = greet("world")` on line 7 is never reported. The user must fix errors one at a time.

**Proposed:** The compiler's PASS 2 currently bails out when `compile_expression` encounters a type error. Instead of returning early, record the diagnostic and continue compiling subsequent statements. The compiled bytecode is never executed (error count > 0), so emitting placeholder instructions for errored expressions is safe.

**Approach:**
- In `hm_expr_typer.rs`, `validate_expr_expected_type_with_policy` already records diagnostics without stopping compilation. The issue is that after the first call-site mismatch, HM inference may propagate incorrect types to downstream expressions. The fix is to ensure HM inference replaces errored type variables with fresh variables (or `Any`) after a unification failure, so downstream inference continues independently.
- Add an error budget (e.g., max 20 type errors per file) to prevent cascade floods.

**Files:** `src/ast/type_infer.rs`, `src/bytecode/compiler/mod.rs`, `src/bytecode/compiler/expression.rs`

### 5.4 Function Type Mismatch Decomposition

**Current behavior:** `Cannot unify (Int) -> String with (Int) -> Int` — the user must mentally diff the two types.

**Proposed:** When unifying two `Fun` types that differ, decompose the error into specific sub-mismatches:

```
--> compiler error[E300]: TYPE UNIFICATION ERROR

Function type mismatch: return types differ.

  --> test.flx:6:19
  |
6 |     let x: (Int) -> Int = formatter
  |                   ^^^^^^
  |                   ------ expected return type `Int`, found `String`

Help:
  The parameter types match, but the return types are incompatible.
```

**Approach:** In `unify_with_span`, when unifying `Fun(p1, r1) ↔ Fun(p2, r2)`, on failure, re-attempt pairwise unification of params and return to identify which component failed. Emit a decomposed message.

**Files:** `src/types/type_subst.rs`, `src/ast/type_infer.rs`

### 5.5 Type Annotation Error Recovery

**Current behavior:** `let x: List<Int, = 42` — `parse_type_expr` returns `None` and the entire let statement is lost. Downstream code may cascade.

**Proposed:** When `parse_type_expr` fails mid-parse (e.g., unexpected `,` or missing `>`), emit a specific diagnostic and attempt to recover by:
1. Consuming tokens until a recovery point (`=`, `)`, `{`, newline at lower indent).
2. Returning `None` for the type annotation but allowing the statement to continue parsing with the annotation treated as absent.

This means `let x: <broken> = 42` would still parse the `= 42` part and infer `x`'s type from the initializer (gradual typing).

**Files:** `src/syntax/parser/helpers.rs` (`parse_type_expr`, `parse_non_function_type`)

### 5.6 Match Arm Type Consistency

**Current behavior:** Match arms with different types compile without error; only the executed arm's type matters at runtime.

**Proposed:** After inferring all match arm body types, check that they are consistent. When two arms have different concrete types, emit:

```
--> compiler error[E300]: TYPE UNIFICATION ERROR

Match arms have incompatible types.

  --> test.flx:4:20
  |
3 |         Some(x) -> x + 1
  |                    ----- this arm has type `Int`
4 |         None -> "default"
  |                 ^^^^^^^^^ expected Int, found String

Help:
  All match arms must return the same type.
```

**Approach:** HM `infer_match` already unifies arm types. Ensure it uses `unify_reporting` (not silent) for arm-vs-arm joining, and add secondary labels pointing to the first arm's type as the expected type.

**Files:** `src/ast/type_infer.rs` (`infer_match`)

---

## 6. Implementation Order

| Phase | Item | Complexity | Impact |
|-------|------|-----------|--------|
| 1 | 5.1 Compile-time arity checks | Low | High — catches common bugs at compile time |
| 2 | 5.2 If/else branch mismatch | Medium | High — fundamental type safety |
| 3 | 5.6 Match arm type consistency | Medium | High — fundamental type safety |
| 4 | 5.3 Multi-error continuation | Medium | Medium — better DX |
| 5 | 5.4 Function type decomposition | Low | Medium — better DX |
| 6 | 5.5 Type annotation error recovery | Medium | Low — rare edge case |

---

## 7. New Error Codes

| Code | Title | Description |
|------|-------|-------------|
| E056 | WRONG_NUMBER_OF_ARGUMENTS | Compile-time arity mismatch |

All other improvements use existing error codes (E300 TYPE_UNIFICATION_ERROR) with enhanced messages and labels.

---

## 8. Diagnostics Compatibility

- Existing E300/E055 diagnostics remain unchanged for cases they already cover.
- New compile-time arity check (E056) replaces runtime E1000 for statically-known calls — strictly better (earlier detection).
- If/else and match arm checks are new diagnostics for currently-silent bugs — no existing behavior changes.

---

## 9. Test Plan

### Unit Tests
1. Arity mismatch: too many args, too few args, correct args (no error).
2. If/else branch mismatch: Int vs String, Int vs Float, matching types (no error).
3. Match arm mismatch: inconsistent arm types, consistent (no error).
4. Multi-error: two independent type errors in one file, both reported.
5. Function type decomposition: param mismatch, return mismatch, both.
6. Type annotation recovery: malformed type, statement continues.

### Snapshot Tests
- Update `tests/snapshots/examples_fixtures/` for any affected examples.
- Add new `examples/type_system/` files for each new diagnostic.

### Regression
- `cargo test` — all existing tests pass.
- `cargo clippy --all-targets --all-features -- -D warnings` — clean.
- Verify no unintended cascade inflation.

---

## 10. Validation Commands

```bash
cargo fmt --all -- --check
cargo check --all --all-features
cargo test
cargo test --test examples_fixtures_snapshots
cargo clippy --all-targets --all-features -- -D warnings
cargo insta test --accept  # if snapshots change
```

---

## 11. Risks and Mitigations

1. **Risk:** Multi-error continuation causes cascade of misleading secondary errors.
   **Mitigation:** Error budget (max 20 per file), fresh type variables after failure.

2. **Risk:** Arity check false positives for dynamic dispatch or variadic patterns.
   **Mitigation:** Only fire for statically-known function calls with concrete contracts.

3. **Risk:** Branch/arm mismatch checks interfere with gradual typing (untyped code).
   **Mitigation:** Only report when both types are fully concrete (no `Any`, no type variables).

4. **Risk:** Type annotation recovery consumes too many tokens and loses sync.
   **Mitigation:** Conservative recovery points (`=`, `)`, `{`, `}`), with error limit.

---

## 12. Acceptance Criteria

1. `add(1, 2, 3)` produces compile-time E056 instead of runtime E1000.
2. `if true { 42 } else { "nope" }` produces compile-time E300 with branch labels.
3. Two independent type errors in one file both appear in compiler output.
4. Function type mismatches identify which component (params/return) differs.
5. All existing tests pass, no diagnostic regression.
