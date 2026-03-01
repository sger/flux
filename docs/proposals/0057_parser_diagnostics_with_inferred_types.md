- Feature Name: Rich Diagnostics with Inferred Types — Elm-style Errors & Generic Parser
- Start Date: 2026-02-27
- Proposal PR: 
- Flux Issue: 

# Proposal 0057: Rich Diagnostics with Inferred Types — Elm-style Errors & Generic Parser

## Summary
[summary]: #summary

Improve Flux diagnostics by leveraging HM-inferred type information to produce rich, contextual, human-readable error messages — inspired by Elm's diagnostic philosophy. This proposal covers six diagnostic improvements *and* two architectural upgrades (a context-aware unification error system and a generic parser recovery API) that make future improvements easy to add without per-case ad-hoc wiring.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Catch arity mismatches at compile time.
2. Detect if/else branch type mismatches with dual source labels.
3. Detect match arm type inconsistencies with dual source labels.
4. Report multiple independent type errors per file.
5. Decompose function-type mismatches into specific sub-errors (param position, return type).
6. Recover gracefully from malformed type annotations without losing the surrounding statement.
7. Introduce a `ReportContext` architecture so all future type errors automatically get rich context.
8. Introduce generic parser helpers (`parse_type_annotation_opt`, `parse_required`) that centralise recovery.

### 4. Non-Goals

1. No new syntax or grammar changes.
2. No runtime type checking changes.
3. No changes to the HM inference algorithm itself (unification rules are unchanged).
4. No effect system enforcement (separate proposal track).
5. No typed-AST migration (proposal 046).

### 3. Goals

### 4. Non-Goals

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **2.1 Current Error Quality:** Today, most type errors produce a single terse message with no secondary labels, no context about *what the expression is for*, and no actionabl...
- **2.1 Current Error Quality:** Today, most type errors produce a single terse message with no secondary labels, no context about *what the expression is for*, and no actionable hint: ``` -- compiler error[E30...
- **2.2 The Elm Standard:** Elm's error messages answer these questions proactively: ``` -- TYPE MISMATCH ------------------------------------ src/Main.elm
- **2.3 Diagnostic Gaps:** 1. **Arity mismatches are runtime-only** — `add(1, 2, 3)` panics at E1000 instead of being caught at compile time. 2. **If/else branch type mismatches are silent** — `if true {...
- **2.4 Architectural Gaps:** - **No diagnostic context system** — `type_unification_error` is a single function that produces an identical generic message for every call site. There is no way to attach sema...
- **5.1 Context-Aware Unification: `ReportContext`:** **Problem:** `unify_reporting` is called from many sites but always emits the same `type_unification_error`. There is nowhere to attach "this is an if-branch mismatch" vs. "this...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. No new syntax or grammar changes.
2. No runtime type checking changes.
3. No changes to the HM inference algorithm itself (unification rules are unchanged).
4. No effect system enforcement (separate proposal track).
5. No typed-AST migration (proposal 046).

### 4. Non-Goals

### 14. Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Branch/arm checks produce false positives on untyped code | Only emit when both types are fully concrete and non-`Any` |
| Arity check fires on dynamic dispatch | Guard on `HmExprTypeResult::Known` only; skip if callee is not an identifier |
| Multi-error continuation causes misleading cascade | `Any` fallback in HM after every failure; only concrete-vs-concrete errors fire |
| Type annotation recovery consumes wrong tokens | Conservative sync points: `=`, `)`, `{`, `}`, EOF only |
| Fun decomposition adds detail to unify_many internals | `detail` is set at the Fun arm level, not inside `unify_many`; safe boundary |
| Lexer recovery for unclosed string misaligns token stream | Synthesize close-quote only at newline boundary, not mid-token; retest with existing snapshot suite |
| `suggest_effect_name` false positives on user-defined effect names | Only suggest when the name matches a known built-in effect within edit distance 1; skip user ADT names |
| Cross-module `Any` from errored dependency masks real errors in dependents | This is intentional — false positives are worse than false negatives here; document the tradeoff |

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
