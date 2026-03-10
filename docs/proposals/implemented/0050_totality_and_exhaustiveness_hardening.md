- Feature Name: Totality and Exhaustiveness Hardening
- Start Date: 2026-02-26
- Completion Date: 2026-03-03
- Status: Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0050: Totality and Exhaustiveness Hardening

## Summary
[summary]: #summary

Strengthen compile-time totality/exhaustiveness guarantees for supported match spaces so missing cases are caught deterministically and runtime match failures are minimized.

## Motivation
[motivation]: #motivation

Flux has improved exhaustiveness checks, but totality coverage is still uneven across domains and nested pattern shapes.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Define domain-by-domain totality behavior as an explicit contract.
2. Normalize guard semantics across all match coverage logic.
3. Improve nested constructor-space and tuple/list coverage where currently partial.
4. Make residual runtime-failure boundary explicit and narrow.
5. Preserve deterministic diagnostics and VM/JIT parity expectations.

### 4. Non-Goals

1. Full theorem proving over arbitrary guards.
2. New pattern syntax or or-pattern redesign.
3. Runtime pattern engine redesign.
4. Record-pattern totality (until record typing proposal lands).

### 6. Guard Semantics (Locked)

1. Guarded arms never provide unconditional coverage on their own.
2. A guarded wildcard does not satisfy catch-all requirements.
3. Only unguarded wildcard/identifier catch-all arms provide unconditional fallback.
4. Diagnostics must clearly state guard-conditional non-coverage when relevant.

### 3. Goals

### 4. Non-Goals

### 6. Guard Semantics (Locked)

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **5. Coverage Domains (Canonical Matrix):** Each domain must be tagged `guaranteed | conservative | unsupported`. - **7. Formalized Coverage Algorithm (Implementation Contract...
- **5. Coverage Domains (Canonical Matrix):** Each domain must be tagged `guaranteed | conservative | unsupported`.
- **7. Formalized Coverage Algorithm (Implementation Contract):** 1. Build domain-specific constructor/value partitions for the scrutinee type. 2. For each arm: - compute covered partition subset, - mark as conditional if guarded. 3. Compute u...
- **8. Diagnostics Policy:** 1. Reuse `E015` for non-exhaustive match where class is unchanged. 2. Reuse existing ADT exhaustiveness diagnostics where applicable (`E083` class if active). 3. Add new codes o...
- **9. Residual Runtime-Failure Policy:** Runtime match failure is acceptable only when one of the following holds: 1. scrutinee domain is intentionally dynamic/unknown (`Any`-driven path), 2. pattern-space reasoning fo...
- **Stage 1 (strict-first):** 1. Apply strongest exhaustiveness hardening in strict and typed/HM-known contexts. 2. Keep conservative behavior in unresolved gradual contexts.

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. Full theorem proving over arbitrary guards.
2. New pattern syntax or or-pattern redesign.
3. Runtime pattern engine redesign.
4. Record-pattern totality (until record typing proposal lands).

### 4. Non-Goals

### 14. Risks and Mitigations

1. Risk: false-positive non-exhaustive errors.
   - Mitigation: conservative-domain labeling and support-table transparency.
2. Risk: user confusion around guards.
   - Mitigation: explicit guard semantic messaging in diagnostics/docs.
3. Risk: diagnostic churn across nested ADT paths.
   - Mitigation: snapshot gating and deterministic ordering rules.

### 18.4 Deferred Non-Goal (Explicit)

Tuple completeness theorem/prover-style reasoning is intentionally deferred.  
Conservative tuple policy remains canonical for Proposal 0050 closure.

### 4. Non-Goals

### 14. Risks and Mitigations

### 18.4 Deferred Non-Goal (Explicit)

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

### Coverage domain matrix (implemented)

| Domain | Coverage level | Guarantee |
|--------|---------------|-----------|
| ADT constructors | guaranteed | Full constructor-space partition; E083 on missing arms |
| Bool | guaranteed | `true`/`false` partition; E015 if either missing |
| Option / Either | guaranteed | `Some`/`None`, `Left`/`Right` partition; E015/E083 |
| Lists (cons/nil) | guaranteed | `[]` / `[h \| t]` partition |
| Tuples | conservative | Requires unguarded catch-all; mixed-shape arms conservatively rejected |
| `Any`-typed values | unsupported | Runtime match failure is expected; no compile-time guarantee |

### Guard semantics (locked)

- Guarded arms never provide unconditional coverage (`arm.guard.is_none()` check in `pattern_validate.rs:159`).
- Guarded wildcard does **not** satisfy catch-all — emits `E015` with "guard may fail" hint.
- Only unguarded `_` or identifier arms provide unconditional fallback.
- Test: `match_guarded_wildcard_only_non_exhaustive_error` in `compiler_rules_tests.rs`.
- Fixture: `144_guarded_wildcard_only_non_exhaustive_targeted.flx`.

### Residual runtime-failure policy

Runtime match failure (`E1016`) is accepted only when:
1. Scrutinee type is `Any`-driven (dynamic path).
2. Pattern-space reasoning is unsupported for the domain (e.g., arbitrary integer literals).
3. Conservative tuple policy applies and no catch-all arm is present (programmer's responsibility).

Fully-typed ADT/Bool/Option/Either/List programs with correct exhaustiveness are guaranteed no runtime match failure for the covered domains.

### Deferred (explicit non-goals)

- Tuple completeness theorem-prover: deferred per §18.4.
- Record-pattern totality: deferred until 0048 (typed records) lands.
- Or-pattern semantics redesign: out of scope.
