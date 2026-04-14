- Feature Name: Static Typing Completion Roadmap
- Start Date: 2026-04-14
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: 0123 (Full Static Typing), 0147 (Constrained Type Params and Instance Contexts)

# Proposal 0156: Static Typing Completion Roadmap

## Summary
[summary]: #summary

Complete Flux's transition from a gradual HM system with `Any` escape hatches to a genuinely static type system across parsing, inference, Core lowering, module boundaries, and runtime type translation.

This proposal does **not** restart static typing from scratch. It treats the current repo state as the baseline:

- typed Core/Aether/native infrastructure exists
- operator desugaring is implemented
- HKT instance resolution is implemented
- base HM signature tightening is implemented
- type classes, deriving, and dictionary elaboration exist

The remaining work is concentrated in five areas:

1. finish the remaining source-level class semantics
2. eliminate `Any` from HM fallback paths
3. make strict typing part of inference semantics, not just a post-check
4. remove stdlib and module-boundary escape hatches
5. close structural typing gaps that still leak `Any`

## Motivation
[motivation]: #motivation

Flux is much more statically typed than it used to be, but the current repo still has a split personality:

- **the proposal/docs story** says full static typing is complete
- **the implementation reality** still contains a live gradual escape hatch via `Any`

The main evidence is direct:

- unification still treats `Any` as compatible with everything in [unify.rs](/Users/s.gerokostas/Downloads/Github/flux/src/types/unify.rs:21)
- strict typing is implemented as a post-inference binding check in [strict_types.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/strict_types.rs:1)
- many HM inference paths still return `Any` on unsupported or mixed cases
- runtime type lowering still collapses unresolved/high-level forms to `RuntimeType::Any` in [type_env.rs](/Users/s.gerokostas/Downloads/Github/flux/src/types/type_env.rs:354)

This creates three problems:

1. **Static typing is not yet semantic truth**
   - today, strict typing mostly means “reject some residual `Any` after inference”
   - it does not mean “the inference engine itself is non-gradual”

2. **Backend improvements sit on a soft type foundation**
   - typed Core, Aether, and LLVM work are valuable
   - but they still inherit unresolved `Any` zones from HM and runtime translation

3. **Proposal state is stale**
   - some old blockers are already done
   - the real remaining blockers are now concentrated in `Any` elimination and strict typing depth

## Current State
[current-state]: #current-state

### Already implemented

These items were previously on the static-typing critical path but are already done in the repo:

- **0149 operator desugaring** — implemented
  - [0149_operator_desugaring.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/implemented/0149_operator_desugaring.md:1)
- **0150 HKT instance resolution** — complete
  - [0150_hkt_instance_resolution.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/0150_hkt_instance_resolution.md:1)
- **0074 base HM signature tightening** — implemented
  - [0074_base_signature_tightening.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/implemented/0074_base_signature_tightening.md:1)
- **typed Core/Aether/native pipeline work** — largely implemented
  - [0123_full_static_typing.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/implemented/0123_full_static_typing.md:1)

### Still missing

The remaining gaps are now:

- **0147 instance context enforcement and constrained type parameter completion**
  - [0147_constrained_type_params_and_instance_contexts.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/0147_constrained_type_params_and_instance_contexts.md:1)
- **`Any` elimination from HM inference**
- **strict typing as an inference mode instead of only a post-validation mode**
- **deeper expression-level and module-level strict validation**
- **runtime type lowering that no longer silently collapses unresolved forms to `Any`**

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Compiler contributors should think of this proposal as the plan to make static typing **true by construction**, not merely **enforced at the edges**.

After this roadmap is complete:

- strict typing should no longer be “HM inference plus cleanup”
- `Any` should not silently mask unresolved typing inside maintained compiler paths
- the stdlib should type-check under the same strict rules as user code
- Core/Aether/backend work should rely on a truly static upstream contract

The key implementation rule is:

> remove `Any` from semantic fallback paths before adding more downstream typed optimizations.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Phase 0 — Baseline alignment

This phase does not add features. It updates the static-typing roadmap to reflect reality.

#### Goals

- treat the following as complete prerequisites:
  - 0149 operator desugaring
  - 0150 HKT instance resolution
  - 0074 base HM signatures
- stop treating those items as blockers for static typing completion
- re-center the roadmap on:
  - 0147 completion
  - `Any` elimination
  - strict-mode semantics

#### Deliverables

- proposal updates only
- no compiler behavior change

---

### Phase 1 — Complete source-level class semantics

The remaining type-class work is no longer broad infrastructure. It is the last source-language gap that affects strict typing.

#### Scope

