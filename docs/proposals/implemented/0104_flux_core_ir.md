- Feature Name: Flux Core IR
- Start Date: 2026-03-15
- Proposal PR: pending
- Flux Issue: pending
- Status: Implemented
- Depends on: 0086 (backend-neutral core ir), 0100 (core ir consolidation)

# Proposal 0104: Flux Core IR

## Summary

Adopt Flux Core as the compiler's canonical semantic IR and require production
backends to consume Core rather than lowering directly from AST or from
ad hoc intermediate layers. Core sits between typed AST/HM inference and a
single backend IR, carrying explicit lambdas, applications, case analysis,
constructors, primitive operations, `perform`, and `handle`.

The target long-term pipeline is:

```text
Surface AST -> Typed Flux Core -> Backend IR -> VM / JIT / future backends
```

`src/core` now owns the semantic Core implementation. `crate::backend_ir` now
exists as the canonical backend-IR API over today's CFG implementation.
`src/ir` and `src/cfg` do not merge into Core; they converge into a single
backend IR below Core. `src/ir` has now been retired; production ownership
lives in `backend_ir`, `cfg` remains the private backend engine behind that
boundary, and `shared_ir` is reduced to shared ID types only.

## Motivation

Flux currently has the right idea and the wrong center of gravity.

- Core now exists as the canonical semantic layer in both API and implementation.
- `src/ir` and `src/cfg` are both backend-shaped IRs with heavy duplication.
- AST still lowers directly into backend-facing IR in the main production path.
- Function identity and lowering joins are still reconstructed in places by
  names rather than preserved structurally.

That architecture works, but it does not scale toward the intended design
target: a compiler with a small, explicit, effect-aware semantic core in the
style of Koka, Haskell Core, or the OCaml middle-end.

The project needs a clear answer to these questions:

1. Which IR is the semantic source of truth?
2. Which IR is the backend source of truth?
3. Where do effect semantics live?
4. Where do binding identity and optimization really belong?

This proposal answers them directly:

- Core IR is the semantic source of truth.
- Backend IR is a separate, lower-level representation for VM/JIT codegen.
- Effects remain explicit through Core.
- Binding identity becomes structural at Core instead of name-reconstructed
  later.

## Guide-level explanation

Compiler contributors should think about Flux as a four-stage pipeline:

```text
Surface AST
  -> Typed Flux Core
  -> Backend IR
  -> VM / JIT / future backends
```

### What Core is for

Core is where the program becomes compiler-real:

- sugar is gone
- named control flow is explicit
- lambdas and applications are explicit
- `match` is normalized into `Case`
- operators are normalized into `PrimOp`
- effects remain explicit as `Perform` and `Handle`

Core is the right layer for:

- beta reduction
- inlining
- let simplification
- case simplification
- effect-aware simplification
- future handler/evidence optimizations

### What Core is not for

Core is not where backend details should appear. It should not know about:

- basic blocks
- GC root plumbing
- stack slots
- bytecode opcodes
- Cranelift helper calling conventions
- ad hoc name-patching to reconnect lowered functions

### Contributor rule of thumb

If a change is about *language meaning*, it belongs in Core.

If a change is about *execution strategy*, it belongs in backend IR.

If a backend needs language meaning from the AST directly, the architecture is
wrong and the missing lowering belongs in Core.

## Reference-level explanation

### 1. Canonical semantic layer

Flux Core should standardize on a small expression language:

- `Var`
- `Lit`
- `Lam`
- `App`
- `Let`
- `LetRec`
- `Case`
- `Con`
- `PrimOp`
- `Perform`
- `Handle`

`src/core` is the canonical semantic IR layer. New code should treat
`crate::core` as the only supported semantic-IR boundary.

### 2. Binding identity

Core should move toward stable binder identity instead of relying on names as
the semantic key.

Target direction:

- binders get explicit IDs
- variable references point to binder IDs
- source names become metadata/debug information

This proposal does not require binder-ID migration in phase 1, but it does make
it the architectural direction for future Core work.

### 3. Effects remain explicit in Core

Core must keep:

- `Perform { effect, operation, args }`
- `Handle { body, effect, handlers }`

These remain semantic constructs in Core rather than being immediately lowered
into backend-specific runtime plumbing. That keeps effect optimizations and
handler strategy changes possible without rewriting frontend lowering.

### 4. Backend IR unification

The project should converge on one backend IR family rather than maintaining
both `src/ir` and `src/cfg` as near-duplicate lowering/passes/validation
universes.

The chosen backend IR should own:

- blocks / terminators
- SSA-ish variables / block params
- closure layout lowering
- unboxing decisions
- tail-call introduction
- backend-friendly control flow

