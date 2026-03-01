- Feature Name: Effect Rows and Constraint Solving for `with e`
- Start Date: 2026-02-25
- Proposal PR: 
- Flux Issue: 

# Proposal 0042: Effect Rows and Constraint Solving for `with e`

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Effect Rows and Constraint Solving for `with e` in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

Proposal 0032 introduced algebraic effects and a practical `with e` model for higher-order functions.
Flux now supports useful effect-variable propagation, but it is still not full row polymorphism.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 2. Goals

1. Make `with e` a true row-polymorphic mechanism.
2. Support row extension and normalization (order-insensitive sets).
3. Support row constraints needed for handlers and effect-safe APIs.
4. Preserve Flux ergonomics and compatibility with existing `with` syntax.
5. Improve diagnostics for conflicting or unsatisfied effect constraints.

### 3. Non-Goals (for this proposal)

1. Full capability/security effect systems.
2. Changes to runtime effect representation.
3. Mandatory `fn main` policy changes (covered by Phase 4 hybrid policy already chosen).

### 2. Goals

### 3. Non-Goals (for this proposal)

### 3. Non-Goals (for this proposal)

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **4. Surface Model:** Flux keeps user-facing effect clauses in `with ...` form, but gains row semantics: - **5. Type-System Model:** - `Empty` row - `Label(name, tail)` row no...
- **4. Surface Model:** Flux keeps user-facing effect clauses in `with ...` form, but gains row semantics: ```flux fn map<T, U>(xs: List<T>, f: (T) -> U with e) -> List<U> with e
- **5. Type-System Model:** Represent effects as rows: - `Empty` row - `Label(name, tail)` row nodes - `Var(e)` row variables
- **6. Constraints:** Solver constraints introduced by typing: 1. **Row equality**: `r1 == r2` 2. **Row contains**: `label in r` 3. **Row extension**: `r_out = label + r_in` 4. **Handled subtraction...
- **7. Solving Strategy:** Constraint solving proceeds in phases: 1. Collect effect constraints during type/effect checking. 2. Unify row variables with occurs checks. 3. Normalize row terms after each un...
- **8.1 Row Extension:** ```flux fn with_logging<T>(f: () -> T with e) -> T with IO, e { print("start") let x = f() print("done") x } ```

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 3. Non-Goals (for this proposal)

1. Full capability/security effect systems.
2. Changes to runtime effect representation.
3. Mandatory `fn main` policy changes (covered by Phase 4 hybrid policy already chosen).

### 3. Non-Goals (for this proposal)

### 3. Non-Goals (for this proposal)

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

### 12. Open Questions

1. Should Flux expose explicit row-tail syntax in user code, or keep only `with ...` sugar?
2. Should absence constraints be part of v1, or deferred?
3. How much row detail should appear in user diagnostics by default?
4. Should `--strict` require explicit effect annotations for public higher-order APIs?

### 12. Open Questions

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
