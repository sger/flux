- Feature Name: Static Typing Contract Hardening
- Start Date: 2026-04-20
- Status: Proposed
- Proposal PR:
- Flux Issue:
- Depends on: current HM/Core pipeline, [0157](0157_remove_dynamic_from_maintained_paths.md), [0158](0158_static_typing_contract_enforcement.md), [0159](0159_polymorphic_recursion_signature_guided.md), [0164](0164_internal_primop_contract_and_stdlib_surface.md) where relevant

# Proposal 0167: Static Typing Contract Hardening

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
