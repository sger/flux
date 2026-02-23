# Proposal 034: Base PrimOps — Structured PrimOp Layer

**Status:** In Progress (Phases 0, 1, 3, 4 implemented)  
**Date:** 2026-02-19 (updated 2026-02-20)  
**Scope:** Compiler lowering, bytecode VM, Cranelift JIT, runtime primop execution  

---

## Summary

Flux now has a first-class PrimOp layer that coexists with base functions.

Implemented in this branch:
- Shared `PrimOp` enum and execution logic in `src/primop/mod.rs`
- Generic primop bytecode opcode `OpPrimOp(id, arity)` in `src/bytecode/op_code.rs`
- Compiler call lowering for supported direct calls in `src/bytecode/compiler/expression.rs`
- VM primop dispatch in `src/runtime/vm/dispatch.rs` and `src/runtime/vm/primop.rs`
- JIT primop path via `rt_call_primop` in `src/jit/runtime_helpers.rs` and lowering in `src/jit/compiler.rs`
- Base superinstruction path `OpCallBase(idx, arity)` for selected higher-order base functions
- Function-level effect boundary metadata (`EffectSummary`) in debug info
- Example programs in `examples/prims/`

The current strategy is:
- Use PrimOps as a fast execution path.
- Keep existing base functions as compatibility fallback.
- Preserve VM/JIT semantic parity through shared runtime primop execution.

---

## Motivation

Before PrimOps, direct base calls typically used:

```text
OpGetBase(idx) + OpCall(arity)
```

That path adds avoidable overhead:
- Base value materialization on stack
- Generic call dispatch
- Repeated runtime lookup and argument packing

PrimOps encode compiler-known primitive operations directly as runtime operations:

```text
OpPrimOp(id, arity)
```

This keeps the surface minimal while reducing dispatch overhead in both VM and JIT backends.

---

## Current Implemented PrimOp Surface

Implemented PrimOps (36):

- Integer arithmetic: `IAdd`, `ISub`, `IMul`, `IDiv`, `IMod`
- Float arithmetic: `FAdd`, `FSub`, `FMul`, `FDiv`
- Comparisons: `ICmpEq`, `ICmpNe`, `ICmpLt`, `ICmpLe`, `ICmpGt`, `ICmpGe`, `FCmpEq`, `FCmpNe`, `FCmpLt`, `FCmpLe`, `FCmpGt`, `FCmpGe`, `CmpEq`, `CmpNe`
- Array: `ArrayLen`, `ArrayGet`, `ArraySet`
- Map: `MapGet`, `MapSet`, `MapHas`
- String: `StringLen`, `StringConcat`, `StringSlice`
- Effectful: `Println`, `ReadFile`, `ClockNow`, `Panic`

Effect classification is explicit through `PrimEffect`:
- `Pure`
- `Io`
- `Time`
- `Control`

---

## Implemented Architecture

## PrimOps Architecture

```text
Flux Source
   |
   v
AST (Expression::Call)
   |
   +--> PrimOp Resolver (name + arity match)
   |        |
   |        +--> match => emit OpPrimOp(id, arity)
   |        |
   |        +--> no match => existing base/function call lowering
   |
   v
Bytecode / JIT Lowering
   |
   +--> VM path:
   |      OpPrimOp -> VM dispatch -> execute_primop(op, args)
   |
   +--> JIT path:
          compile_primop_call -> rt_call_primop -> execute_primop(op, args)
```

Core architectural principle:
- `execute_primop` is the semantic single source of truth.
- VM and JIT both call into that shared layer, so behavior stays aligned.

Layer responsibilities:
- `src/bytecode/compiler/expression.rs`
  PrimOp detection and bytecode emission.
- `src/bytecode/op_code.rs`
  Bytecode encoding contract (`OpPrimOp` + operand widths).
- `src/runtime/vm/dispatch.rs` + `src/runtime/vm/primop.rs`
  VM decoding and stack argument plumbing.
- `src/jit/compiler.rs`
  PrimOp call lowering in native backend.
- `src/jit/runtime_helpers.rs`
  ABI bridge for JIT-to-runtime primop invocation.
- `src/primop/mod.rs`
  PrimOp enum, metadata, and semantic execution.

Invariants:
- Every `PrimOp` has stable `id`, fixed `arity`, and explicit effect classification.
- VM and JIT must produce identical results/errors for the same PrimOp inputs.
- Compiler PrimOp lowering is opportunistic; fallback always preserves old behavior.

Fallback architecture:
- Fast path: `OpPrimOp`.
- Compatibility path: base functions/functions via existing call infrastructure.
- This avoids a flag day migration and keeps higher-order base behavior intact.

## 1. Compiler Lowering

In `Expression::Call`, compiler attempts primop lowering first:
- `src/bytecode/compiler/expression.rs` (`try_emit_primop_call`)

