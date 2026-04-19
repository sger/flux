- Feature Name: Type Inference Completion Roadmap
- Start Date: 2026-03-26
- Status: Implemented (2026-04-18)
- Last Updated: 2026-04-18
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0145 (Type Classes), Proposal 0156 (Static Typing Completion Roadmap)
- Closed under: [0160 (Static Typing Hardening Closure)](0160_static_typing_hardening_closure.md)
- Related implemented work: [0159 (Signature-Directed Checking and Skolemisation)](0159_signature_directed_checking_and_skolemisation.md), [0155 (Core IR Parity Simplification)](0155_core_ir_parity_simplification.md)
- Delivery commits: `457eae2d` (numeric defaulting), `db228cf1`/`d4806c84`/`f1b5ddb7` (skolems + checked mode, via 0159), `0dba6d46`/`605fd7fa` (annotation enforcement)

# Proposal 0127: Type Inference Completion Roadmap

## Summary
[summary]: #summary

Track the remaining inference-only work after the major static-typing milestones
already landed.

This proposal is no longer about broad inference completion across the whole
compiler. Most of the originally planned work is already complete elsewhere:

- constrained class constraints and solving are implemented
- constraint-aware generalization is implemented
- dictionary elaboration is implemented
- kinded higher-kinded application support is implemented
- maintained `Any`-based fallback is removed from the static-typing path by `0156`

The remaining work is narrower:

1. numeric defaulting policy
2. signature-directed checking for annotated bindings
3. skolemisation / rigid type variables for checked annotations
4. explicit support for polymorphic recursion when justified by signatures

## Implementation status
[implementation-status]: #implementation-status

| Area | Status | Notes |
|---|---|---|
| Constraint generation | Complete | Proposal `0145` |
| Constraint solving | Complete | Proposal `0145` |
| Constraint-aware generalization | Complete | `Scheme.constraints`, `generalize_with_constraints` |
| Evidence/dictionary elaboration | Complete | Core-to-Core dict elaboration |
| Kind system / HKT application | Complete | constructor-headed HKT resolution landed |
| Defaulting | Open | still no settled `Num` defaulting policy |
| Signature-directed checking | Open | annotated bindings still lack a dedicated checked path |
| Skolemisation | Open | no rigid quantified variables for checked annotations |
| Polymorphic recursion via signatures | Open | requires signature-directed checking and skolemisation |

## Motivation
[motivation]: #motivation

Flux now has a credible static-typing story. The remaining inference gaps are no
longer about gradual typing. They are about precision and expressiveness:

- annotated signatures are still mostly consumed as annotation constraints rather
  than checked contracts
- recursive definitions cannot yet use explicit signatures to unlock
  polymorphic recursion
- the language still lacks a settled ambiguity/defaulting policy for constrained
  numeric inference

Those are no longer blockers for claiming maintained static typing. They are the
main remaining inference-completeness items.

## Scope
[scope]: #scope

This proposal covers inference-layer follow-on work only.

Related proposals:

- `0155` owns Core validation and pass-boundary `core_lint`
- `0157` owns the semantic-vs-representation rationale
- `0158` owns the downstream Core/CFG/LIR representation cleanup
- `0159` owns signature-directed checking and skolemisation as the concrete
  delivery proposal for the largest remaining inference gap
- `0160` owns the final static-typing hardening closure criteria, including the
  remaining inferred-scheme surface normalization work

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Remaining phases

#### Phase 1 — Defaulting policy

Define when unconstrained numeric class variables default, and to what.

Deliverables:

- decide whether `Num` defaults to `Int`
- define whether defaulting occurs only at top-level/generalization points
- add focused tests for ambiguous numeric bindings and literal-heavy examples

#### Phase 2 — Signature-directed checking

Annotated bindings should not only constrain inference indirectly. They should
also support a dedicated checked path.

Deliverables:

- distinguish infer-only from check-against-signature paths
- treat explicit function signatures as contracts, not just sources of extra unification
- improve annotation mismatch diagnostics for quantified and constrained signatures

#### Phase 3 — Skolemisation and rigid type variables

Checked signatures need rigid quantified variables so the inference engine does
not silently solve them by ordinary unification.

Deliverables:

- introduce rigid checked type variables for quantified annotations
- reject escaping or illegally unified rigid variables with dedicated diagnostics
- make checked signature mismatches more local and comprehensible

#### Phase 4 — Polymorphic recursion via explicit signatures

Recursive groups should be able to use explicit signatures to type-check through
their annotated polymorphic shape instead of monomorphically self-unifying.

Deliverables:

- use signatures to break recursive groups where legal
- support polymorphic recursion only when an explicit checked signature justifies it
- add focused recursive-function fixtures and regressions

## Exit criteria
[exit-criteria]: #exit-criteria

This proposal should be considered complete when:

- defaulting behavior is explicitly specified and test-backed
- annotated bindings can follow a true checked path
- rigid quantified variables exist for checked annotations
- explicit signatures can enable polymorphic recursion in supported cases
- these changes are documented as inference follow-on work rather than as
  blockers for `0156`

## Historical note
[historical-note]: #historical-note

This file originally tracked a much broader inference-completion effort. Most of
that work is already complete and now belongs to the main static-typing corpus:

- `0145` completed constraints, solving, and elaboration
- `0156` completed maintained front-end static typing
- `0155`, `0157`, and `0158` cover the remaining Core/runtime-representation follow-on work
- `0159` is the concrete delivery proposal for the remaining annotation-checking and skolemisation work
- `0160` is the umbrella closure proposal that ties the remaining hardening
  items into one final static-typing story
