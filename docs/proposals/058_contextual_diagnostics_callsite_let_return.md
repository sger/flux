# Proposal 058: Contextual Diagnostics — Call-Site Arguments, Let Annotations, and Function Return Types

**Status:** Implemented
**Date:** 2026-02-28
**Updated:** 2026-02-28
**Depends on:** `057_parser_diagnostics_with_inferred_types.md` (ReportContext architecture, `unify_with_context`, `UnifyErrorDetail`)

---

## 1. Summary

Proposal 057 introduced the `ReportContext` architecture and delivered contextual diagnostics for if/else branch mismatches, match arm mismatches, and function type decomposition. This proposal completes the contextual diagnostic picture with three remaining high-impact improvements:

1. **Named call-site argument diagnostics** — enrich the existing `fun_param_type_mismatch` path at call sites with the function name and definition span.
2. **Let annotation mismatch dual-label diagnostic** — replace the generic PASS 2 "binding initializer does not match" message with a dual-label diagnostic showing both the annotation span and the value span.
3. **Function return type mismatch dual-label diagnostic** — replace the generic PASS 2 "return expression type does not match" message with a named, dual-label diagnostic.

All three build directly on 057's infrastructure with no new architectural changes.

---

## 2. Motivation

### 2.1 Current State After 057

After proposal 057, the following diagnostic classes are rich and contextual:
- `if`/`else` branch mismatch — ✅ dual labels, Elm-style
- `match` arm mismatch — ✅ dual labels, Elm-style
- Function type decomposition (param/return/arity) — ✅ component-specific messages
- Call arity mismatch — ✅ E056 with function def span
- Undefined variable "did you mean?" — ✅ already implemented via `find_similar_names`

The following remain generic or incomplete:

### 2.2 Gap A — Call-site argument mismatch: missing function name and def span

`greet(42)` where `fn greet(name: String)` currently emits via `fun_param_type_mismatch` (which fires because `UnifyErrorDetail::FunParamMismatch` is set in `infer_call`), but the message has no mention of which function was called, and no secondary label pointing to the definition:

```
-- compiler error[E300]: TYPE MISMATCH
Function parameter types do not match.

3 |     greet(42)
  |     ^^^^^^^^^ expected String, found Int
```

The Elm ideal:
```
-- compiler error[E300]: TYPE MISMATCH
The 1st argument to `greet` has the wrong type:

3 |     greet(42)
  |           ^^ this is Int

The function `greet` expects a String as the 1st parameter:

1 | fn greet(name: String) -> Unit {
  |               ^^^^^^
```

### 2.3 Gap B — Let annotation mismatch: generic message, no annotation span

`let x: Int = "hello"` is checked in PASS 2 (`statement.rs:128`) via `validate_expr_expected_type_with_policy`. The current output:

```
-- compiler error[E300]: TYPE MISMATCH
binding initializer does not match type annotation

1 | let x: Int = "hello"
  |              ^^^^^^^ this is String
```

The annotation span (`Int`) is not labeled. The user cannot see which part of the statement is the conflict. The Elm ideal:

```
-- compiler error[E300]: TYPE MISMATCH
The value of `x` does not match its type annotation:

1 | let x: Int = "hello"
  |        ^^^   ^^^^^^^ this is String
  |        |
  |        but `x` was annotated as Int
```

### 2.4 Gap C — Function return type mismatch: generic message, no annotation span

`fn add(a: Int, b: Int) -> Int { "oops" }` is checked in PASS 2 (`statement.rs:438`) via `validate_expr_expected_type_with_policy`. The current output:

```
-- compiler error[E300]: TYPE MISMATCH
return expression type does not match the declared return type

3 |     "oops"
  |     ^^^^^^ this is String
```

The return type annotation (`Int`) and the function name are not shown. The Elm ideal:

```
-- compiler error[E300]: TYPE MISMATCH
The return value of `add` does not match its declared return type:

3 |     "oops"
  |     ^^^^^^ this is String

But `add` was declared to return Int:

1 | fn add(a: Int, b: Int) -> Int {
  |                            ^^^
```

---

## 3. Goals

1. Name the called function in argument-mismatch diagnostics and show the definition site.
2. Show both the annotation span and the value span in let-binding type mismatches.
3. Show both the return annotation span and the return expression span in function return mismatches.
4. No new architectural changes — all three improvements reuse 057's infrastructure.
5. No false positives on untyped/gradual code — all guards already in place from 057.