1. **Constrained type parameter syntax and enforcement**
   - `fn f<a: Eq>(x: a, y: a) -> Bool`
   - ensure declared constraints are carried into checking and generalized schemes

2. **Instance context enforcement**
   - `instance Eq<a> => Eq<List<a>>`
   - instance method bodies must see and use the context dictionaries implied by the instance head

#### Files

- [0147_constrained_type_params_and_instance_contexts.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/0147_constrained_type_params_and_instance_contexts.md:1)
- likely implementation in:
  - `src/syntax/parser/statement.rs`
  - `src/syntax/type_class.rs`
  - `src/ast/type_infer/*`
  - `src/core/passes/dict_elaborate.rs`
  - `src/types/class_env.rs`
  - `src/types/class_solver.rs`

#### Success condition

- constrained polymorphic functions are expressible and enforced
- instance contexts are semantic, not decorative
- no class-method call in a valid constrained context falls back to an untyped/panic path

---

### Phase 2 — Remove `Any` from HM escape hatches

This is the main static-typing phase.

#### Problem

Today, HM inference still produces `Any` in many places. Representative live sites:

- ADT inference fallbacks:
  - [adt.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/adt.rs:113)
- collection inference:
  - [collections.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/expression/collections.rs:52)
- operator inference:
  - [operators.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/expression/operators.rs:22)
- effect inference:
  - [effects_nodes.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/expression/effects_nodes.rs:22)
- access/member/index inference:
  - [access.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/expression/access.rs:23)
  - [access.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/expression/access.rs:84)
- control-flow joins:
  - [unification.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/unification.rs:77)
- generic expression fallback:
  - [expression/mod.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/expression/mod.rs:61)

#### Work items

1. **Unknown operator handling**
   - replace `Any` fallback with typed diagnostics

2. **Heterogeneous collection handling**
   - heterogeneous arrays/lists should fail with a type error instead of collapsing to `Any`

3. **Branch-join behavior**
   - `join_types` must stop using `Any` as a conflict sink in strict mode
   - mismatched `if`/`match` branches should produce proper diagnostics

4. **Member/index access**
   - non-module member access and unsupported access shapes should produce typed errors
   - they should not silently infer as `Any`

5. **Effect nodes**
   - unresolved `perform`/`handle` typing must become diagnostic, not gradual fallback

6. **ADT decomposition and pattern typing**
   - unknown or unsupported field/pattern typing must no longer bind through `Any`

#### Unifier change

This phase must also change the foundational rule in [unify.rs](/Users/s.gerokostas/Downloads/Github/flux/src/types/unify.rs:101):

```rust
(InferType::Con(TypeConstructor::Any), _) | (_, InferType::Con(TypeConstructor::Any))
```

That rule is the core gradual-typing escape hatch. Full static typing requires:

- either removing it entirely for strict mode
- or structurally forbidding `Any` from reaching unification in strict mode

#### Success condition

- strict typing no longer depends on a live `Any` compatibility rule
- unsupported inference paths emit diagnostics instead of degrading to `Any`

---

### Phase 3 — Make strict typing part of inference semantics

Today, strict typing is primarily a post-pass:

- [strict_types.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/strict_types.rs:1)

It checks top-level binding schemes after HM completes. That is useful, but too shallow.

#### Work items

1. **Inference-mode strictness**
   - thread a strict-typing mode into HM inference itself
   - strict mode should alter fallback behavior during inference, not only after it

2. **Expression-level validation**
   - reject unresolved/gradual subexpressions even when the top-level binding scheme looks clean

3. **Better error provenance**
   - diagnostics should point at the actual unresolved subexpression, not only the surrounding binding name

4. **Boundary-sensitive enforcement**
   - ensure public APIs, instance methods, handler arms, and effect boundaries are checked under the same strict policy

#### Success condition

- strict typing means “inference is non-gradual here”, not “we checked for leftover `Any` later”

---

### Phase 4 — Remove stdlib and module-boundary exclusions

Strict typing is not complete while maintained library code is excluded.

#### Current issue

The current roadmap and code paths still carry special handling for Flow stdlib and module-boundary looseness. The compiler should not permanently rely on stdlib carve-outs.

#### Work items

1. **Remove Flow stdlib strict-type exclusion**
   - after base signatures and class plumbing are strong enough, `lib/Flow/` should type-check under strict mode

2. **Tighten cross-module typing**
   - module interfaces must preserve enough type information that strict mode remains meaningful across module boundaries

3. **Revisit partial module contracts work**
   - integrate remaining useful parts of module contract checking into the static typing story

#### Related areas

- proposal 0039 (module contracts / partial)
- module interface and cache/type boundary code

