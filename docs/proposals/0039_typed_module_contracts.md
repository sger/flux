- Feature Name: Typed Module Contracts (Boundary-First Typing)
- Start Date: 2026-02-20
- Proposal PR: 
- Flux Issue: 

# Proposal 0039: Typed Module Contracts (Boundary-First Typing)

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Typed Module Contracts (Boundary-First Typing) in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

Flux remains gradual/dynamic internally, but most production failures happen at API boundaries between modules.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 5. Compiler Design

Add `ModuleContractTable` built during module compilation:

- key: `(module_name, function_name, arity)`
- value: `FnContract { params, ret, effects }`

Lowering steps:

1. Parse/store annotations on exported functions.
2. Validate annotation syntax + referenced type names.
3. At direct calls to known exported symbols:
   - if argument types known and incompatible -> compile-time error
   - if argument is `Any`/unknown -> emit runtime boundary check node
4. For unknown/generic calls, keep existing dynamic path.

No syntax break required if `export` already exists; otherwise module export marker can be introduced in same phase.

### 6. Runtime/VM Design

Introduce lightweight runtime contract checks:

- `check_arg_type(value, expected_type, span)`
- `check_return_type(value, expected_type, span)`

Check insertion points:

- function entry for exported typed functions called from dynamic sites
- function return boundary before handing value to typed caller

Behavior:

- on mismatch: structured runtime type error with expected/actual + source location
- keep error wording aligned with existing diagnostics style

### 5. Compiler Design

Add `ModuleContractTable` built during module compilation:

Lowering steps:

### 6. Runtime/VM Design

Introduce lightweight runtime contract checks:

Check insertion points:

Behavior:

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **2. Scope:** - typed signatures for exported module functions - compile-time checks at typed call sites - runtime boundary checks (`Any -> T`) at dynamic call sites - effect...
- **2. Scope:** v1 scope: - typed signatures for exported module functions - compile-time checks at typed call sites - runtime boundary checks (`Any -> T`) at dynamic call sites - effect annota...
- **3. User Model:** Example: ```flux module Math { export fn add(a: Int, b: Int) -> Int { a + b } } ```
- **4. Type Surface (v1):** Supported contract types: - `Int`, `Float`, `Bool`, `String` - `Array<T>`, `Map<K, V>`, `Option<T>` - `Any` (explicit escape hatch) - function return types + effect clause `with...
- **7. JIT Parity:** Policy parity: - JIT must enforce the same boundary checks as VM.
- **8. Effect Contract Layer:** v1 rules: - if export signature is pure, body cannot call effectful primops/base functions - if export signature has `with IO`, IO operations are allowed - typed callers must sa...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

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
