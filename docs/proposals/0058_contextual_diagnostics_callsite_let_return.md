- Feature Name: Contextual Diagnostics — Call-Site Arguments, Let Annotations, and Function Return Types
- Start Date: 2026-02-28
- Proposal PR: 
- Flux Issue: 

# Proposal 0058: Contextual Diagnostics — Call-Site Arguments, Let Annotations, and Function Return Types

## Summary
[summary]: #summary

Proposal 0057 introduced the `ReportContext` architecture and delivered contextual diagnostics for if/else branch mismatches, match arm mismatches, and function type decomposition. This proposal completes the contextual diagnostic picture with three remaining high-impact improvements: Proposal 0057 introduced the `ReportContext` architecture and delivered contextual diagnostics for if/else branch mismatches, match arm mismatches, and function type decomposition. This proposal completes the contextual diagnostic picture with three remaining high-impact improvements:

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Name the called function in argument-mismatch diagnostics and show the definition site.
2. Show both the annotation span and the value span in let-binding type mismatches.
3. Show both the return annotation span and the return expression span in function return mismatches.
4. No new architectural changes — all three improvements reuse 057's infrastructure.
5. No false positives on untyped/gradual code — all guards already in place from 057.

### 4. Non-Goals

1. No changes to HM unification rules.
2. No changes to the PASS 2 multi-error continuation (already done in 057).
3. No improvements to effect/purity diagnostics (separate track).
4. No let-destructuring pattern diagnostics (separate from simple let).

### 3. Goals

### 4. Non-Goals

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **2.1 Current State After 057:** After proposal 057, the following diagnostic classes are rich and contextual: - `if`/`else` branch mismatch — ✅ dual labels, Elm-style - `matc...
- **2.1 Current State After 057:** After proposal 057, the following diagnostic classes are rich and contextual: - `if`/`else` branch mismatch — ✅ dual labels, Elm-style - `match` arm mismatch — ✅ dual labels, El...
- **2.2 Gap A — Call-site argument mismatch: missing function name and def span:** `greet(42)` where `fn greet(name: String)` currently emits via `fun_param_type_mismatch` (which fires because `UnifyErrorDetail::FunParamMismatch` is set in `infer_call`), but t...
- **2.3 Gap B — Let annotation mismatch: generic message, no annotation span:** `let x: Int = "hello"` is checked in PASS 2 (`statement.rs:128`) via `validate_expr_expected_type_with_policy`. The current output: ``` -- compiler error[E300]: TYPE MISMATCH bi...
- **2.4 Gap C — Function return type mismatch: generic message, no annotation span:** `fn add(a: Int, b: Int) -> Int { "oops" }` is checked in PASS 2 (`statement.rs:438`) via `validate_expr_expected_type_with_policy`. The current output: ``` -- compiler error[E30...
- **5.1 Gap A — Named Call-Site Diagnostics via `ReportContext::CallArg`:** `infer_call` builds `Fun(arg_tys, ret_var, [])` and calls `unify_reporting(&fn_ty, &expected_fn_ty, span)`. When the callee has `Fun([String], _, _)` and the argument is `Int`,...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. No changes to HM unification rules.
2. No changes to the PASS 2 multi-error continuation (already done in 057).
3. No improvements to effect/purity diagnostics (separate track).
4. No let-destructuring pattern diagnostics (separate from simple let).

### 4. Non-Goals

### 14. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| `infer_call` per-param restructure regresses HM behavior | keep arity handling in `E056` compile path, add focused HM/compile tests before snapshot updates |
| Duplicate diagnostics between HM and PASS 2 | retain existing overlap suppression and keep callsite contextual emission isolated to call-arg path |
| Span plumbing introduces environment lookup regressions | add `TypeEnv` lookup-span unit coverage and minimal touch to non-function bindings |
| Contextual emissions on unresolved/`Any` values | enforce concrete + deep-`Any` guards and add negative tests in T6 |
| Snapshot churn beyond scope | accept intentional-only diffs with per-path rationale in T7 |

### 4. Non-Goals

### 14. Risks and Mitigations

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
