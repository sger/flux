- Feature Name: Base PrimOps — Structured PrimOp Layer
- Start Date: 2026-02-19
- Status: Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0034: Base PrimOps — Structured PrimOp Layer

## Summary
[summary]: #summary

Flux now has a first-class PrimOp layer that coexists with base functions.

## Motivation
[motivation]: #motivation

Before PrimOps, direct base calls typically used: Before PrimOps, direct base calls typically used:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Non-Goals (Current)

- Immediate deprecation/removal of base functions
- Full world-token effect threading in bytecode/runtime
- Complete rewrite of all higher-order base functions as true primops

### Non-Goals (Current)

### Non-Goals (Current)

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Current Implemented PrimOp Surface:** - Integer arithmetic: `IAdd`, `ISub`, `IMul`, `IDiv`, `IMod` - Float arithmetic: `FAdd`, `FSub`, `FMul`, `FDiv` - Comparisons: `ICmpEq`...
- **Current Implemented PrimOp Surface:** Implemented PrimOps (36): - Integer arithmetic: `IAdd`, `ISub`, `IMul`, `IDiv`, `IMod` - Float arithmetic: `FAdd`, `FSub`, `FMul`, `FDiv` - Comparisons: `ICmpEq`, `ICmpNe`, `ICm...
- **PrimOps Architecture:** Core architectural principle: - `execute_primop` is the semantic single source of truth. - VM and JIT both call into that shared layer, so behavior stays aligned.
- **1. Compiler Lowering:** In `Expression::Call`, compiler attempts primop lowering first: - `src/bytecode/compiler/expression.rs` (`try_emit_primop_call`)
- **2. Bytecode Encoding:** `src/bytecode/op_code.rs`: - Added `OpPrimOp` - Operand widths: `[u8 primop_id, u8 arity]`
- **3. VM Execution:** `src/runtime/vm/primop.rs`: - Decodes `primop_id` - Validates arity - Pops arguments - Calls shared `execute_primop(...)` - Pushes result

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Non-Goals (Current)

- Immediate deprecation/removal of base functions
- Full world-token effect threading in bytecode/runtime
- Complete rewrite of all higher-order base functions as true primops

### Non-Goals (Current)

### Non-Goals (Current)

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

### Phase 4 (Completed): Effect Boundary + Future Effects Integration

Focus:
- Keep effectful ops (`print*`, file/time reads, panic-like control) explicit in metadata
- Prepare lowering path for future algebraic effects

Goal:
- Smooth migration from implicit runtime ordering to effect-aware IR in future work

Implemented details:
- PrimOp effect kinds remain explicit in `PrimOp::effect_kind()`:
  - `Pure`, `Io`, `Time`, `Control`
- Compiler records per-function `EffectSummary` in debug info:
  - `Pure`, `Unknown`, `HasEffects`
- Summary is computed during instruction emission:
  - effectful primops => `HasEffects`
  - generic calls / fused base calls => `Unknown` (conservative boundary)
  - otherwise `Pure`
- Metadata is preserved through bytecode cache serialization.
