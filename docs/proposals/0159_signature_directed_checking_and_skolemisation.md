- Feature Name: Signature-Directed Checking and Skolemisation
- Start Date: 2026-04-15
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: 0127 (Type Inference Completion Roadmap), 0145 (Type Classes), 0156 (Static Typing Completion Roadmap)

# Proposal 0159: Signature-Directed Checking and Skolemisation

## Summary
[summary]: #summary

Introduce a dedicated checked path for annotated bindings and the rigid type
machinery needed to support it.

This proposal adds four closely related pieces:

1. signature-directed checking for annotated bindings
2. rigid quantified variables for checked signatures
3. explicit skolemisation boundaries during checking
4. polymorphic recursion support when an explicit checked signature justifies it

This proposal does **not** restart static typing from scratch. `0156` already
closed maintained front-end static typing. The goal here is narrower: make the
inference engine more precise and expressive around explicit annotations.

## Motivation
[motivation]: #motivation

Flux now has:

- constraint generation and solving
- constrained schemes
- dictionary elaboration
- maintained static typing in the front-end pipeline

What it still lacks is a strong distinction between:

- inferring a type for an unannotated binding
- checking a binding against an explicit polymorphic signature

Without that distinction, three problems remain:

1. **Explicit signatures are weaker than they should be**
   - many annotations behave like extra unification constraints instead of checked contracts

2. **Polymorphic recursion is still blocked in practice**
   - recursive functions cannot reliably use explicit signatures to type-check through their intended polymorphic shape

3. **Diagnostics around quantified annotations remain weaker**
   - without rigid checked variables, mismatch and escape diagnostics are less precise than they should be

This proposal addresses those problems directly.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Compiler contributors should use this mental model:

> inference discovers types; checking verifies explicit signatures.

After this proposal:

- unannotated bindings still use the existing inference path
- annotated bindings can use a checked path
- quantified variables introduced by a checked signature are rigid during checking
- recursive groups can use explicit signatures to justify polymorphic recursion where supported

### What changes for users

Annotated code becomes more trustworthy:

```flux
fn id<a>(x: a) -> a { x }
```

The compiler should check this against the declared quantified shape, not merely
treat it as a set of soft constraints.

Recursive code with explicit signatures becomes more expressive:

```flux
fn fold<a, b>(xs: List<a>, acc: b, f: (b, a) -> b) -> b { ... }
```

When supported by the recursive structure, the signature should guide recursive
calls through the declared polymorphic type instead of forcing monomorphic
self-unification.

## Non-goals
[non-goals]: #non-goals

- redesigning Core, CFG, or native runtime representation
- replacing `0155` Core validation work
- replacing `0157` or `0158` downstream representation cleanup
- broad solver redesign unrelated to checked signatures
- reopening `Any` or gradual-typing semantics

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Track 1 — Checked binding path

Introduce a dedicated checking path for annotated bindings:

- annotated functions
- typed `let` bindings where a checked path is meaningful
- recursive groups with explicit signatures

This path should:

- instantiate the declared signature in checking mode
- type-check the body against that expected shape
- reject mismatches against the checked contract directly

It should not rely solely on “infer first, then see whether annotation unification happened to work”.

### Track 2 — Skolemisation and rigid quantified variables

When checking against a quantified signature, quantified variables must become
rigid checked variables for the duration of the check.

Required behavior:

- rigid variables cannot be freely unified away
- escaping rigid variables produce dedicated diagnostics
- checked signatures can distinguish “wrong implementation” from “unresolved inference residue”

This is the core semantic step that makes checked signatures real contracts.

### Track 3 — Recursive group splitting and polymorphic recursion

Recursive groups should gain a checked path that can use explicit signatures to
break groups more precisely.

Required behavior:

- explicitly typed recursive bindings can enter the environment through their checked signature
- dependent bindings in the same group can type-check against that signature
- polymorphic recursion is supported only when an explicit checked signature justifies it

Unannotated recursive bindings remain on the ordinary inference path.

### Track 4 — Diagnostics

Add clearer diagnostics for:

- checked-signature mismatch
- rigid variable escape
- illegal unification against rigid checked variables
- unsupported polymorphic-recursion cases

These diagnostics should be distinct from ordinary inference mismatch and
ordinary unresolved-boundary errors.

## Touched modules
[touched-modules]: #touched-modules

Primary expected modules:

- `src/ast/type_infer/function.rs`
- `src/ast/type_infer/statement.rs`
- `src/ast/type_infer/mod.rs`
- `src/types/scheme.rs`
- `src/types/type_env.rs`
- new checking/skolem support module(s) under `src/ast/type_infer/` or `src/types/`
- diagnostics definitions and rendering

Possible follow-on touch points:

- parser/AST only if additional annotation metadata is needed
- proposal/test documentation for checked recursive fixtures

## Phases
[phases]: #phases

### Phase 1 — Checked signature infrastructure

Deliver:

- a dedicated checking entry point for annotated bindings
- explicit distinction between infer mode and check mode
- regression tests for straightforward annotated functions

### Phase 2 — Skolemisation

Deliver:

- rigid checked variables for quantified annotations
- checked mismatch diagnostics that do not depend on ordinary flexible unification
- escape diagnostics for rigid variables

### Phase 3 — Recursive signatures

Deliver:

- explicit-signature handling for recursive groups
- supported polymorphic recursion with checked signatures
- regression fixtures for recursive functions that were previously forced monomorphic

### Phase 4 — Hardening and docs

Deliver:

- documentation updates in the inference proposal stack
- focused integration tests for annotated functions, typed lets, and recursive groups
- proposal cross-links so this work is clearly separated from `0155`, `0157`, and `0158`

## Test plan
[test-plan]: #test-plan

Required coverage:

- annotated polymorphic identity and higher-order functions
- checked mismatch against quantified signatures
- rigid-variable escape cases
- recursive functions that require explicit signatures to type-check
- negative cases where polymorphic recursion remains unsupported

Suggested suites:

- `tests/type_inference_tests.rs`
- `tests/compiler_rules_tests.rs`
- targeted new regression tests for annotated recursive bindings

## Relationship to nearby proposals
[relationship-to-nearby-proposals]: #relationship-to-nearby-proposals

- `0127` remains the inference follow-on roadmap; this proposal is its concrete delivery slice for checked signatures and skolemisation.
- `0155` remains the Core validation and `core_lint` proposal.
- `0157` and `0158` remain the semantic-vs-representation and downstream cleanup proposals.
- `0160` remains the umbrella static-typing hardening closure proposal that
  treats this work as a final acceptance criterion rather than as a reopening
  of the main static-typing claim.
- `0156` remains complete for maintained static typing; this proposal is about inference completeness, not reopening that closure.

## Exit criteria
[exit-criteria]: #exit-criteria

This proposal is complete when:

- annotated bindings can be checked through a dedicated checked path
- quantified checked variables are rigid during checking
- explicit signatures can enable supported polymorphic recursion
- diagnostics clearly distinguish checked-signature failures from ordinary inference failures
- the proposal corpus no longer treats this work as an unnamed leftover under older static-typing documents
