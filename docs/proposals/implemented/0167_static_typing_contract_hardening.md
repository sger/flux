- Feature Name: Static Typing Contract Hardening
- Start Date: 2026-04-20
- Status: Implemented (2026-04-22)
- Proposal PR:
- Flux Issue:
- Depends on: current HM/Core pipeline, [0157](../0157_remove_dynamic_from_maintained_paths.md), [0158](../0158_static_typing_contract_enforcement.md), [0159](../0159_polymorphic_recursion_signature_guided.md), [0164](0164_internal_primop_contract_and_stdlib_surface.md) where relevant

# Proposal 0167: Static Typing Contract Hardening

## Implementation Summary

Landed across two commits (April 21–22).

**Commit `ca6f79ca` — infrastructure (April 21):**

- `src/ast/type_infer/boundary.rs` — `BoundaryKind` enum with six
  variants (`PublicFunctionSignature`, `AnnotatedLet`, `AnnotatedReturn`,
  `EffectBoundary`, `ModuleInterfaceBoundary`, `BackendConcreteBoundary`)
  plus `BoundaryViolation` carrier and stable user-facing labels.
- `src/diagnostics/ranking.rs` — unified overlap-based
  `is_suppressed_by` / `spans_related` helper replacing the per-pass
  "span overlap OR same line" heuristics in `hm_expr_typer` and
  `static_type_validation`. Same-line-disjoint no longer suppresses.
- `src/core/passes/static_contract.rs` — Core-adjacent walker that
  inspects each `CoreDef::result_ty` for free variables not enclosed by
  a `Forall`, wired into the semantic core-pass pipeline as Stage 1b.
- `src/ast/type_infer/static_type_validation.rs` — `is_illegal_residue`
  three-conjunct rule: a type variable is illegal residue iff it is not
  in `allowed_generalized_vars`, not in `instantiated_expr_vars`, and
  tagged in `fallback_vars`. The `fallback_vars` conjunct is retained
  with a documented rationale (mutual-recursion groups).
- `src/syntax/expression.rs` — `ExprIdGen::resuming_past_program` /
  `resuming_past_statements` replace the hardcoded 1_000_000 sentinel
  for compiler-generated AST (class dispatch, synthetic wrappers).
- Infinite-type diagnostic already present as **E301 `OCCURS_CHECK_FAILURE`**
  with "infinite type" user-facing wording (the proposal's suggested
  `E306` is subsumed by the existing code).
- Typed-let strict path cleanup in `src/compiler/statement.rs`:
  `block_has_typed_let_error` now only triggers AST fallback on
  diagnosable errors (E300 mismatch, E425 unresolved, unknown annotation
  constructor); well-typed annotated lets proceed through CFG.

**Part 1 consumption + Part 7 promotion (April 22):**

- `StrictTypeValidator` now carries a `current_boundary: BoundaryKind`
  field threaded via `with_boundary` at every relevant entry point:
  `Statement::Function` (per `is_public` / `return_type`),
  `Statement::Let` (when `type_annotation.is_some()`), `Statement::Return`,
  and `Expression::Perform`. Both emission sites (binding-level and
  expression-level E430) now include the boundary label in the user-facing
  message via `kind.label()`.
- `src/core/passes/static_contract.rs` `build_violation` tags emissions
  as `BackendConcreteBoundary` and sets `Severity::Error` explicitly.
- `src/core/passes/mod.rs` removes the `FLUX_CORE_CONTRACT_WARN` opt-in
  and emits by default; `FLUX_CORE_CONTRACT_SILENT` remains as a
  rollout escape hatch.
- `src/compiler/mod.rs` `is_expression_level_e430` predicate loosened:
  drops the trailing period from the message-prefix match so the new
  " at the {boundary}" infix still participates in suppression.
- `validate_statement` split into `validate_function_statement` and
  `validate_let_statement` helpers to stay within the
  `type_infer_function_complexity_budget` guard.

**Part-by-part status:**

- Part 1 (boundary classification): enum + consumption — user-visible
  diagnostics carry the label.
- Part 2 (infinite type): already landed as **E301**.
- Part 3 (unresolved-boundary rule): `is_illegal_residue` three-conjunct.
- Part 4 (reduce AST fallback): typed-let happy path on CFG; remaining
  AST fallbacks are specialised-diagnostic paths only.