---

## 4. Non-Goals

1. No changes to HM unification rules.
2. No changes to the PASS 2 multi-error continuation (already done in 057).
3. No improvements to effect/purity diagnostics (separate track).
4. No let-destructuring pattern diagnostics (separate from simple let).

---

## 5. Architectural Design

### 5.1 Gap A — Named Call-Site Diagnostics via `ReportContext::CallArg`

**Current path:**

`infer_call` builds `Fun(arg_tys, ret_var, [])` and calls `unify_reporting(&fn_ty, &expected_fn_ty, span)`. When the callee has `Fun([String], _, _)` and the argument is `Int`, unification produces `UnifyErrorDetail::FunParamMismatch { index: 0 }`. The `function_detail_diag` closure in `unify_with_context` picks this up and calls `fun_param_type_mismatch(file, span, 1, "String", "Int")` — a generic decomposed error without function identity.

**Proposed change:**

Extract the callee identity (name + definition span) from the `function` expression inside `infer_call`. Pass a `ReportContext::CallArg { fn_name, arg_index, fn_def_span }` to `unify_with_context` instead of `Plain`.

```rust
// src/ast/type_infer.rs — ReportContext additions
enum ReportContext {
    Plain,
    IfBranch { then_span: Span, else_span: Span },
    MatchArm { first_span: Span, arm_span: Span, arm_index: usize },
    // New:
    CallArg {
        fn_name:     Option<String>,  // None for dynamic callees
        fn_def_span: Option<Span>,    // None if not statically known
    },
}
```

In `infer_call`, before calling `unify_with_context`:

```rust
let call_ctx = match function {
    Expression::Identifier { name, .. } => {
        let fn_name = self.interner.resolve(*name).to_string();
        let fn_def_span = self.env.lookup_span(*name);  // span of the fn declaration
        ReportContext::CallArg {
            fn_name: Some(fn_name),
            fn_def_span,
        }
    }
    _ => ReportContext::CallArg { fn_name: None, fn_def_span: None },
};
self.unify_with_context(&fn_ty, &expected_fn_ty, span, call_ctx);
```

In `unify_with_context`, the `CallArg` arm reads `e.detail`:
- `FunParamMismatch { index }` → `call_arg_type_mismatch(file, arg_span, fn_name, index+1, fn_def_span, &exp, &act)`
- Other details → fall through to `function_detail_diag()` or `type_unification_error`

**New diagnostic constructor:**

```rust
pub fn call_arg_type_mismatch(
    file:        String,
    arg_span:    Span,          // primary: the argument expression
    fn_name:     Option<&str>,  // "greet", or None for dynamic callees
    arg_index:   usize,         // 1-based
    fn_def_span: Option<Span>,  // secondary: function definition
    expected:    &str,
    actual:      &str,
) -> Diagnostic
```

**Challenge: per-argument spans**

`infer_call` currently unifies the whole function type at once (`Fun(all_args, ret, [])` vs `fn_ty`). The resulting mismatch span is the whole call span — not the individual argument span. To get the argument span in the primary label, we need to identify which argument failed.

The `UnifyErrorDetail::FunParamMismatch { index }` already tells us the index. The argument span is `arguments[index].span()`. This span must be threaded into the `call_arg_type_mismatch` call. Inside `unify_with_context`, `arguments` is not available — only `span` (the full call span).

**Solution:** Pass a `call_arg_spans: Option<&[Span]>` alongside the context, or resolve the argument span from `index` at the `infer_call` call site after receiving the error. The simplest approach: extract the reporting logic out of `unify_with_context` and into `infer_call` directly for the `CallArg` case, similar to how `infer_match` drives context-selection inline.

Concretely, `infer_call` can be restructured to:
1. Unify each param individually: `for (i, (arg_ty, param_ty)) in ...`.
2. On failure, emit `call_arg_type_mismatch` with `arguments[i].span()` as the primary span.

This approach is cleaner and gives accurate per-argument spans. The only cost is that the global unification of `fn_ty` against `Fun(all_args, ret, [])` is replaced by per-param unification — which is equivalent for the concrete-type path and still returns `Any` on failure.

---

### 5.2 Gap B — Let Annotation Dual-Label via Dedicated Diagnostic Constructor

**Current path:**

