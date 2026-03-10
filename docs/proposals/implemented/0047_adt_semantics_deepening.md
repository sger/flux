- Feature Name: ADT Semantics Deepening (Post-Sugar Hardening)
- Start Date: 2026-02-26
- Completion Date: 2026-03-03
- Status: Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0047: ADT Semantics Deepening (Post-Sugar Hardening)

## Summary
[summary]: #summary

After landing `type ... = ... | ...` ADT declaration sugar, deepen ADT semantics without introducing new syntax.

## Motivation
[motivation]: #motivation

Current ADT support is functional, but semantic precision remains uneven across:
- constructor field typing under generics,
- module-qualified constructor typing and visibility boundaries,
- HM integration for nested constructor expressions,
- deeper constructor-space exhaustiveness diagnostics.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Enforce constructor field type constraints consistently across generic instantiations.
2. Stabilize module-qualified constructor typing and boundary policy diagnostics.
3. Improve HM inference around nested ADT constructor calls/patterns.
4. Strengthen nested exhaustiveness behavior and diagnostics determinism.
5. Preserve VM/JIT diagnostic parity for ADT failure cases.

### 4. Non-Goals

1. New ADT declaration syntax.
2. Trait/typeclass system.
3. Runtime ADT representation redesign.
4. Pattern theorem-proving beyond constructor-space + guard policy.

### 3. Goals

### 4. Non-Goals

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **5.1 Constructor field type enforcement:** - Ensure constructor argument checks use instantiated generic parameter mapping. - Eliminate permissive `Any` paths where concrete...
- **5.1 Constructor field type enforcement:** - Ensure constructor argument checks use instantiated generic parameter mapping. - Eliminate permissive `Any` paths where concrete constructor types are known.
- **5.2 Module-qualified constructor policy:** - Keep 0.0.4 factory-only public boundary policy explicit. - Ensure diagnostics for direct constructor boundary misuse are stable and actionable.
- **5.3 HM integration:** - Tighten HM typing for nested constructor expressions and matches. - Ensure constructor-return and pattern-binding types are propagated consistently.
- **5.4 Exhaustiveness precision:** - Extend nested constructor-space coverage checks for generic ADTs. - Keep guard policy deterministic (guarded arms contribute conditional coverage only).
- **6. Implementation Plan:** 1. Audit ADT constructor typing callsites in compiler expression/match paths. 2. Introduce/normalize generic-instantiation mapping for constructor checks. 3. Harden module const...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. New ADT declaration syntax.
2. Trait/typeclass system.
3. Runtime ADT representation redesign.
4. Pattern theorem-proving beyond constructor-space + guard policy.

### 4. Non-Goals

### 4. Non-Goals

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

## Completion notes (2026-03-03)

All four goals are implemented and test-backed:

| Goal | Implementation | Key fixtures |
|------|---------------|--------------|
| Constructor field constraints under generics | `src/ast/type_infer/adt.rs:52-89` `instantiate_constructor_parts()` | `93_adt_generic_constructor_hm_ok.flx`, `89_adt_generic_constructor_hm_mismatch.flx` |
| Module-qualified constructor boundary policy | `expression.rs:879-905`, E084/E086/W201 | `94_adt_module_factory_boundary_ok.flx` |
| HM inference for nested constructors/patterns | `adt.rs:91-120`, `check_nested_pattern_set()` | `95_adt_generic_nested_pattern_hm_ok.flx`, `91_adt_nested_pattern_binding_type_mismatch.flx` |
| Nested exhaustiveness + diagnostics determinism | `check_nested_constructor_exhaustiveness()` expression.rs:3458–3713 | `76_adt_nested_exhaustive_ok.flx`, `67_adt_multi_arity_nested_non_exhaustive.flx` |

Verification:
```bash
cargo test --test compiler_rules_tests   # 144 passed
cargo test --test type_inference_tests   # 78 passed
```
