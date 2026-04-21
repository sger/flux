- Feature Name: HKT Polymorphic Dispatch Completion
- Start Date: 2026-04-20
- Status: Proposed
- Proposal PR:
- Flux Issue:
- Depends on: [0145](0145_type_class_dispatch.md), [0146](0146_type_class_dispatch_runtime.md), [0147](0147_dictionary_elaboration.md), [0150](0150_constructor_headed_instance_resolution.md)

# Proposal 0168: HKT Polymorphic Dispatch Completion

## Summary
[summary]: #summary

Complete the remaining gap in Flux type-class dispatch for higher-kinded and constructor-headed polymorphic calls by routing unresolved polymorphic class-method calls through the existing dictionary elaboration machinery instead of falling back to runtime panic stubs. This proposal makes the existing type-class surface more complete without changing user syntax.

## Motivation
[motivation]: #motivation

Flux already supports a substantial class-dispatch stack:

- type classes
- multi-parameter classes
- superclasses
- direct monomorphic dispatch via mangled `__tc_*` functions
- compile-time constructor-headed instance resolution from 0150
- dictionary elaboration machinery for polymorphic calls

But one real gap remains:

- some polymorphic higher-kinded dispatch paths still end up at the panic stubs instead of being fully routed through dictionary resolution

That means user code can look accepted at the surface while still failing in a dispatch path that should be statically supported.

### Why this matters

This blocks an important class of abstractions:

- container-polymorphic class methods
- reusable higher-order library APIs over type constructors
- the practical completeness of the current class system

If Flux exposes classes over constructor-headed types, the polymorphic path should not degrade into panic for cases the type system and elaboration machinery already conceptually support.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### The user model

After this proposal, a class-based polymorphic function over a type constructor should either:

- resolve through dictionary elaboration and compile correctly
- or fail statically for a real missing-instance reason

It should not succeed through typing and then panic because the remaining dispatch path was never completed.

### Example shape

Conceptually, code like:

```flux
fn map_like<F, A, B>(xs: F<A>, f: A -> B) -> F<B>
    where Functor<F>
{
    fmap(xs, f)
}
```

should use the same dispatch model end to end:

- direct mangled call when monomorphic and known
- dictionary-elaborated call when polymorphic

not:

- direct call sometimes
- panic stub for the unresolved polymorphic path

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

## Scope

This proposal covers the remaining incomplete dispatch paths for polymorphic constructor-headed and HKT-shaped class calls.

It does not add:

- new syntax for classes
- higher-rank types
- new runtime representation for values

## Current behavior

The current class pipeline already has three useful layers:

1. instance method generation as mangled `__tc_*` functions
2. compile-time direct resolution for concrete monomorphic call sites
3. dictionary elaboration for polymorphic calls

The remaining problem is that some polymorphic HKT/constructor-headed cases still reach the generated polymorphic panic stubs instead of staying on the dictionary-elaborated path.

## Design

### Principle

If a call is:

- polymorphic
- class-constrained
- and not directly resolvable monomorphically

then it must be routed through dictionary elaboration, not left on the panic placeholder.

### Required work

1. Audit the remaining call sites where compile-time direct resolution declines.
2. Ensure constructor-headed/HKT-shaped constraints are preserved through the elaboration path.
3. Ensure the elaborated call carries the right dictionary evidence for the method actually invoked.
4. Keep the panic stub only as a true unreachable fallback for invalid or unimplemented cases, not as the normal path for valid polymorphic dispatch.

## Acceptance criteria

- constructor-headed polymorphic class calls no longer fall through to panic when valid evidence exists
- existing monomorphic direct dispatch remains unchanged
- existing dictionary elaboration cases remain green
- parity tests cover at least one HKT-shaped polymorphic dispatch scenario that previously failed

## Drawbacks
[drawbacks]: #drawbacks

- This proposal operates in a subtle part of the compiler where HM inference, class environment lookup, and lowering interact closely.
- It will likely require new focused regression fixtures to keep the dispatch matrix understandable.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not leave this for later

Because Flux already exposes enough class machinery that the incomplete polymorphic path now feels like a correctness/completeness bug, not an optional enhancement.

### Why not solve this with new syntax

The problem is not syntax. The problem is finishing the semantics of the existing class-dispatch architecture.

## Prior art
[prior-art]: #prior-art

Languages with class/typeclass-style dispatch typically distinguish:

- direct specialized calls for monomorphic sites
- evidence/dictionary passing for polymorphic sites

Flux already follows that architecture in broad form. This proposal completes the missing part for constructor-headed and HKT-shaped uses.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Exact remaining gap surface after a full dispatch-path audit.
- Whether any current panic stub should remain observable in supported user code after this lands.
- Whether additional interface/module metadata is needed for imported polymorphic class evidence.

## Future possibilities
[future-possibilities]: #future-possibilities

- richer container-polymorphic stdlib abstractions
- better no-instance diagnostics for polymorphic constructor-headed cases
- eventual interaction with higher-rank checking if Flux later adds it