PASS 2 `statement.rs:128` calls `validate_expr_expected_type_with_policy(expected_infer, value, ...)`. This internally calls `hm_expr_type_strict_path(value)` and — if the types mismatch — calls `type_unification_error(file, span, exp, act)` with just the value span and a generic message.

**Proposed change:**

At `statement.rs:128`, after the annotation is successfully converted to `expected_infer`, replace the call to `validate_expr_expected_type_with_policy` with a direct HM type check followed by a dedicated constructor call:

```rust
// statement.rs — inside compile_let with type annotation
if let HmExprTypeResult::Known(val_infer) = self.hm_expr_type_strict_path(value) {
    if !types_compatible(&val_infer, &expected_infer) {
        let val_ty_str  = display_infer_type(&val_infer, &self.interner);
        let ann_ty_str  = display_infer_type(&expected_infer, &self.interner);
        return Err(Self::boxed(let_annotation_type_mismatch(
            self.file_path.clone(),
            annotation_span,  // span of the TypeExpr in the source
            value.span(),     // span of the initializer expression
            &name_str,
            &ann_ty_str,
            &val_ty_str,
        )));
    }
}
```

The `annotation_span` is available from the `TypeExpr` node in the AST (`annotation.span()`). The `name_str` is already resolved at this point.

**New diagnostic constructor:**

```rust
pub fn let_annotation_type_mismatch(
    file:        String,
    ann_span:    Span,    // secondary: "annotated as X"
    value_span:  Span,    // primary: "this is Y"
    name:        &str,
    ann_ty:      &str,
    value_ty:    &str,
) -> Diagnostic
```

**Guard:** Only fires when `hm_expr_type_strict_path(value)` returns `Known` with a concrete, non-`Any` type. When the value type is `Any` or unresolved, the existing `validate_expr_expected_type_with_policy` continues as before.

---

### 5.3 Gap C — Function Return Dual-Label via Dedicated Diagnostic Constructor

**Current path:**

PASS 2 `statement.rs:438` calls `validate_expr_expected_type_with_policy(expected_ret, expression, ...)`. Same generic output as Gap B.

**Proposed change:**

At `statement.rs:438`, replace with a dedicated constructor call when the return expression type is known:

```rust
if let HmExprTypeResult::Known(body_infer) = self.hm_expr_type_strict_path(expression) {
    if !types_compatible(&body_infer, &expected_ret) {
        let body_ty_str = display_infer_type(&body_infer, &self.interner);
        let ret_ty_str  = display_infer_type(&expected_ret, &self.interner);
        return Err(Self::boxed(fun_return_annotation_mismatch(
            self.file_path.clone(),
            ret_annotation_span,  // span of the `-> Type` in the source
            expression.span(),    // span of the return expression
            &fn_name_str,
            &ret_ty_str,
            &body_ty_str,
        )));
    }
}
```

The `ret_annotation_span` is the span of the `TypeExpr` in `return_type: Option<TypeExpr>`. The `fn_name_str` is resolved from `name` at the top of `compile_function`.

**New diagnostic constructor:**

```rust
pub fn fun_return_annotation_mismatch(
    file:           String,
    ret_ann_span:   Span,   // secondary: "declared to return X"
    return_expr_span: Span, // primary: "this is Y"
    fn_name:        &str,
    declared_ty:    &str,
    actual_ty:      &str,
) -> Diagnostic
```

**Guard:** Same as Gap B — only fires when HM gives a concrete, non-`Any` return expression type.

---

## 6. Detailed Design

### 6.1 Call-site argument mismatch (Gap A)

**Output for `greet(42)` where `fn greet(name: String)`:**

```
-- compiler error[E300]: TYPE MISMATCH --------------------- test.flx

The 1st argument to `greet` has the wrong type:

3 |     greet(42)
  |           ^^ this is Int

The function `greet` expects a String as the 1st parameter:

1 | fn greet(name: String) -> Unit {
  |               ^^^^^^
```

**Output for anonymous callees (`(get_fn())(42, "x")`):**

```
-- compiler error[E300]: TYPE MISMATCH --------------------- test.flx

The 1st argument to this function has the wrong type:

3 |     (get_fn())(42, "x")
  |                ^^ this is Int, but the function expects String
```

No secondary label when the definition span is not statically known.

**Files:** `src/ast/type_infer.rs` (`infer_call`), `src/diagnostics/compiler_errors.rs`

---

### 6.2 Let annotation mismatch (Gap B)