- Part 5 (unified suppression): `ranking::is_suppressed_by` consumed
  by both validators; same-line disjoint no longer related.
- Part 6 (globally unique ExprId): `resuming_past_program`.
- Part 7 (Core-adjacent contract): default-on, **Error severity**,
  `FLUX_CORE_CONTRACT_SILENT` escape hatch.

**Tests:** `tests/type_inference/boundary_contract_tests.rs` covers the
label vocabulary, strict-mode typed-let acceptance, the ranking policy,
`ExprIdGen::resuming_past_program`, the residue rule, and the Core
contract pass entry point (`core_contract_violation_includes_boundary_label`
confirms the `BackendConcreteBoundary` label flows into the emitted
diagnostic).

## Summary
[summary]: #summary

Strengthen Flux static typing by making HM/Core the single maintained authority for static boundary checks, hardening unresolved-type rejection, rejecting infinite types explicitly, reducing AST fallback in strict typing paths, and unifying static diagnostic suppression policy. This proposal is about the static typing contract itself, not new type-system surface syntax.

## Motivation
[motivation]: #motivation

Flux already has a strong type-system core:

- rank-1 HM inference with real generalization and instantiation
- type classes and dictionary elaboration
- row-polymorphic effects
- explicit Core IR as the semantic checkpoint
- `Dynamic`/`Any` removed from maintained semantic paths

But the remaining weaknesses are no longer about “having a type system.” They are about making the static typing contract trustworthy and architecturally clean.

### Problem 1: static typing still has multiple authorities

Important checks still fall back to AST-specific paths rather than staying on the maintained HM/Core route. That means “statically accepted by Flux” is not always defined by one semantic pipeline.

### Problem 2: unresolved-type rejection is too implementation-shaped

Today unresolved residue is filtered through metadata such as fallback vars. That helps, but it is not the semantic question we really care about.

The real question is:

- does a boundary that must be concrete still contain illegal free type variables?

### Problem 3: infinite types are not surfaced explicitly

Recursive unifications should be diagnosed directly as infinite types. Silent cycle-breaking or indirect fallback behavior weakens the contract and makes debugging harder.

### Problem 4: diagnostics are suppressed by inconsistent local heuristics

Different passes suppress overlapping static typing errors differently:

- exact span
- line-based overlap
- local error-code filtering

This makes diagnostics unstable and hard to reason about.

### Problem 5: compiler-generated expressions can still influence static correctness

Recent regressions showed that generated class-dispatch AST identity could collide in ways that affected HM expression typing. That class of bug needs to be closed structurally.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### The user model

After this proposal, Flux’s static typing story should be simple:

- HM inference determines types
- explicit boundaries decide where types must be concrete
- if a boundary still contains illegal free type variables, Flux rejects it
- Core debug output is the maintained semantic debugging surface

Users should not need to know whether a given check was “AST-only” or “CFG-path-only.”

### What counts as a concrete boundary

This proposal treats the following as concrete static boundaries:

- public function signatures in strict mode
- annotated let bindings
- annotated returns
- effect operation boundaries
- module interface serialization boundaries
- backend/lowering boundaries that require concrete representation

At these boundaries, unresolved residue is not acceptable unless it is legitimate quantified polymorphism.

### Example: infinite type

```flux
fn bad(x) {
    x(x)
}
```

Flux should report a dedicated infinite-type diagnostic, not silently degrade into later residue or fallback behavior.

### Example: strict unresolved boundary

```flux
public fn f(x) {
    x
}
```

In strict mode, if the public boundary still contains unresolved illegal residue, Flux should reject it through one canonical boundary-check rule.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

## Scope

This proposal covers:

1. explicit static boundary classification
2. infinite-type diagnostics
3. unresolved-boundary hardening
4. diagnostic suppression unification
5. compiler-generated expression identity robustness
6. reduction of AST fallback for strict typing enforcement

It does **not** add:

- bidirectional typing
- higher-rank types
- top-level signature syntax

Those remain future work.

## Part 1: explicit static boundary categories

Introduce one internal boundary classification surface used by typing and validation code.

Suggested categories:

- `PublicFunctionSignature`
- `AnnotatedLet`
- `AnnotatedReturn`
- `EffectBoundary`
- `ModuleInterfaceBoundary`
- `BackendConcreteBoundary`

This should be an internal compiler notion, not a user-visible feature.

The point is to stop each pass from inventing its own definition of “strict boundary.”

## Part 2: infinite-type diagnostic

When unification encounters a recursive type equation such as:

- `?t = ?t -> Int`

Flux should emit a dedicated infinite-type error instead of silently breaking or laundering the cycle into later fallback behavior.

This should be implemented in the unifier, not as a late-phase residue check.

Suggested diagnostic:

- `E306 Infinite Type`

The message should explicitly say that the type would need to contain itself.

## Part 3: unresolved-boundary hardening

Refine the static validator so a type is invalid at a concrete boundary iff:

- it still contains free variables
- those variables are not legitimately quantified for that binding
- they are not legal instantiation-local placeholders

`fallback_vars` may still be useful supporting metadata, but it must not be the sole truth source for whether residue is illegal.

The validator should answer a semantic question, not only an inference bookkeeping question.

## Part 4: reduce AST fallback in strict typing paths

Typed `let` and similar strict typing checks should stop being a standing reason to leave the maintained path.

The target architecture is:

- HM inference
- boundary validation
- Core lowering

not:

- HM inference
- sometimes AST fallback
- sometimes maintained path

In particular, strict annotated-let validation should move fully onto HM-resolved type comparison rather than relying on AST fallback branches.

## Part 5: unify diagnostic suppression

Static typing passes should use one shared ranking/suppression rule.

Preferred ranking:

- narrower span beats wider span
- concrete mismatch beats follow-on unresolved residue
- existing primary error beats later residual noise

What should be removed:

- line-wide suppression heuristics for type diagnostics
- pass-local special cases where possible

## Part 6: robust generated expression identity

All compiler-generated AST expressions that participate in HM-typed validation must receive globally unique expression IDs from one shared allocator.

Do not rely on:

- `ExprId::UNSET`
- hardcoded reserved ranges
- per-generated-body local counters

This applies especially to:

- class-dispatch synthesis
- generated wrappers
- any future synthetic AST inserted before or during HM-sensitive paths

## Part 7: Core-adjacent static contract validation

Strengthen the maintained path by validating static boundary contracts close to Core-visible function/binding structure.

This does not mean re-running HM on Core.

It means:

- use HM-resolved types plus Core-visible definitions
- assert that concrete boundaries do not still carry illegal unresolved residue
- keep Core as the main semantic debugging surface

## Diagnostics

This proposal is expected to refine or add diagnostics including:

- infinite type
- unresolved boundary type
- better boundary-local mismatch reporting

Diagnostics should remain stable enough for snapshot testing and should not leak inference-internal IDs.

## Drawbacks
[drawbacks]: #drawbacks

- This proposal touches multiple compiler layers rather than one local subsystem.
- Some existing tests and snapshots will need careful rebaselining.
- Tightening unresolved-boundary rules may surface latent bugs that currently pass.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not only improve diagnostics

Because the issue is not just messaging. The static contract itself is still partially split across multiple enforcement paths.

### Why not jump straight to bidirectional checking

Bidirectional checking would help, but it is a larger strategic rewrite. The current proposal focuses on making the existing HM/Core architecture more trustworthy first.

### Why not leave typed-let fallback alone

Because as long as strict typing depends on AST-only escape hatches, the maintained path is not the true authority.

## Prior art
[prior-art]: #prior-art

Typed functional languages typically separate:

- inference
- unification failure
- exhaustiveness / boundary validation
- IR lowering

and treat infinite types as direct errors rather than late residue.

Flux should follow that structural lesson without changing its core language direction.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Final diagnostic code choices for infinite-type and any refined unresolved-boundary cases.
- Exact placement of the shared boundary classification and diagnostic ranking helpers.
- Whether any AST fallback must remain temporarily for non-typing reasons during migration.

## Future possibilities
[future-possibilities]: #future-possibilities

- Bidirectional type checking for better local error quality.
- Top-level signature declarations.
- Higher-rank type checking once boundary semantics and expected-type pushdown are stronger.
- Stronger Core contract validation surfaces for tooling and IDEs.
