- Feature Name: Decouple Aether from Core IR
- Start Date: 2026-04-11
- Status: Not Implemented
- Proposal PR:
- Flux Issue:

## Summary
[summary]: #summary

Decouple Aether from `CoreExpr` by making Aether a **backend-only lowering layer** instead of a set of `CoreExpr` variants or `CorePrimOp`s.

The corrected architecture is:

- `src/core/` remains the only semantic IR.
- `run_core_passes*` produces clean semantic Core only.
- Aether becomes a lowering product derived from clean Core plus borrow metadata.
- RC backends (VM and LLVM/native) consume the Aether product.
- Future non-RC backends consume clean Core directly and never need to know about Aether.

This proposal intentionally does **not** introduce a second semantic IR. Aether is a backend-specific ownership and memory-management layer, not a semantic compiler stage.

## Motivation
[motivation]: #motivation

Today, `CoreExpr` in `src/core/mod.rs` contains five Aether-only variants:

```rust
CoreExpr::Dup { var, body, span }
CoreExpr::Drop { var, body, span }
CoreExpr::Reuse { token, tag, fields, field_mask, span }
CoreExpr::DropSpecialized { scrutinee, unique_body, shared_body, span }
CoreExpr::AetherCall { func, args, arg_modes, span }
```

These variants are injected after semantic Core passes and are only meaningful to RC-based backends. That creates several problems:

### 1. Semantic Core walkers must understand backend-only structure

Many generic Core helpers and tests have to match on `Dup`, `Drop`, `Reuse`, `DropSpecialized`, and `AetherCall` even though those nodes are not semantic Core constructs. This leaks backend ownership planning into:

- Core display/dump code
- Core walkers and helper analyses
- tests that are nominally about Core
- lowering code that has to distinguish "clean Core" from "Aether-mutated Core"

### 2. `run_core_passes*` does not currently mean "semantic Core only"

The current pipeline runs Aether inside `run_core_passes_with_optional_interner`. That means call sites asking for "Core passes" actually receive ownership-annotated, backend-specific Core. As a result:

- there is no stable "clean Core" compiler boundary today
- `--dump-core` is not guaranteed to remain purely semantic
- future backends would need to either run and ignore Aether or fork the pipeline

### 3. Re-encoding Aether as Core primops would still pollute Core

A previous direction considered deleting the five `CoreExpr` variants and re-encoding them as `CoreExpr::PrimOp`. That does not solve the real problem. It would still place backend-only ownership structure inside semantic Core, just under a different syntax.

This is especially awkward for:

- `AetherCall`, which is a call-site ownership contract rather than a primitive operation
- `Reuse`, which carries structural construction data
- `DropSpecialized`, which is control flow with unique/shared branches

These are not naturally "just primops".

### 4. Flux needs a clean semantic/backend boundary

The architecture contract for Flux is:

- `Core` is the only semantic IR
- backend-specific lowering happens below Core

Aether is backend-specific lowering. Treating it as such makes the architecture honest and keeps future backend work tractable.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Before

Today the pipeline is effectively:

```text
AST -> Core -> semantic Core passes -> Aether mutates CoreExpr in place
    -> CFG / LIR lowering
```

The same `CoreExpr` type is used for both:

- semantic program structure
- ownership / RC planning

### After

The corrected pipeline is:

```text
AST -> Core -> semantic Core passes -> clean Core
    -> Aether lowering -> AetherProgram
        -> VM/CFG lowering
        -> LLVM/LIR lowering
```

or, for a future non-RC backend:

```text
AST -> Core -> semantic Core passes -> clean Core
    -> backend-specific lowering that does not use Aether
```

Key consequences:

- `CoreExpr` becomes semantic-only again.
- `--dump-core` shows semantic Core only.
- `--dump-aether` becomes the ownership/debug surface for RC backends.
- VM and LLVM stop consuming "Core with Aether nodes" and instead consume a backend-only Aether product.

### What Aether becomes

Aether becomes its own lowering representation, owned by `src/aether/`, with names such as:

- `AetherExpr`
- `AetherDef`
- `AetherProgram`

It can structurally resemble today’s Aether-enriched Core when useful, but it is no longer `CoreExpr`.

This is **not** a second semantic IR:

- semantic passes still operate on clean Core only
- Aether exists only for ownership planning, FBIP validation, debug dumping, and RC backend lowering

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### 1. Clean up the public compiler boundary

Split the current pipeline into two explicit stages.

#### Semantic Core stage

`run_core_passes*` becomes semantic-only:

