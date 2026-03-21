- Feature Name: Aether Memory Model
- Start Date: 2026-03-08
- Status: Implemented (foundation landed; ongoing maturity work tracked in 0114)
- Proposal PR:
- Flux Issue:

# Proposal 0084: Aether Memory Model

## Summary
[summary]: #summary

Status note:
The Aether architecture described here is the landed memory-model foundation.
Follow-on precision, coverage, and maturity work now lives in proposal 0114.

**Aether** is Flux's memory model. It replaces the legacy mark-and-sweep GC with
pure reference counting (`Rc`) and progressively adds compile-time optimizations
inspired by Perceus (Reinking et al., PLDI 2021).

Aether is designed for Flux's specific requirements: three execution backends
(VM, JIT, LLVM), algebraic effects with continuations, and future actor-based
concurrency.

Aether unifies proposals 0068 (uniqueness analysis), 0069 (in-place reuse), and
0070 (GC heap elimination) under a single coherent design.

### Core principles

1. Values are **semantically immutable** at the language level.
2. The runtime uses **reference counting (`Rc`) as the sole ownership mechanism**.
3. Storage may be **reused internally** when uniqueness makes that observationally safe.
4. VM, JIT, and LLVM must preserve the **same observable memory semantics**.
5. **No ownership syntax leakage** — Aether is invisible to Flux programmers.

### Aether vs Perceus — honest assessment

