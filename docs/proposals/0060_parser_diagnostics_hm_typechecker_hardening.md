- Feature Name: Parser Recovery Breadth, Diagnostic Precision, HM Hardening, and Type Checker Completeness
- Start Date: 2026-02-28
- Proposal PR: 
- Flux Issue: 

# Proposal 0060: Parser Recovery Breadth, Diagnostic Precision, HM Hardening, and Type Checker Completeness

## Summary
[summary]: #summary

This proposal is a focused hardening pass across four layers of the Flux compiler front-end, building directly on the infrastructure established in proposals 057–059. It does not introduce new language features or new AST nodes. Every item is a diagnostic improvement, a soundness tightening, or a recovery-breadth extension.

## Motivation
[motivation]: #motivation

After 059 the parser produces targeted messages for keyword aliases, missing braces in `if`/`else`, missing `=` in `let`, missing `()` in `fn`, and `|`/`=>` in match arms. However approximately 35 `expect_peek` call sites in `expression.rs` and `statement.rs` still emit generic messages. Similarly, the HM engine contains ~35 `Any` fallback sites; several of them in high-visibility positions (branch disagreement, tuple projection, match scrutinee) are both easy to tighten and high-value for user-facing correctness.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Non-Goals

1. No new syntax or AST nodes.
2. No changes to the runtime value system or GC.
3. No changes to JIT compilation paths except where diagnostics feed through shared compiler infrastructure.
4. No higher-rank polymorphism or theorem-proving exhaustiveness.
5. No record-pattern totality (blocked on proposal 048).
6. No new effect checking rules (owned by 042/049).

### T14 Non-Goals Lock

- No harness changes in T14 closure.
- Harness root broadening/exclusion policy is deferred to a dedicated infrastructure task.

### 3. Non-Goals

### T14 Non-Goals Lock

### 3. Non-Goals

### T14 Non-Goals Lock

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **P1: Named-construct messages for remaining unclosed delimiter sites:** **Problem.** After 059, the following `expect_peek` failure sites still produce the generic `"Expected...
- **P1: Named-construct messages for remaining unclosed delimiter sites:** **Problem.** After 059, the following `expect_peek` failure sites still produce the generic `"Expected X, got Y"` message with no construct name and no correct-syntax hint: | Si...
- **P2: Contextual `->` arrow errors in match arms and lambdas:** **Problem.** When `->` is missing in a match arm (not the `=>` case already handled by 059), the parser returns `None` with the generic message "Expected `->`, got `<token>`". T...
- **P3: Orphan pattern diagnostic at statement level:** **Problem.** A match constructor used at the top level (not inside a `match`) reads as an identifier call expression, and the subsequent content produces a cascade. Example: ```...
- **P4: `do` block missing braces:** **Problem.** `do expr` (missing `{`) silently falls through to a generic error.
- **D1: Span precision for multi-token construct labels:** **Problem.** Several diagnostics in `hm_expr_typer.rs` and `type_infer.rs` attach the error label to the whole enclosing expression rather than the specific subterm that caused...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 3. Non-Goals

1. No new syntax or AST nodes.
2. No changes to the runtime value system or GC.
3. No changes to JIT compilation paths except where diagnostics feed through shared compiler infrastructure.
4. No higher-rank polymorphism or theorem-proving exhaustiveness.
5. No record-pattern totality (blocked on proposal 048).
6. No new effect checking rules (owned by 042/049).

### T14 Non-Goals Lock

- No harness changes in T14 closure.
- Harness root broadening/exclusion policy is deferred to a dedicated infrastructure task.

### 3. Non-Goals

### T14 Non-Goals Lock

### 3. Non-Goals

### T14 Non-Goals Lock

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

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
- This is a subset of `0051_any_fallback_reduction.md` scoped to the recursive self-reference site.
- Fixtures 140–141: recursive `sum` infers `Int`; recursive `map` infers `Array<B>` given body constraints.

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

### H4: Recursive self-reference type propagation

### T11 — H4 Recursive Self-Reference Propagation

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