- builtin promotion
- simplification loop
- evidence passing
- ANF normalization
- dictionary elaboration where applicable

It must not:

- run Aether insertion
- materialize Aether-only nodes
- enforce Aether-only validation contracts

Its output is the canonical clean `CoreProgram`.

#### Aether stage

Introduce a new entrypoint owned by `src/aether/`, for example:

```rust
pub fn lower_core_to_aether(
    core: &CoreProgram,
    registry: &BorrowRegistry,
) -> Result<AetherProgram, Diagnostic>
```

or an equivalent API that is decision-equivalent:

- input: clean Core
- input: borrow metadata
- output: backend-only Aether product

The exact function name can vary, but the boundary must be explicit and stable.

### 2. Define the Aether representation as backend-only

The new Aether layer must support these behaviors:

- ownership-aware call nodes or call annotations
- explicit dup/drop sequencing
- reuse nodes with field-mask information
- drop-specialization branching with unique/shared continuations

It must preserve enough structure for:

- borrow inference output
- FBIP checking
- `--dump-aether`
- VM lowering
- LLVM/LIR lowering

It must not be used by semantic Core passes.

### 3. Move `AetherCall` out of `CoreExpr`

`AetherCall` should not survive as a semantic Core construct and should not be collapsed into a plain `App` plus an invisible side table.

Instead:

- Core remains a normal `App`
- borrow metadata still lives in `BorrowRegistry`
- the Aether lowering stage materializes an Aether-layer call form that carries the ownership contract needed for debug output and lowering

This keeps ownership-aware calls visible at the backend-only layer without polluting semantic Core.

### 4. Remove Aether-only `CoreExpr` variants after the Aether layer exists

Once the Aether representation is introduced and both RC backends consume it, delete these from `CoreExpr`:

```rust
AetherCall { func, args, arg_modes, span }
Dup { var, body, span }
Drop { var, body, span }
Reuse { token, tag, fields, field_mask, span }
DropSpecialized { scrutinee, unique_body, shared_body, span }
```

This removal is intentionally **not** the first step.

### 5. Update backend lowering contracts

After the Aether layer exists:

- VM/CFG lowering consumes Aether form, not `CoreExpr` with Aether variants
- LLVM/LIR lowering consumes Aether form, not `CoreExpr` with Aether variants

Backends must no longer pattern-match Aether-only structure out of `CoreExpr`.

### 6. Keep debug surfaces aligned with the architecture

The proposal locks these surfaces:

- `--dump-core`
  - semantic-only Core
  - first semantic debugging surface
- `--dump-aether`
  - backend-only ownership/debug surface
  - shows borrow modes, reuse, and drop specialization

This keeps the debugging story aligned with the architecture contract:

- inspect Core first for semantic bugs
- inspect Aether only after Core is correct

## Implementation strategy
[implementation-strategy]: #implementation-strategy

This proposal is intentionally staged. It should **not** be implemented as a one-shot refactor unless a separate spike proves that safe.

### Stage 1: Split the pipeline boundary first

Goals:

- make `run_core_passes*` semantic-only in API and intent
- introduce an explicit post-Core Aether stage in the pipeline
- keep current internals temporarily if needed while the boundary is being established

Requirements:

- all clean-Core call sites become explicit
- `--dump-core` remains semantic-only
- `--dump-aether` remains the ownership/debug surface
- do not collapse Aether into Core primops during this stage

This stage is allowed to preserve some temporary compatibility internally, but the public/compiler boundary must become explicit.

### Stage 2: Introduce backend-only Aether types

Goals:

- add `AetherExpr` / `AetherDef` / `AetherProgram` (or equivalent)
- port Aether logic from `CoreExpr -> CoreExpr` transforms into:
  - clean-Core-to-Aether lowering
  - Aether-to-Aether transforms/checks

Requirements:

- borrow inference still feeds Aether lowering
- `Reuse` and `DropSpecialized` stay structural in Aether, not re-encoded as Core primops
- `--dump-aether` reads from the new Aether product
- VM and LLVM lowering switch to the new Aether product

### Stage 3: Remove Aether from Core entirely

Goals:

- delete the five Aether-specific `CoreExpr` variants
- remove Aether match arms from semantic Core walkers and helpers
- simplify Core tests so they no longer assert on Aether-only nodes

Requirements:

- semantic Core helpers no longer know about Aether-only structure
- Aether regression tests move to Aether-layer tests
- backend lowering no longer matches Aether nodes in `CoreExpr`