| Aspect | Perceus (Koka) | Aether (Flux) — Current | Aether — Target (Phase 7) |
|--------|---------------|------------------------|--------------------------|
| RC insertion | Compile-time (Core IR dup/drop) | Automatic (Rust's Rc) | Compile-time (Core IR dup/drop) |
| Borrowing | Static parameter analysis (`Borrowed.hs`) | None | Phase 6 — skip dup/drop for non-escaping values |
| Reuse tokens | `reuse` binder → zero-alloc updates | None — `Rc::new()` every time | Phase 7 — reuse freed allocations |
| In-place mutation | Yes, when uniquely owned | Partial (`Rc::try_unwrap` moves fields) | Full, via reuse tokens |
| GC | None | **None** (eliminated in Phase 4) | None |
| Drop specialization | Type-specific generated drops | Iterative drop for cons spines | Type-specific drops via Core IR |
| Scope | One backend (C) | Three backends (VM, JIT, LLVM) | Three backends |
| Effect awareness | Implicit | Explicit reuse barriers | Explicit |
| Actor awareness | None (single-threaded) | Future | Future |

**Current state (post Phase 4):** Aether is "Rc everywhere" — correct, GC-free, and
simpler than before, but not yet Perceus-level. The compile-time optimizations in
Phases 5-7 are what close the gap.

## Motivation
[motivation]: #motivation

### Problem solved (Phases 0-4)

Flux previously had **two memory management systems** running simultaneously:

1. **`Rc<T>`** — used by most `Value` variants (Array, String, Closure, Some/Left/Right)
2. **`GcHeap` (mark-and-sweep)** — used by `Value::Gc` for cons lists, HAMT maps, and ADTs

This dual system caused: two allocation paths, two deallocation mechanisms,
`GcHandle` indices that can't cross actor boundaries, and no unified reuse strategy.

**Phases 0-4 eliminated the GC entirely.** All values now use `Rc`.

### Existing infrastructure

The codebase has significant ownership infrastructure:

| Infrastructure | Location | What it does |
|---|---|---|
| `OpConsumeLocal` / `OpConsumeLocal0` / `OpConsumeLocal1` | `op_code.rs` | Move semantics at VM level |
| `stack_take()` | `vm/mod.rs` | Replace slot with `Uninit`, return owned value |
| `Rc::try_unwrap` in `OpUnwrapSome/Left/Right` | `vm/dispatch.rs` | Move inner value when wrapper is unique |
| `OpIsAdtJumpLocal` + `OpConsumeLocal` + `OpAdtFields2` | `vm/dispatch.rs` | Full ADT move pipeline keeping `Rc::strong_count == 1` |
| `consumable_local_use_counts` | `compiler/mod.rs` | Scope-stacked use counting per binding |
| `collect_consumable_param_uses()` | `compiler/expression.rs` | AST walk counting identifier uses |
| `try_emit_consumed_param()` | `compiler/expression.rs` | Decision: count == 1 AND local → emit `OpConsumeLocal` |
| `AdtFields::into_two()` / `into_nth()` | `runtime/value.rs` | Move semantics for ADT fields |

### Remaining problem (Phases 5-7)

With Rc as the sole mechanism, the overhead profile is:

1. **Redundant Rc::clone/drop pairs** — cloning a value only to drop it two instructions later
2. **No borrowing** — function parameters are always Rc::clone'd even when only read
3. **No allocation reuse** — every cons/ADT/tuple allocates fresh even when the old one is dead

Perceus solves all three at compile time. Phases 5-7 bring those optimizations to Flux.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What Aether means for Flux programmers

Nothing changes at the surface level. Flux programmers write pure code:

```flux
fn increment_all(xs: Array<Int>) -> Array<Int> {
    map(xs, \x -> x + 1)
}
```

Aether makes this efficient behind the scenes:

- if `xs` is uniquely owned (last use), `map` reuses the `Vec` in-place
- if `xs` is shared (used again later), `map` allocates a fresh `Vec`
- both produce identical observable results

### What Aether is not

- a user-visible ownership system (no `unique` keyword, no borrow annotations)
- a direct copy of Perceus (Flux has 3 backends, actors, effects — Koka doesn't)
- permission for arbitrary mutation of shared values
- a license for backends to diverge in behavior

### Core guarantees

1. **Purity preserved** — reuse never changes language-visible behavior
2. **No aliasing surprises** — reuse only when value is not observably shared
3. **Backend parity** — VM, JIT, and LLVM produce identical results
4. **Fallback safety** — failed uniqueness proofs degrade to safe allocation

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Ownership states

```text
Fresh       - newly allocated, uniquely owned (Rc::strong_count == 1 guaranteed)
Unique      - exactly one live owning reference proven or checked
Shared      - multiple live owners may exist
Escaped     - crosses analysis boundary (closure capture, handler, continuation)
Transferred - moved across actor boundary (future)
```

### Reuse policy

Reuse is permitted only when ALL of:

1. Value is uniquely owned or proven fresh
2. Operation preserves identical observable result
3. No actor boundary crossed illegally
4. No effect handler requires old structure to remain materialized
5. Specific representation supports safe reuse

If any condition unmet → allocate fresh (correctness over performance).

### Reuse barriers

1. **Handler boundary** — values captured by effect handlers or continuations
2. **Closure capture** — variable captured by lambda that may be called multiple times
3. **Unknown aliasing** — globals, polymorphic calls, imported functions
4. **Foreign/runtime boundary** — host callbacks, JIT helper calls, debug tooling
5. **Actor boundary** — values sent between actors (future)

### Where Aether fits in the Flux compilation pipeline

```text
Source → Lexer → Parser → AST → HM Type Inference
                                       │
                    ┌──────────────────▼──────────────────┐
                    │         Core IR (core/)              │
                    │                                      │
                    │  Existing passes (unchanged):        │
                    │  beta → cokc → case_of_case →        │
                    │  inline → dead_let → evidence → anf  │
                    │                                      │
                    │  Phase 5: aether_rc                  │ ← dup/drop insertion
                    │  Phase 6: aether_borrow              │ ← borrow elision
                    │  Phase 7: aether_reuse               │ ← reuse token insertion
                    │                                      │
                    └──────────────────┬──────────────────┘
                                       │
                    ┌──────────────────▼──────────────────┐
                    │         CFG IR (cfg/)                 │
                    │                                      │
                    │  Dup/Drop/Reuse lower to explicit     │
                    │  Rc operations in IrInstr             │
                    └──────────────────┬──────────────────┘
                                       │
              ┌────────────────────────┼────────────────────────┐
              │                        │                        │
     ┌────────▼────────┐     ┌────────▼────────┐     ┌────────▼────────┐
     │   VM Backend     │     │   JIT Backend    │     │  LLVM Backend   │
     │                  │     │                  │     │                 │
     │  OpDup/OpDrop    │     │  Dup/Drop →      │     │  Dup/Drop →     │
     │  opcodes         │     │  inline RC ops   │     │  LLVM intrinsics│
     │  + Rc::try_unwrap│     │  or rt_* calls   │     │  (ARC-style)    │
     └──────────────────┘     └──────────────────┘     └─────────────────┘
              │                        │                        │
              └────────────────────────┼────────────────────────┘
                                       │
                    ┌──────────────────▼──────────────────┐
                    │       Shared Runtime (runtime/)       │
                    │                                      │
                    │  ✅ Phase 0: base function Rc reuse  │
                    │  ✅ Phase 1-3: GcHeap → Rc migration │
                    │  ✅ Phase 4: GcHeap deleted           │
                    └──────────────────────────────────────┘
```

**Key insight from Koka:** In Koka, Perceus (`Parc.hs`) runs on Core IR *after* all
semantic optimization passes but *before* C code generation. This is the same position
Aether Phase 5 targets — after the existing 7 Core passes, before CFG lowering.

**Why not earlier?** Core passes (beta reduction, inlining, dead let elimination) change
the use patterns of variables. Running Aether dup/drop insertion before these passes
would produce incorrect results. The passes must stabilize the code first.

**Why not later (at CFG level)?** Dup/drop is a semantic operation tied to variable
lifetimes, not control flow. Core IR's tree structure makes lifetime analysis natural.
CFG's block-and-jump structure would require dataflow analysis for the same information.

## Phased rollout
[phased-rollout]: #phased-rollout

### Phase 0: Runtime Rc reuse fast paths ✅ IMPLEMENTED

VM-level optimizations using existing infrastructure:

1. **`Rc::try_unwrap` fast paths in base functions**
   - `base_map`: in-place array mutation when `Rc::strong_count == 1`
   - `base_filter`: in-place filtering when uniquely owned
   - Files: `src/runtime/base/higher_order_ops.rs`

2. **`Rc::try_unwrap` in VM opcodes**
   - `OpConsHead`: move head when cons cell is unique
   - `OpConsTail`: move tail when cons cell is unique
   - `OpTupleIndex`: move element when tuple is unique
   - File: `src/bytecode/vm/dispatch.rs`

### Phase 1: Cons list migration ✅ IMPLEMENTED

Migrated cons lists from `GcHeap` to `Rc<ConsCell>`:

1. `ConsCell` struct with iterative `Drop` (stack-overflow prevention)
2. `Value::Cons(Rc<ConsCell>)` variant replaces `Value::Gc(HeapObject::Cons)`
3. All VM opcodes, base functions, and JIT/LLVM helpers updated
4. File: `src/runtime/cons_cell.rs`

### Phase 2: ADT unification ✅ IMPLEMENTED

Unified all ADTs under `Value::Adt(Rc<AdtValue>)`:

1. `Value::GcAdt` variant removed
2. `AdtRef` simplified to single-variant newtype
3. `NanTag::GcAdt` freed (tag 0x7)
4. All VM opcodes, base functions, and JIT/LLVM helpers updated

### Phase 3: HAMT migration ✅ IMPLEMENTED

Migrated HAMT maps from `GcHeap` to `Rc<HamtNode>`:

1. Complete Rc-based HAMT rewrite in `src/runtime/hamt.rs`
2. `Value::HashMap(Rc<HamtNode>)` variant replaces `Value::Gc(HeapObject::HamtNode)`
3. All 8 HAMT base functions, VM hash building, and JIT/LLVM helpers updated
4. Self-contained module with no GcHeap dependency

### Phase 4: GcHeap elimination ✅ IMPLEMENTED

Removed the GC entirely:

1. `Value::Gc(GcHandle)` variant removed
2. `NanTag::GcHandle` freed (tag 0x6)
3. All GC-related code paths deleted from VM dispatch, base functions, native helpers
4. `GcHeap` struct retained as empty stub (pending cleanup)

### Phase 5: Compile-time dup/drop insertion

**Goal:** Eliminate redundant `Rc::clone()` and `Rc::drop()` at compile time.
**Estimated effort: 4-6 weeks. Depends on Phase 0-4 measured.**

This is the core Perceus algorithm adapted for Flux. For each variable in Core IR,
analyze its usage pattern and insert explicit `Dup` (increment refcount) and `Drop`
(decrement refcount) operations at precise points.

#### 5.1 New Core IR nodes

```rust
pub enum CoreExpr {
    // ... existing variants ...
    Dup {
        var: CoreVarRef,
        body: Box<CoreExpr>,  // dup var; body
        span: Span,
    },
    Drop {
        var: CoreVarRef,
        body: Box<CoreExpr>,  // drop var; body
        span: Span,
    },
}
```

#### 5.2 Dup/drop insertion rules (from Perceus)

The algorithm walks Core IR after ANF normalization. For each `Let` binding:

```text
let x = e in body
```

1. Count occurrences of `x` in `body`:
   - **0 uses**: insert `Drop(x)` immediately after binding
   - **1 use**: no dup needed, no drop needed (ownership transferred)
   - **N uses (N > 1)**: insert `Dup(x)` before each use except the last

2. For `Case` expressions:
   - Each alternative may use different variables
   - Insert drops at the START of each alternative for variables NOT used in that arm
   - Insert dups for variables used in MULTIPLE arms

3. For `Lam` (closures):
   - Captured variables need `Dup` at capture point
   - The closure body owns its copy

4. For `Con` (constructors):
   - Fields consumed by the constructor — no dup needed if last use
   - Otherwise dup before passing to constructor

#### 5.3 Optimization passes on dup/drop

After insertion, run optimization passes:

- **Dup-drop fusion**: `dup x; ... drop x` where no intervening use → eliminate both
- **Drop reorder**: move drops earlier to free memory sooner (reduce peak usage)
- **Dup push-through**: delay dups until actual use point (reduce live references)

#### 5.4 Lowering to backends

Core IR `Dup`/`Drop` nodes lower through CFG IR to backend-specific operations:

- **VM**: `OpDup(local)` → `Rc::clone()`, `OpDrop(local)` → replace slot with `Uninit`
- **JIT**: Inline `Rc::clone`/drop or call `rt_dup`/`rt_drop` helpers
- **LLVM**: LLVM intrinsics or `rt_dup`/`rt_drop` calls

#### 5.5 Updating existing Core passes

All 7 existing Core passes must be updated to handle `Dup`/`Drop` nodes:

- **beta, inline, dead_let**: Treat `Dup`/`Drop` as transparent wrappers
- **case_of_case, cokc**: Propagate dup/drop through case transformations
- **evidence**: Effect handler rewriting must preserve dup/drop
- **anf**: Dup/drop are already in A-normal form (no nested subexpressions)

**Important**: The Aether pass runs AFTER all existing passes, so existing passes
only need to handle dup/drop if they run in a second round of optimization. Initially,
run Aether as the final Core pass with no re-optimization.

#### 5.6 Expected impact

- 40-60% fewer `Rc::clone()` calls in typical functional code
- Measurable with `--stats` comparing before/after Rc operation counts

### Phase 6: Borrowing analysis

**Goal:** Skip dup/drop entirely for values that are only read, not consumed.
**Estimated effort: 3-4 weeks. Depends on Phase 5.**

A function parameter that is only read (not stored in a data structure, not returned,
not captured by a closure) doesn't need its refcount touched at all.

#### 6.1 Borrowed parameter detection

Analyze each function's parameters:

```text
fn f(x, y) =
  let a = x + 1    -- x is READ (borrowed) — no dup needed
  let b = Cons(y, Nil)  -- y is CONSUMED — dup if used again
  a
```

A parameter is **borrowed** if all its uses are in "read" positions:
- Operand of a `PrimOp` (arithmetic, comparison, string ops)
- Scrutinee of a `Case` (pattern matching reads, doesn't consume)
- Argument to a function known to borrow its parameter

A parameter is **owned** if any use is in a "consume" position:
- Field of a `Con` (stored in data structure)
- Returned from the function
- Captured by a `Lam`
- Passed to a function that owns its parameter

#### 6.2 Inter-procedural borrowing

For known call sites (not higher-order), propagate borrowing info:

```text
fn g(x) = x + 1       -- g borrows x
fn h(xs) = map(xs, g)  -- map owns xs (stores it), borrows g
```

Start with intra-procedural analysis (Phase 6a), extend to inter-procedural
for known calls (Phase 6b).

#### 6.3 Effect handler boundaries

Values captured by continuations must NOT be borrowed across the capture point.
A `Perform` expression is a reuse barrier — all live borrowed values must be
dup'd before the perform.

#### 6.4 Expected impact

- Near-zero RC overhead for leaf functions (arithmetic, comparison, formatting)
- 10-20% additional reduction on top of Phase 5

### Phase 7: Reuse tokens

**Goal:** When a drop makes refcount hit zero, reuse the freed memory for the
next allocation of the same shape. Zero-allocation functional updates.
**Estimated effort: 4-6 weeks. Depends on Phase 6.**

This is the signature Perceus optimization. When you pattern match on a cons cell
and immediately construct a new one, the old cell's memory is reused.

#### 7.1 New Core IR node

```rust
pub enum CoreExpr {
    // ... existing variants ...
    Reuse {
        token: CoreBinder,    // reuse token from a drop
        tag: CoreTag,         // constructor to reuse for
        fields: Vec<CoreExpr>,
        span: Span,
    },
}
```

#### 7.2 Reuse analysis

After dup/drop insertion, scan for `Drop` followed by `Con` of compatible shape:

```text
-- Before reuse analysis:
case xs of
  Cons(h, t) ->
    drop xs            -- xs is dead after destructure
    let h' = h + 1
    Cons(h', t)        -- allocates fresh cons cell

-- After reuse analysis:
case xs of
  Cons(h, t) ->
    let token = drop_reuse xs   -- drop returns reuse token
    let h' = h + 1
    reuse token Cons(h', t)     -- reuse old cons cell's memory
```

#### 7.3 Shape compatibility

A reuse token from type A can be used for type B if:
- Same number of fields
- Same or smaller total field size
- Both are heap-allocated (Rc-wrapped)

For Flux's Value representation:
- `ConsCell` (2 fields) can reuse `ConsCell`
- `AdtValue` with N fields can reuse `AdtValue` with ≤ N fields
- Tuples of same arity can reuse each other

#### 7.4 Runtime representation

```rust
pub struct ReuseToken {
    ptr: *mut u8,        // raw pointer to freed allocation
    capacity: usize,     // size of the allocation
}
```

When `Rc::try_unwrap` succeeds in a `drop_reuse`:
1. Move all fields out (they're owned by the match bindings)
2. Keep the allocation alive as a `ReuseToken`
3. `reuse token Con(...)` writes new fields into the existing allocation

When `Rc::try_unwrap` fails (shared):
1. Return a null token
2. `reuse null_token Con(...)` falls back to `Rc::new()` (normal allocation)

#### 7.5 Expected impact

- Zero-allocation `map`, `filter` over lists when uniquely owned
- Zero-allocation ADT updates (e.g., tree rotations, record updates)
- Benchmarks should match Koka's performance for functional update patterns

### Phase 8: Actor transfer (future)

Transfer semantics for uniquely owned actor messages. Only after Phases 5-7
are correct and measured.

## Implementation status

| Phase | Status | Key changes |
|-------|--------|-------------|
| Phase 0 | ✅ Implemented | `Rc::try_unwrap` fast paths in `base_map`, `base_filter`, `OpConsHead`, `OpConsTail`, `OpTupleIndex` |
| Phase 1 | ✅ Implemented | `ConsCell` struct, `Value::Cons(Rc<ConsCell>)`, iterative `Drop` |
| Phase 2 | ✅ Implemented | `Value::GcAdt` removed, all ADTs unified under `Value::Adt(Rc<AdtValue>)` |
| Phase 3 | ✅ Implemented | Rc-based HAMT in `src/runtime/hamt.rs`, `Value::HashMap(Rc<HamtNode>)` |
| Phase 4 | ✅ Implemented | `Value::Gc` removed, `NanTag::GcHandle` freed, GC code paths deleted |
| Phase 5 | ✅ Implemented | Core IR `Dup`/`Drop` insertion, borrowing elision for read-only params |
| Phase 6 | ✅ Implemented | Borrowing analysis (`owned_use_count` skips dup for borrowed positions) |
| Phase 7 | ✅ Implemented | Reuse tokens (`Reuse` node, `rt_reuse_cons`/`rt_reuse_adt` runtime helpers, backend emission) |
| Phase 8 | 📋 Future | Actor transfer semantics |

## Expected impact

| Phase | What improves | Estimated gain |
|-------|--------------|----------------|
| Phase 0 ✅ | Array `map`/`filter` on unique inputs | 2-4× for these operations |
| Phase 1 ✅ | Cons list operations (no GC pauses) | Eliminates GC cycles for list-heavy code |
| Phase 2 ✅ | ADT operations (unified path) | Simpler code, slight perf improvement |
| Phase 3 ✅ | HAMT operations (no GC pauses) | Eliminates GC cycles for map-heavy code |
| Phase 4 ✅ | Overall (no dual memory system) | Simpler runtime, predictable perf |
| Phase 5 | All Rc operations (compile-time dup/drop) | 40-60% fewer Rc::clone() calls |
| Phase 6 | Function-local values (borrowing) | Near-zero RC overhead for leaf functions |
| Phase 7 | Functional updates (reuse tokens) | Zero-allocation map/filter/tree operations |

## Drawbacks
[drawbacks]: #drawbacks

1. **Core IR complexity** — Phase 5 adds Dup/Drop nodes, Phase 7 adds Reuse nodes.
   All 7 existing Core passes must be updated. Mitigated by running Aether as the
   final pass (existing passes don't need to handle new nodes initially).
2. **Three-backend constraint** — Dup/Drop/Reuse must lower correctly to VM, JIT,
   and LLVM. Each backend has different optimization opportunities.
3. **Effect handler interaction** — Perform/Handle create reuse barriers that limit
   optimization. The analysis must be conservative at handler boundaries.
4. **Incremental complexity** — Each phase adds optimization machinery. Risk of
   the Core IR becoming hard to debug. Mitigated by `--dump-core` showing dup/drop.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not keep tracing GC as the main model?

Eliminated in Phases 0-4. Rc is simpler, has no pauses, and enables reuse analysis.

### Why not adopt Perceus verbatim?

Flux is not Koka:
- Flux has 3 backends (VM, JIT, LLVM) — Koka has 1 (C)
- Flux has algebraic effects with runtime continuations
- Flux will have actor-based concurrency
- Flux has its own Value representation and NaN-boxing

Perceus is prior art and inspiration, not a drop-in plan.

### Why not expose ownership in the syntax?

Flux should stay pure, approachable, and free of low-level ownership burden.
Aether is a compiler/runtime optimization, not a language feature.

### Why Phase 5 before Phase 7?

Dup/drop insertion (Phase 5) is prerequisite for both borrowing (Phase 6) and
reuse tokens (Phase 7). Borrowing elides dup/drop pairs. Reuse transforms
specific drop+allocate patterns. Without explicit dup/drop in the IR, neither
analysis has anything to optimize.

### Why A→B→C ordering?

- **A (dup/drop)**: Highest ROI — eliminates 40-60% of redundant RC operations
  with no new runtime concepts. Pure compile-time optimization.
- **B (borrowing)**: Builds on A — identifies dup/drop pairs that can be removed
  entirely. Lower effort once A is in place.
- **C (reuse tokens)**: Builds on A+B — requires new runtime representation
  (ReuseToken) and careful interaction with all backends. Highest complexity,
  but delivers the signature Perceus win (zero-alloc functional updates).

## Prior art
[prior-art]: #prior-art

- **Perceus**: Garbage Free Reference Counting with Reuse (Reinking, Xie, Leijen, PLDI 2021)
- **Reference Counting with Frame Limited Reuse** (Lorenzen, Leijen, 2022)
- **Optimizing Reference Counting with Borrowing** (Lorenzen, master thesis)
- **Koka compiler**: `Backend/C/Parc.hs` (dup/drop insertion), `ParcReuse.hs` (reuse analysis),
  `Core/Borrowed.hs` (borrowing analysis)
- **Swift ARC optimization**: retain/release elimination on SIL
- **Clean language**: uniqueness types for in-place mutation
- **Lobster**: compile-time reference counting with borrowing

## Relationship to other proposals

| Proposal | Relationship to Aether |
|----------|----------------------|
| **0045** (GC) | Superseded — GC eliminated in Phase 4 |
| **0068** (uniqueness analysis) | Subsumed — Phase 5 implements full analysis |
| **0069** (Rc::get_mut fast paths) | Subsumed — Phase 0 (runtime), Phase 7 (compile-time reuse) |
| **0070** (GcHandle elimination) | Subsumed — Phases 1-4 implemented |
| **0109** (VM optimizations) | Complementary — dispatch optimizations independent of Aether |
| **0110** (JIT optimizations) | Complementary — JIT benefits from Aether evidence |
| **0111** (LLVM optimizations) | Complementary — LLVM benefits from Aether evidence |
| **0112** (shared pipeline) | Complementary — Core IR caching independent |

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. ~~Should GC be fully eliminated or kept as a fallback?~~ → Eliminated (Phase 4).
2. ~~Should iterative drop use a loop or trampoline?~~ → Loop with `Rc::try_unwrap` (Phase 1).
3. ~~What representation for empty HAMT maps?~~ → `Rc<HamtNode>` with empty bitmap (Phase 3).
4. How should handler-aware uniqueness evidence be represented in Core IR?
5. Should Aether diagnostics surface in `--dump-core` or a separate `--dump-aether` flag?
6. What is the right granularity for reuse token shape compatibility?
7. How should mutual recursion interact with dup/drop insertion?

## Future possibilities
[future-possibilities]: #future-possibilities

- Transfer semantics for uniquely owned cross-actor values
- Region-like short-lived allocation strategies layered under Aether
- Adaptive heuristics for reuse vs rebuild for small values
- Ownership-aware optimization reports in `flux analyze`
- Specialized Aether paths for persistent collection combinators
- `unique` keyword in function signatures for guaranteed ownership transfer