This proposal leaves open whether `ir` or `cfg` survives, but it does not leave
open the architecture: exactly one backend IR family survives below Core.

### 5. Backend extensibility rule

No long-term backend may lower directly from AST.

Future backends such as C, LLVM, Wasm, AOT native, or alternate interpreters
must accept Core as input and may define backend-specific lowering only below
that point. Shared semantic optimizations happen on Core once, not once per
backend.

### 6. Migration plan

#### Phase 1: Canonical module boundary

Land a new public `flux::core` module as the canonical semantic-IR entry point.

This phase is intentionally low-risk:

- no semantic changes
- no backend changes
- existing code keeps compiling

#### Phase 2: Prefer Core in new code

New semantic-lowering and optimization work should import `crate::core`.

Completed implementation state:

- `flux::core` now exists as the canonical public semantic-IR module.
- Core type ownership now lives in `src/core/mod.rs`.
- semantic lowering, passes, display, and Core→backend lowering now live under
  `src/core/*`.
- `src/nary` has been retired.

#### Phase 3: Collapse duplicated backend IR work

Stop duplicating lowering/validation/pass logic across `src/ir` and `src/cfg`.
`crate::backend_ir` is the canonical caller boundary during this migration.
Pick one backend IR implementation as canonical and migrate callers behind that
boundary.

Completed implementation state:

- `flux::backend_ir` is the canonical backend-facing boundary for callers.
- `cfg` remains the private concrete backend engine behind `backend_ir`.
- `flux::shared_ir` owns canonical IR ID types only.
- `src/ir` has been removed.

#### Phase 4: Move binding identity into Core

Introduce binder IDs and migrate Core passes/lowering to structural identities.

Completed implementation state:

- `Flux Core` now has explicit `CoreBinderId`, `CoreBinder`, and `CoreVarRef`
  types
- AST→Core lowering allocates binder IDs at bind sites and resolves variable
  references lexically
- Core optimization passes and Core→backend lowering key off binder IDs instead
  of raw names
- Core→backend lowering now treats “resolved binder missing from env” as an
  invariant violation rather than silently falling back to external-name loads
- Core debug/display output renders binder identity explicitly
- binder-aware constructors/helpers now exist for common Core construction paths

#### Phase 5: Delete legacy semantic-lowering paths

Retire direct AST-to-backend lowering once Core-backed lowering covers the same
surface area and has equivalent test coverage.

Completed implementation state:

- the canonical `flux::backend_ir::lower_program_to_ir` entry point is now
  Core-backed (`AST -> Flux Core -> backend IR`) rather than using direct
  AST-to-backend lowering
- production JIT compilation is now backend-only:
  `AST -> Flux Core -> backend IR -> JIT`
- the production AST-backed JIT fallback has been removed
- the bytecode compiler now consumes backend/CFG IR directly on the production
  path rather than converting through a mixed shared container
- production bytecode/JIT code no longer imports `structured_ir` or
  `shared_ir` directly; production IR traffic flows through `core` and
  `backend_ir`
- `shared_ir::IrProgram` has been retired; `shared_ir` now contains shared ID
  types only
- `structured_ir` has been retired

## Drawbacks

- This is an architectural migration, so it creates temporary mixed-state code.
- A compatibility facade can make progress look more complete than it is if the
  repo does not keep the migration plan explicit.
- Binder-ID migration is real work and will touch many passes once started.

## Rationale and alternatives

### Why not keep the old `nary` naming?

Because the semantic layer is no longer “the candidate” for Flux Core. It is
Flux Core. Keeping the old name would preserve ambiguity for contributors and
for future backend work.

### Why not rewrite directly from scratch?

Because the existing implementation was already a strong foundation. Reusing it
and moving it under `src/core` was faster and lower risk than inventing a
fourth semantic IR.

### Why not keep both `ir` and `cfg` indefinitely?

Because the duplication cost is already visible in lowering, passes, and
validation. The current overlap is a transitional compromise, not a strong
steady-state design.

## Prior art

- Koka: explicit effect-aware core language and effect-directed optimizations
- GHC Core: small typed functional core as the center of the compiler
- OCaml Lambda/Flambda middle-end: semantic simplification before backend
  lowering

Flux does not need to copy any of these exactly, but it should adopt the same
discipline of having one clear semantic core and one clear backend IR family.

## Unresolved questions

- Which existing backend IR module (`ir` or `cfg`) should survive the
  backend-layer unification?
- How much effect information should be stored inline on Core nodes versus in a
  side table keyed by node identity?

## Future possibilities

- handler/evidence-passing optimizations on Core
- Core-level effect simplification and dead-effect elimination
- closure conversion as an explicit backend-lowering step
- alternate backends sharing the same Core pipeline