**Output for `let x: Int = "hello"`:**

```
-- compiler error[E300]: TYPE MISMATCH --------------------- test.flx

The value of `x` does not match its type annotation:

1 | let x: Int = "hello"
  |        ^^^   ^^^^^^^ this is String
  |        |
  |        but `x` was annotated as Int

Hint: Either change the annotation to `String`, or change the value to an Int.
```

**Output for `let counter: Int = true`:**

```
-- compiler error[E300]: TYPE MISMATCH --------------------- test.flx

The value of `counter` does not match its type annotation:

1 | let counter: Int = true
  |              ^^^   ^^^^ this is Bool
  |              |
  |              but `counter` was annotated as Int
```

**Files:** `src/bytecode/compiler/statement.rs`, `src/diagnostics/compiler_errors.rs`

---

### 6.3 Function return type mismatch (Gap C)

**Output for `fn add(a: Int, b: Int) -> Int { "oops" }`:**

```
-- compiler error[E300]: TYPE MISMATCH --------------------- test.flx

The return value of `add` does not match its declared return type:

3 |     "oops"
  |     ^^^^^^ this is String

But `add` was declared to return Int:

1 | fn add(a: Int, b: Int) -> Int {
  |                            ^^^
```

**Files:** `src/bytecode/compiler/statement.rs`, `src/diagnostics/compiler_errors.rs`

---

## 7. New Diagnostic Constructors

| Constructor | E-code | Trigger |
|---|---|---|
| `call_arg_type_mismatch` | E300 | Argument type ≠ expected param type, with function name and def span |
| `let_annotation_type_mismatch` | E300 | Let initializer type ≠ annotation, with dual span labels |
| `fun_return_annotation_mismatch` | E300 | Return expression type ≠ declared return type, with dual span labels |

All follow the existing builder pattern:
```rust
diag_enhanced(&TYPE_UNIFICATION_ERROR)
    .with_file(...)
    .with_span(...)           // primary label
    .with_message(...)
    .with_secondary_label(...)
    .with_help(...)
```

---

## 8. Coordination with 057's `unify_propagate`

Gaps B and C live at the PASS 2 level, which is the existing design: `unify_propagate` in the HM pass is intentionally silent for annotations so the PASS 2 boundary checker remains authoritative (avoids double-reporting). The proposed changes improve the PASS 2 reporter itself — no change to HM's `unify_propagate` is needed.

Gap A operates at the HM level (`infer_call`) which fires before PASS 2 and is the correct level for structural type mismatch during inference. The existing `suppress_overlapping_hm_diagnostics()` deduplication ensures no double-reporting if PASS 2 also fires for the same call.

---

## 9. Guards and False Positive Prevention

| Case | Guard | Behavior |
|---|---|---|
| Call with untyped callee (Any) | `!fn_ty.contains_any()` (already in `unify_with_context`) | No error — gradual typing |
| Let binding with unresolved value | `HmExprTypeResult::Known` only | Falls through to `validate_expr_expected_type_with_policy` |
| Return expression unknown at compile time | `HmExprTypeResult::Known` only | Falls through to existing path |
| Named callee not in scope | `env.lookup_span` returns `None` | Omits secondary label, still emits primary |
| Multi-arg call with all-Any params | `contains_any()` per-param guard in per-param unification loop | No error |

---

## 10. Files Modified

| File | Change |
|---|---|
| `src/ast/type_infer.rs` | Add `ReportContext::CallArg`; restructure `infer_call` to per-param unification; pass call context |
| `src/types/type_env.rs` | Add `lookup_span(name) -> Option<Span>` to `TypeEnv` for definition-site secondary labels |
| `src/diagnostics/compiler_errors.rs` | Add `call_arg_type_mismatch`, `let_annotation_type_mismatch`, `fun_return_annotation_mismatch` |
| `src/bytecode/compiler/statement.rs` | Replace `validate_expr_expected_type_with_policy` with dedicated constructors for let-annotation and function-return paths |

---

## 11. Implementation Plan (Task Breakdown)

### T0 — Baseline & Guardrails (No behavior change)
- **Goal:** Freeze pre-058 behavior and lock non-regression policy.
- **Files:** proposal sections 11–17, current tests/snapshots.
- **Changes:** lock class boundaries (`E300` for new paths only), one-task-per-PR, intentional snapshot policy.
- **Tests:** baseline gate from §16.
- **Risk:** Low.
- **Done When:** baseline outcomes and guardrails are recorded and reproducible.