This proposal explicitly forbids collapsing Stages 2 and 3 unless a smaller exploratory spike demonstrates that the migration is mechanically safe.

## Migration impact
[migration-impact]: #migration-impact

The major affected areas are:

- `src/core/`
  - semantic walkers/helpers stop matching Aether-only constructs
- `src/core/passes/`
  - no Aether insertion inside semantic Core passes
- `src/aether/`
  - moves from mutating `CoreExpr` in place to producing and transforming Aether form
- `src/cfg/`
  - lowers from Aether form for RC backends instead of matching Aether nodes in Core
- `src/lir/lower.rs`
  - same change as CFG/VM path
- tests
  - assertions on `CoreExpr::AetherCall`, `Dup`, `Drop`, `Reuse`, and `DropSpecialized` move to Aether-layer tests

Dump surfaces also change:

- `--dump-core` snapshots become cleaner
- `--dump-aether` becomes the canonical ownership/reuse surface

Parity/debug guidance does not change in spirit:

- inspect `--dump-core` first
- inspect `--dump-aether` only after Core looks correct

## Drawbacks
[drawbacks]: #drawbacks

1. **This is a real refactor, not just an enum cleanup**

The compiler currently assumes that post-Aether code is still `CoreExpr`. Moving to a backend-only Aether layer affects:

- lowering boundaries
- tests
- dumps
- ownership analyses

2. **There will be a temporary mixed state during staging**

Stage 1 necessarily introduces some transitional plumbing before Stage 3 removes the old variants completely.

3. **Aether gets its own data model**

This adds more types to the codebase. That cost is intentional: it restores the semantic/backend boundary and avoids hiding backend structure in Core.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not encode Aether as Core primops?

Because it keeps backend-only structure inside semantic Core.

It also fits poorly for the most important Aether constructs:

- `AetherCall` is a call contract, not a primitive operation
- `Reuse` is a structural constructor/rewrite with field-mask semantics
- `DropSpecialized` is branching control flow

Encoding those as Core primops would only rename the leak, not remove it.

### Why not keep Aether in `CoreExpr`?

Because that conflates:

- semantic program representation
- backend-specific ownership planning

and makes every future backend carry knowledge of RC-specific structure even if it never uses RC.

### Why a separate Aether layer?

Because it gives Flux the correct architectural split:

- clean semantic Core
- backend-only ownership lowering

without introducing a second semantic IR.

This also provides the right type-level separation:

- pre-Aether code is `Core`
- post-Aether RC lowering is `Aether`

That is exactly the distinction the current design tries to express informally with pipeline discipline alone.

## Prior art
[prior-art]: #prior-art

### GHC (Haskell)

GHC’s Core is semantic. Backend and runtime concerns live below Core rather than as ad hoc semantic IR variants. Flux should follow the same architectural discipline even though its memory model differs.

### Backend-specific lowering layers in optimizing compilers

Many compilers keep a clean high-level IR and then derive lower, backend-oriented forms for ownership, layout, and code generation. The important lesson is not the exact IR names; it is keeping semantic IR and backend memory-management structure separate.

### Flux’s own architecture contract

Flux already states that:

- `Core` is the only semantic IR
- backend IR belongs below Core

Making Aether backend-only is simply aligning the implementation with that contract.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. **Aether type shape**

The proposal recommends `AetherExpr`, `AetherDef`, and `AetherProgram`, but the exact split between expression-level and program-level metadata still needs to be chosen during implementation.

2. **Borrow metadata placement**

`BorrowRegistry` remains compiler-owned metadata, but implementation still needs to decide exactly how much of that metadata is copied into Aether nodes versus referenced alongside them.

3. **Stage 1 compatibility plumbing**

Stage 1 allows temporary internal compatibility while the clean-Core boundary is introduced. Implementation must keep that temporary state narrow and short-lived.

## Future possibilities
[future-possibilities]: #future-possibilities

1. **Non-RC backends**

A GC-backed or managed-runtime backend can consume clean Core directly and never run Aether.

2. **Alternative ownership lowerings**

If Flux later experiments with another ownership or allocation strategy, it can be implemented as a different post-Core lowering rather than by modifying semantic Core again.

3. **Cleaner serialization and caching**

A clean semantic Core boundary is easier to cache, diff, serialize, and reuse than a Core representation polluted with backend-specific ownership directives.

4. **Cleaner parity/debug layering**

The distinction between:

- semantic mismatch (`--dump-core`)
- ownership/backend mismatch (`--dump-aether`)

becomes explicit in the compiler architecture rather than being an accidental byproduct of how Core is currently mutated.