If a call matches a supported name and arity, it emits:
- `OpPrimOp(primop_id, arity)`

If it does not match, compiler falls back to existing call lowering:
- base path
- closure/function call path

This keeps compatibility and enables incremental migration.

## 2. Bytecode Encoding

`src/bytecode/op_code.rs`:
- Added `OpPrimOp`
- Operand widths: `[u8 primop_id, u8 arity]`

This is a hybrid model:
- Generic primop opcode now
- Option to add dedicated hot opcodes later without changing `PrimOp` semantic layer

## 3. VM Execution

`src/runtime/vm/dispatch.rs`:
- New `OpPrimOp` dispatch arm

`src/runtime/vm/primop.rs`:
- Decodes `primop_id`
- Validates arity
- Pops arguments
- Calls shared `execute_primop(...)`
- Pushes result

Core runtime semantics live in:
- `src/primop/mod.rs`

## 4. JIT Execution

`src/jit/compiler.rs`:
- Direct call primop resolution mirrors compiler mapping
- Emits helper call to `rt_call_primop`

`src/jit/runtime_helpers.rs`:
- Adds `rt_call_primop(ctx, primop_id, args_ptr, nargs)`
- Calls shared `execute_primop(...)`

Result:
- VM and JIT share the same primop semantics and error behavior

---

## Base Functions Relationship

Base Functions are not removed in this phase.

Current model:
- PrimOps for supported direct-call fast paths
- Base Functions remain as:
  - public compatibility surface
  - fallback for unsupported calls
  - primary path for complex and higher-order base functions

This preserves stability while enabling measurable optimization.

---

## Updated Phase Plan

## Phase 0 (Completed in this branch): PrimOp Foundation

Delivered:
- Shared PrimOp runtime layer
- `OpPrimOp` bytecode
- Compiler + VM + JIT integration
- PrimOp examples and validation

## Phase 1 (Completed): Highest ROI Base Migration to True PrimOps

Target base functions:
- `len`
- `abs`, `min`, `max`
- `type_of`
- `is_int`, `is_float`, `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some`
- `to_string`

Goal:
- Route hot simple base functions through direct PrimOp lowering
- Keep base fallback in place

## Phase 2: Medium Complexity Primitive Base Functions

Target base functions:
- Collections: `first`, `last`, go ahea`rest`, `contains`, `slice`
- Strings: `concat`, `trim`, `upper`, `lower`, `starts_with`, `ends_with`, `replace`, `chars`, `substring`
- Maps: `get`, `put`, `has_key`, `is_map`, `keys`, `values`, `delete`, `merge`
- Numeric parsing: `parse_int`, `parse_ints`, `split_ints`

Goal:
- Broaden primop coverage while preserving behavior

## Phase 3 (Completed): Superinstruction for Remaining Base Calls

Add:
- `OpCallBase(idx, arity)` as a fused superinstruction for complex base functions

Likely candidates:
- higher-order and callback-heavy base functions (`map`, `filter`, `fold`, `flat_map`, `any`, `all`, `find`, `sort_by`, `count`)
- other non-true-primop candidates where call overhead still matters

Goal:
- Reduce generic base call overhead without rewriting all complex logic as primops

Implemented details:
- Compiler emits `OpCallBase` only for base-scoped, direct-call allowlisted names:
  - `map`, `filter`, `fold`, `flat_map`, `any`, `all`, `find`, `sort_by`, `count`
- VM executes `OpCallBase` through direct base invocation path (no `Value::Base` callee materialization)
- JIT parity is policy-based (same allowlist/shadowing semantics), while runtime call remains `rt_call_base`

## Phase 4 (Completed): Effect Boundary + Future Effects Integration

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

---

## Non-Goals (Current)

- Immediate deprecation/removal of base functions
- Full world-token effect threading in bytecode/runtime
- Complete rewrite of all higher-order base functions as true primops

---

## Validation Status

Implemented codepath validates with:
- `cargo fmt --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test -q`

Examples:
- `examples/prims/arith_and_compare.flx`
- `examples/prims/collections.flx`
- `examples/prims/string_and_effects.flx`
- `examples/prims/panic_demo.flx`

---

## Files Added/Updated (Implemented)

- `src/primop/mod.rs`
- `src/bytecode/op_code.rs`
- `src/bytecode/compiler/expression.rs`
- `src/runtime/vm/primop.rs`
- `src/runtime/vm/dispatch.rs`
- `src/jit/compiler.rs`
- `src/jit/runtime_helpers.rs`
- `src/lib.rs`
- `examples/prims/*`

---

## Next Recommended Actions

1. Add explicit compiler tests that assert `OpPrimOp` emission for mapped names.
2. Add VM/JIT parity tests per primop category.
3. Benchmark pre/post for `len` and type checks before Phase 1 expansion.
4. Land Phase 1 base migrations with snapshot and perf guardrails.