### T1 — Diagnostic Constructors Foundation
- **Goal:** Add new constructor APIs without callsite activation.
- **Files:** `src/diagnostics/compiler_errors.rs`.
- **Changes:** add `call_arg_type_mismatch`, `let_annotation_type_mismatch`, `fun_return_annotation_mismatch` with deterministic messages/labels/help.
- **Tests:** constructor shape tests in `tests/error_codes_registry_tests.rs`.
- **Risk:** Low.
- **Done When:** constructors compile with stable output contracts and no behavior wiring.

### T2 — Let Annotation Dual-Label Wiring (PASS 2)
- **Goal:** Replace generic typed-let mismatch with contextual dual-label `E300`.
- **Files:** `src/bytecode/compiler/statement.rs`.
- **Changes:** for typed lets, emit `let_annotation_type_mismatch` when HM strict type is known + concrete + non-`Any`; keep existing fallback path otherwise.
- **Tests:** fixtures `106_let_annotation_int_string.flx`, `107_let_annotation_bool_int.flx`; compiler rules assertions for annotation/value labels.
- **Risk:** Low.
- **Done When:** `let x: Int = "hello"` shows annotation and initializer spans deterministically.

### T3 — Function Return Annotation Dual-Label Wiring (PASS 2)
- **Goal:** Replace generic return mismatch with named dual-label `E300`.
- **Files:** `src/bytecode/compiler/statement.rs`.
- **Changes:** emit `fun_return_annotation_mismatch` when return expression HM type is known + concrete + non-`Any`; preserve existing fallback path otherwise.
- **Tests:** fixtures `108_fun_return_string_vs_int.flx`, `109_fun_return_bool_vs_unit.flx`; compiler rules assertions for function name + return annotation span.
- **Risk:** Low.
- **Done When:** `fn f() -> Int { "oops" }` reports contextual dual-label return mismatch.

### T4 — TypeEnv Definition-Span Plumbing
- **Goal:** Make definition spans available for named call diagnostics.
- **Files:** `src/types/type_env.rs` and minimal HM binding callsites.
- **Changes:** store optional definition span with type bindings; add `lookup_span`.
- **Tests:** type-env unit tests for span registration + lookup.
- **Risk:** Medium.
- **Done When:** named function symbols resolve stable definition spans from HM environment.

### T5 — Call-Site Argument Contextual Diagnostics (HM Path)
- **Goal:** Emit argument-indexed, argument-span-precise call mismatch diagnostics.
- **Files:** `src/ast/type_infer.rs`, `src/diagnostics/compiler_errors.rs`.
- **Changes:** refactor `infer_call` to per-param unification; emit `call_arg_type_mismatch` on mismatch with argument primary span, optional function name, optional def-site secondary label.
- **Tests:** fixtures `110_call_arg_named_fn.flx`, `111_call_arg_anonymous_fn.flx`; HM + compiler rules checks for index, named/anonymous behavior, def-span optionality.
- **Risk:** High.
- **Done When:** `greet(42)` names the function and points to definition; anonymous callees show primary-only contextual error.

### T6 — False-Positive and Fallback Hardening
- **Goal:** Guarantee no contextual noise for unresolved/gradual (`Any`) paths.
- **Files:** `tests/type_inference_tests.rs`, `tests/compiler_rules_tests.rs`.
- **Changes:** add negative tests for unresolved/`Any` call/let/return paths; confirm fallback diagnostics remain active when strict HM typing is unavailable.
- **Tests:** targeted guard tests only.
- **Risk:** Medium.
- **Done When:** guarded non-concrete cases suppress contextual variants by design.

### T7 — Fixtures, README, Snapshot Lock
- **Goal:** Freeze fixture matrix and intentional snapshot deltas.
- **Files:** `examples/type_system/failing/{106..111}.flx`, `examples/type_system/failing/README.md`, snapshots.
- **Changes:** add/update fixture docs + runnable commands; accept intentional-only snapshot diffs.
- **Tests:** `snapshot_diagnostics`, `purity_vm_jit_parity_snapshots`.
- **Risk:** Low.
- **Done When:** fixture inventory is complete and snapshot churn is intentional-only.