#### Success condition

- no unconditional stdlib strict-mode carve-out remains
- strict typing remains meaningful across imports and cached module boundaries

---

### Phase 5 — Fix runtime type lowering and structural gaps

Even after HM and Core are stricter, runtime-facing type translation still collapses many forms to `Any`.

#### Current issue

`TypeEnv::to_runtime` still maps several forms to `RuntimeType::Any`:

- unresolved vars
- functions
- HKT apps
- many non-primitive ADT/runtime cases

See:
- [type_env.rs](/Users/s.gerokostas/Downloads/Github/flux/src/types/type_env.rs:354)
- [type_env.rs](/Users/s.gerokostas/Downloads/Github/flux/src/types/type_env.rs:399)

#### Work items

1. **Tighten runtime type translation**
   - make translation preserve more static information where required
   - stop silently collapsing unresolved forms in strict paths

2. **Expression-level strictness on translated boundaries**
   - typed runtime contracts should fail at compile time when static proof is unavailable

3. **Structural typing gaps**
   - evaluate remaining gaps that still force `Any`, including:
     - typed records / named structural field access
     - partial module contract coverage
     - unresolved structural access shapes

#### Success condition

- runtime type lowering no longer reintroduces “hidden graduality” after HM has succeeded

## Detailed gap inventory
[detailed-gap-inventory]: #detailed-gap-inventory

### Gap A — `Any` remains part of the type model

- `InferType::Con(TypeConstructor::Any)` is still first-class in:
  - [infer_type.rs](/Users/s.gerokostas/Downloads/Github/flux/src/types/infer_type.rs:257)
- `contains_any()` is still a core utility:
  - [infer_type.rs](/Users/s.gerokostas/Downloads/Github/flux/src/types/infer_type.rs:272)

This is compatible with gradual typing, but not with claiming that static typing is complete.

### Gap B — strict mode checks only top-level binding schemes

- [strict_types.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/strict_types.rs:17)

This misses:

- unresolved subexpressions hidden inside otherwise-generalized bindings
- localized fallback sites that never surface clearly at the binding boundary

### Gap C — docs and proposal state are ahead of implementation reality

- [0123_full_static_typing.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/implemented/0123_full_static_typing.md:1) says “all phases complete”
- current internals docs still describe intentional gradual fallback zones

The roadmap must align the proposal corpus with the actual compiler.

## Testing plan
[testing-plan]: #testing-plan

### Phase 1

- constrained function syntax tests
- instance context enforcement tests
- class method resolution tests in constrained instance bodies

### Phase 2

- strict-mode regression tests for every prior `Any` fallback site
- focused HM tests for:
  - heterogeneous arrays/lists
  - unresolved member/index access
  - branch mismatch
  - unsupported effect signatures

### Phase 3

- subexpression-level strict diagnostics
- negative tests ensuring strict mode errors are emitted at the true failing expression

### Phase 4

- strict stdlib compilation tests
- cross-module strict typing tests
- cache/interface strict typing tests

### Phase 5

- runtime type translation tests
- strict-boundary translation tests
- typed record / structural-access regressions where relevant

## Drawbacks
[drawbacks]: #drawbacks

- This proposal will break code that currently relies on gradual fallback.
- Strict typing diagnostics may become more numerous before they become better.
- Removing `Any` from inference paths will expose latent stdlib and test-suite issues that are currently masked.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not declare static typing “done”

Because the compiler still contains semantic `Any` fallback in inference and runtime translation. Marking the work complete would make later backend and language work build on a false premise.

### Why not keep `Any` and just harden validation

Because a post-check cannot fully replace a non-gradual inference engine. If `Any` remains part of normal unification and join behavior, strict mode will always be weaker and harder to reason about.

### Why not solve only backend typing now

Because typed backends are downstream consumers. The remaining blocker is upstream: HM and runtime type translation still allow hidden graduality.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Should strict mode remove `Any` from unification entirely, or only forbid it from being constructed in maintained inference paths?
- Should strict typing become the default immediately after these phases, or after one compatibility cycle?
- How much runtime type detail is actually required for maintained strict paths?
- Which structural typing features should be considered mandatory for “full static typing” versus follow-up language work?

## Future possibilities
[future-possibilities]: #future-possibilities

- Once the type system is genuinely non-gradual in maintained paths, a later proposal can consider removing `Any` entirely from user-visible language semantics.
- After strict typing is complete, Core-level type-driven optimizations become safer and easier to justify.
- A later proposal can narrow the language’s compatibility story around strict mode and editions once the implementation no longer depends on gradual escape hatches.
