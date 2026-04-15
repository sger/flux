- Feature Name: Static Typing Hardening Closure
- Start Date: 2026-04-15
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: 0127 (Type Inference Completion Roadmap), 0155 (Core IR Parity Simplification), 0156 (Static Typing Completion Roadmap), 0158 (Core Semantic Types and Backend Representation Split Execution), 0159 (Signature-Directed Checking and Skolemisation)

# Proposal 0160: Static Typing Hardening Closure

## Summary
[summary]: #summary

Define the remaining work needed to make Flux's static-typing story fully
coherent at the language, semantic IR, and inspection surfaces.

This proposal is an umbrella closure proposal. It does **not** replace `0155`
or `0159`:

- `0155` remains the delivery proposal for `core_lint`
- `0159` remains the delivery proposal for signature-directed checking and
  skolemisation
- `0160` owns the final hardening and closure criteria across the static-typing
  stack, including the one workstream not cleanly owned elsewhere yet:
  inferred-scheme surface normalization

The closure target is:

1. stable inferred-scheme and inspection surfaces
2. maintained Core invariant verification
3. checked signatures with rigid quantified variables
4. a proposal/test/document stack that states one consistent static-typing story

## Motivation
[motivation]: #motivation

Flux already crossed the main static-typing threshold:

- `0156` closed maintained front-end static typing
- `0158` removed semantic `Dynamic` from the maintained Core/backend pipeline
- proof-oriented test suites now show that unannotated programs infer and
  execute under a real static-typing model

What remains is not a language-model reversal. It is hardening work:

1. **Inferred-scheme surfaces still have representation quirks**
   - proof-facing scheme assertions still need normalization helpers
   - some inspection surfaces expose internal free-vars-vs-forall quirks rather
     than one canonical semantic rendering

2. **Core validation is still weaker than it should be**
   - Core now carries explicit semantic structure, but the stack still lacks a
     maintained `core_lint`-style invariant verifier

3. **Checked signatures are not complete yet**
   - annotated bindings still need a true checked path
   - rigid quantified variables and skolemisation still belong to future work

These gaps do not invalidate the claim that Flux is statically typed. They are
the remaining hardening work needed to make that claim cleaner, more stable,
and easier to maintain.

## Scope
[scope]: #scope

This proposal covers the final static-typing hardening and closure criteria.

It is explicitly **not**:

- a return to `Any`
- a new semantic IR
- a new user-facing typing mode
- a source-code linting proposal

It uses nearby proposals as implementation-owning children:

- `0155` owns Core validation work such as `core_lint`
- `0159` owns signature-directed checking and skolemisation

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Track A — Inferred-scheme surface normalization

This is the main workstream that `0160` owns directly.

Flux now needs a canonical scheme-surface contract for:

- `TypeEnv`-facing inspection
- exported/module-member scheme inspection
- cache/debug/CLI formatting
- proof-oriented test helpers such as the semantic typing matrix

Required behavior:

- quantified variable naming must normalize deterministically
- free-vars-vs-forall storage quirks must not leak into proof-facing or
  user-facing scheme output
- constraints and effect rows must render stably enough for regression tests
- one canonical semantic rendering style must exist for inferred schemes

Expected touch points:

- `src/types/scheme.rs`
- `src/driver/support/shared.rs`
- compiler/module-interface inspection surfaces
- proof-oriented test helpers

This track is complete when:

- scheme assertions no longer depend on raw internal type-variable IDs
- public/test/debug scheme formatting agrees on one semantic contract
- proof suites do not need ad hoc normalization workarounds to assert ordinary
  inferred schemes

### Track B — Core invariant verification

This proposal delegates implementation to `0155`, but defines the acceptance
bar for closing the static-typing hardening story.

`core_lint` must exist as a maintained Core verifier with explicit checks for:

- binder scope validity
- lambda, join, and handler parameter metadata consistency
- case result-type consistency
- malformed Core introduced by semantic passes
- regressions that reintroduce semantic-placeholder behavior into maintained
  Core forms

Required placement:

- after major Core simplification/canonicalization points
- after ANF normalization
- in targeted regression tests for malformed or intentionally bad internal Core
  shapes where practical