### T8 — Proposal Closure and Evidence
- **Goal:** Make 058 auditable and merge-ready.
- **Files:** this proposal sections 11–17.
- **Changes:** add command outcomes, dependency graph, accepted snapshot rationale, parity status summary.
- **Tests:** rerun full §16 gate.
- **Risk:** Low.
- **Done When:** proposal status is implementation-complete with evidence.

---

## 12. Task Dependencies

```
T0 -> T1 -> (T2, T3, T4) -> T5 -> T6 -> T7 -> T8
```

- T2/T3/T4 are parallelizable once constructor foundations are in place.
- T5 depends on T4 span plumbing and stable PASS 2 contextual behavior.
- T7 is the only snapshot-acceptance lock task.

---

## 13. Fixtures

### Failing (error expected)
- `examples/type_system/failing/106_let_annotation_int_string.flx` — dual-label typed-let mismatch (`Int` annotation vs `String` value)
- `examples/type_system/failing/107_let_annotation_bool_int.flx` — dual-label typed-let mismatch (`Bool` annotation vs `Int` value)
- `examples/type_system/failing/108_fun_return_string_vs_int.flx` — dual-label return mismatch (`Int` declared vs `String` returned)
- `examples/type_system/failing/109_fun_return_bool_vs_unit.flx` — dual-label return mismatch (`Bool` declared vs non-`Bool` expression)
- `examples/type_system/failing/110_call_arg_named_fn.flx` — call arg mismatch naming callee and pointing to def site
- `examples/type_system/failing/111_call_arg_anonymous_fn.flx` — anonymous call arg mismatch without def-site secondary label

---

## 14. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| `infer_call` per-param restructure regresses HM behavior | keep arity handling in `E056` compile path, add focused HM/compile tests before snapshot updates |
| Duplicate diagnostics between HM and PASS 2 | retain existing overlap suppression and keep callsite contextual emission isolated to call-arg path |
| Span plumbing introduces environment lookup regressions | add `TypeEnv` lookup-span unit coverage and minimal touch to non-function bindings |
| Contextual emissions on unresolved/`Any` values | enforce concrete + deep-`Any` guards and add negative tests in T6 |
| Snapshot churn beyond scope | accept intentional-only diffs with per-path rationale in T7 |

---

## 15. Diagnostics Compatibility

- All 058 additions remain in `E300` (`TYPE UNIFICATION ERROR`) family.
- No changes to `E015` / `E083` routing or exhaustiveness classes.
- `validate_expr_expected_type_with_policy` remains fallback for unresolved/non-concrete paths.
- Existing decomposed function mismatch constructors remain valid for non-call contexts.
- New `call_arg_type_mismatch` is call-context-specific and does not replace general function-type decomposition globally.

---

## 16. Test Plan

### Full Gate
```bash
cargo check --all --all-features
cargo test --test type_inference_tests
cargo test --test compiler_rules_tests
cargo test --test snapshot_diagnostics
cargo test --all --all-features purity_vm_jit_parity_snapshots
cargo test --all --all-features --test runtime_vm_jit_parity_release
```

### Snapshot Note (71/72)
- Parity snapshots for fixtures `71_hm_if_known_type_compile_mismatch.flx` and `72_hm_match_known_type_compile_mismatch.flx` intentionally shifted from generic function-param wording to call-site contextual wording (`call_arg_type_mismatch`).
- Expected-type visibility remains explicit in rendered diagnostics via call-argument guidance (`Expected \`<T>\` as the <n> argument.`) and/or definition-site secondary labels when available.

### Focused Evidence Runs
```bash
cargo run -- --no-cache examples/type_system/failing/106_let_annotation_int_string.flx
cargo run -- --no-cache examples/type_system/failing/108_fun_return_string_vs_int.flx
cargo run -- --no-cache examples/type_system/failing/110_call_arg_named_fn.flx
cargo run --features jit -- --no-cache examples/type_system/failing/110_call_arg_named_fn.flx --jit
```

---

## 17. Acceptance Criteria

1. `let x: Int = "hello"` reports contextual `E300` with annotation + value spans.
2. `fn add() -> Int { "oops" }` reports contextual `E300` with return annotation + expression spans.
3. `greet(42)` reports contextual `E300` with argument-indexed message and def-site secondary label.
4. Anonymous call mismatches report contextual `E300` without requiring function-name/def-site metadata.
5. Unresolved/`Any` cases do not emit new contextual false positives.
6. Full gate and parity suites pass with intentional-only snapshot diffs.