This proposal is explicit that `core_lint` is:

- a compiler-internal verifier for Core correctness
- not the Flux source-code linter

This track is complete when:

- `0155` lands a maintained `core_lint`
- valid maintained Core passes it
- targeted invalid-Core regressions fail for the right reasons

### Track C — Checked signatures and skolemisation

This proposal delegates implementation to `0159`, but treats it as a required
closure condition for the static-typing hardening story.

Closure requires:

- annotated bindings use a real checked path
- quantified signature variables become rigid during checking
- checked-signature mismatch diagnostics are distinct from ordinary inference
  mismatch
- explicitly typed recursive groups can justify supported polymorphic recursion

This proposal does not restate `0159` in full. `0159` remains the concrete
delivery proposal.

This track is complete when:

- the `0159` exit criteria are met
- the inference stack no longer treats explicit signatures as soft annotation
  constraints only

## Phases
[phases]: #phases

### Phase 1 — Scheme-surface normalization

Exit criteria:

- canonical scheme rendering exists
- proof-facing scheme assertions no longer depend on internal storage quirks
- module/export/debug surfaces use the same semantic formatting contract

### Phase 2 — Core validation

Exit criteria:

- `core_lint` is implemented via `0155`
- maintained Core pass boundaries enforce the intended invariant set
- Core contract regressions fail deterministically

### Phase 3 — Checked signatures and skolemisation

Exit criteria:

- `0159` lands its checked-binding path
- rigid quantified variables exist during checking
- checked recursive signatures support the intended polymorphic-recursion cases

### Phase 4 — Closure pass

Exit criteria:

- proposal statuses and cross-links are consistent
- scheme, Core, and checked-signature proof suites are green
- the static-typing proposal stack presents one coherent closure story

## Public interfaces and internal contracts
[public-interfaces-and-internal-contracts]: #public-interfaces-and-internal-contracts

Expected changes:

- one canonical scheme-formatting path becomes the proof-facing display
  contract
- `core_lint` becomes a maintained Core pass entrypoint under `src/core/passes`
- inference gains an explicit infer-vs-check split through `0159`
- rigid/skolem checked variables become part of the internal inference model
  through `0159`

This proposal itself does not introduce:

- new Flux syntax
- new CLI flags
- a new semantic IR

## Test plan
[test-plan]: #test-plan

Required coverage categories:

### Scheme surface

- alpha-renaming-stable scheme rendering
- constrained and effectful scheme formatting
- module-export/member-scheme stability
- cache/debug/CLI scheme output consistency

### Core validation

- invalid binder scope is rejected
- malformed case/join/handler shapes are rejected
- valid maintained Core passes `core_lint`
- Core dump and matrix regressions remain free of semantic `Dynamic`

### Checked signatures

- annotated polymorphic identity and higher-order functions
- checked-signature mismatch
- rigid-variable escape
- recursive signatures and supported polymorphic recursion
- unsupported checked-recursion cases with explicit diagnostics

### Closure and proof

- semantic typing matrix remains green
- static-typing contract suite remains green
- Core contract tests remain green

## Relationship to nearby proposals
[relationship-to-nearby-proposals]: #relationship-to-nearby-proposals

- `0127` remains the inference follow-on roadmap
- `0155` remains the implementation-owning Core validation proposal
- `0156` remains complete for maintained front-end static typing
- `0158` remains the implemented downstream semantic-`Dynamic` cleanup proposal
- `0159` remains the implementation-owning checked-signature and skolemisation
  proposal

`0160` is the closure umbrella that ties those pieces into one final
static-typing hardening story.

## Exit criteria
[exit-criteria]: #exit-criteria

This proposal is complete when:

- inferred-scheme inspection has a canonical semantic rendering contract
- `0155` lands maintained Core invariant verification
- `0159` lands checked signatures and rigid quantified variables
- proof-oriented static-typing suites remain green without normalization
  workarounds leaking into user-facing semantics
- the proposal corpus clearly presents Flux as already statically typed, with
  these items recorded as hardening and closure work rather than as preconditions
  for the original claim
