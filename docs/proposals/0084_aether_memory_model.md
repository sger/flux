- Feature Name: Aether Memory Model
- Start Date: 2026-03-08
- Status: Draft
- Proposal PR:
- Flux Issue:

# Proposal 0084: Aether Memory Model

## Summary
[summary]: #summary

**Aether** is Flux's reuse-oriented reference-counted memory model. It is inspired by
Perceus (Reinking et al., PLDI 2021) but designed for Flux's specific requirements:
three execution backends (VM, JIT, LLVM), algebraic effects with continuations, and
future actor-based concurrency.

Aether unifies proposals 0068 (uniqueness analysis), 0069 (in-place reuse), and 0070
(GC heap elimination) under a single coherent design with a pragmatic phased rollout.

### Core principles

1. Values are **semantically immutable** at the language level.
2. The runtime uses **reference counting (`Rc`) as the baseline ownership mechanism**.
3. Storage may be **reused internally** when uniqueness makes that observationally safe.
4. VM, JIT, and LLVM must preserve the **same observable memory semantics**.
5. **No ownership syntax leakage** — Aether is invisible to Flux programmers.

### Aether vs Perceus

| Aspect | Perceus (Koka) | Aether (Flux) |
|--------|---------------|---------------|
| Scope | Compiler algorithm for one backend (C) | Memory model for 3 backends |
| IR level | Operates on Koka Core IR before C emission | Operates on Core IR before backend split |
| Reuse tokens | `reuse` binder in Core IR | Runtime `Rc::try_unwrap` (Phase 0-3) + Core IR dup/drop (Phase 4+) |
| Borrowing | `Borrowed.hs` — static parameter analysis | Phase 5 — skip dup/drop for non-escaping locals |
| Drop specialization | `Parc.hs` generates type-specific drops | Phase 3 — iterative drop for cons spines |
| Cycle handling | Type system guarantees no cycles | No-cycle invariant (same guarantee) |
| Actor awareness | None — Koka is single-threaded | Explicit actor boundary rules (future) |
| Effect awareness | Implicit — effects compiled away | Explicit reuse barriers at handler/continuation boundaries |
| GC interaction | No GC at all | Transitional: GC for cons/HAMT/ADT now, migrate to Rc |

## Motivation
[motivation]: #motivation

### Current state

Flux has **two memory management systems** running simultaneously:

1. **`Rc<T>`** — used by most `Value` variants (Array, String, Closure, Some/Left/Right)
2. **`GcHeap` (mark-and-sweep)** — used by `Value::Gc` for cons lists, HAMT maps, and
   some ADTs (`Value::GcAdt`)

This dual system creates problems:

- Two different allocation paths, two different deallocation mechanisms
- `GcHandle` is a `u32` index into a global heap — cannot cross actor boundaries
- No unified reuse strategy — `Rc` values don't benefit from uniqueness, GC values
  don't benefit from reference counting
- Base functions like `map`, `filter`, `push` always allocate new collections even
  when the input is uniquely owned and could be reused in-place

### Existing infrastructure (discovered during 0068 analysis)

The codebase already has significant ownership infrastructure that 0068 underestimated:

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

**The gap is scope, not mechanism.** The use-count analysis only runs per match arm body,
not per full function. Extending it to function scope covers ~70% of what 0068 proposed.

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

### GC heap migration (from proposal 0070)

The `GcHeap` currently manages three object types:

| Object | GC alloc sites | Migration target |
|--------|---------------|-----------------|
| `HeapObject::Cons` | ~10 (VM, base functions, list_ops) | `Value::ConsList(Rc<ConsList>)` |
| `HeapObject::Adt` | ~12 (VM, native_helpers) | Unified `Value::Adt` (partially done) |
| `HeapObject::HamtNode` | ~30 (hamt.rs — every insert/delete/merge) | `Value::HamtMap(Rc<HamtNode>)` |

**Critical blocker for cons lists:** Deep `Rc<ConsList>` chains will stack-overflow on
drop (Rust's `Rc` uses recursive drop). Iterative drop is required before migration:

```rust
impl Drop for ConsList {
    fn drop(&mut self) {
        let mut cur = std::mem::replace(&mut self.tail, Value::EmptyList);
        loop {
            match cur {
                Value::ConsList(rc) => match Rc::try_unwrap(rc) {
                    Ok(mut cell) => cur = std::mem::replace(&mut cell.tail, Value::EmptyList),
                    Err(_) => break, // shared tail, stop iterating
                },
                _ => break,
            }
        }
    }
}
```

### Base function reuse fast paths (from proposal 0069)

The highest-impact reuse optimization is in base functions, **not** new opcodes:

```rust
// Example: base_map with Aether reuse
pub fn base_map(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    match &args[0] {
        Value::Array(arr) => {
            let func = &args[1];
            // Aether fast path: if uniquely owned, mutate in-place
            if Rc::strong_count(arr) == 1 {
                let arr_rc = match args.into_iter().next().unwrap() {
                    Value::Array(a) => a,
                    _ => unreachable!(),
                };
                if let Ok(mut vec) = Rc::try_unwrap(arr_rc) {
                    for elem in vec.iter_mut() {
                        *elem = ctx.invoke_unary_value(func, std::mem::replace(elem, Value::None))?;
                    }
                    return Ok(Value::Array(Rc::new(vec)));
                }
            }
            // Fallback: allocate new Vec
            let results: Result<Vec<Value>, String> = arr.iter()
                .map(|elem| ctx.invoke_unary_value(func, elem.clone()))
                .collect();
            Ok(Value::Array(Rc::new(results?)))
        }
        _ => { /* ... */ }
    }
}
```

Apply same pattern to: `base_filter`, `base_push`, `base_sort`, `base_sort_by`.

All 3 backends benefit because base functions are shared runtime code.

**Note on 0069's `OpReuseCheck` / `OpReuseArray` / `Value::ReuseToken`:** These are
deferred to Phase 4+. Adding `Value::ReuseToken` touches 50+ match sites across the
codebase. The base function approach achieves 80% of the benefit at 10% of the cost
for the common operations (`map`, `filter`, `push`).

### Uniqueness analysis (from proposal 0068)

**0068 as written proposes a new `src/ast/uniqueness.rs` module operating on AST.** Based
on our analysis, this should be revised:

**What already exists (extend, don't rebuild):**

| 0068 Feature | Current status |
|---|---|
| Ownership tracking per binding | `consumable_local_use_counts` — exists but per-arm only |
| AST walk for use counting | `collect_consumable_param_uses()` — exists |
| Emit move for last-use locals | `OpConsumeLocal` — exists |
| Decision logic (count == 1 → move) | `try_emit_consumed_param()` — exists |

**What to extend:**

1. Widen `collect_consumable_param_uses()` from per-match-arm to per-function scope
2. Add closure-capture awareness: variable in `collect_free_vars()` of any lambda → Shared
3. Track cross-statement last-use in sequential `let` bindings

**What to defer:**

- Core IR ownership annotations (`CoreExpr::Dup`/`CoreExpr::Drop`) — Phase 4
- `OwnershipMap` as explicit data structure — only if Phase 0-2 prove insufficient
- `--perceus` / `--aether` CLI flag — make always-on when ready

### Where Aether fits in the Flux compilation pipeline

Aether operates at **multiple levels** of the pipeline, with each phase targeting the
most effective insertion point:

```text
Source → Lexer → Parser → AST → HM Type Inference
                                       │
                    ┌──────────────────▼──────────────────┐
                    │         Core IR (core/)              │
                    │                                      │
                    │  Phase 5: CoreExpr::Dup / Drop       │ ← explicit RC operations
                    │  Phase 6: Borrowing elision           │ ← skip dup/drop pairs
                    │                                      │
                    │  Runs AFTER existing Core passes:     │
                    │  beta → cokc → case_of_case →        │
                    │  inline → dead_let → evidence → anf  │
                    │  THEN: aether_rc → aether_borrow     │
                    │                                      │
                    └──────────────────┬──────────────────┘
                                       │
                    ┌──────────────────▼──────────────────┐
                    │         CFG IR (cfg/)                 │
                    │                                      │
                    │  Dup/Drop lower to explicit           │
                    │  Rc::clone / Rc::drop calls           │
                    │  in IrInstr                           │
                    └──────────────────┬──────────────────┘
                                       │
              ┌────────────────────────┼────────────────────────┐
              │                        │                        │
     ┌────────▼────────┐     ┌────────▼────────┐     ┌────────▼────────┐
     │   VM Backend     │     │   JIT Backend    │     │  LLVM Backend   │
     │                  │     │                  │     │                 │
     │  Phase 0: extend │     │  Dup/Drop →      │     │  Dup/Drop →     │
     │  OpConsumeLocal  │     │  inline RC ops   │     │  LLVM intrinsics│
     │  + Rc::try_unwrap│     │  or rt_* calls   │     │  (ARC-style)    │
     │  in dispatch.rs  │     │                  │     │                 │
     └──────────────────┘     └──────────────────┘     └─────────────────┘
              │                        │                        │
              └────────────────────────┼────────────────────────┘
                                       │
                    ┌──────────────────▼──────────────────┐
                    │       Shared Runtime (runtime/)       │
                    │                                      │
                    │  Phase 0: base function Rc reuse     │ ← map, filter, push
                    │  Phase 1-3: GcHeap → Rc migration    │ ← cons, ADT, HAMT
                    │  Phase 4: delete GcHeap              │
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

**Phase 0-3 are different:** They operate at the runtime/VM level without Core IR changes.
This is intentional — they exploit existing infrastructure (OpConsumeLocal, Rc::try_unwrap,
base function signatures) and deliver value before the heavier Core IR work.

## Phased rollout
[phased-rollout]: #phased-rollout

### Phase 0: Quick wins (no Core IR changes, no new modules)

**Estimated effort: 1-2 weeks. No dependencies.**

VM-level optimizations using existing infrastructure:

1. **Extend `OpConsumeLocal` to full function scope**
   - Widen `collect_consumable_param_uses()` from per-match-arm to entire function body
   - Mark last-use of any local binding, emit `OpConsumeLocal` instead of `OpGetLocal`
   - Files: `src/bytecode/compiler/expression.rs`

2. **Extend `Rc::try_unwrap` to more VM opcodes**
   - `OpConsHead`: move head when cons cell is unique (currently clones)
   - `OpConsTail`: move tail when cons cell is unique (currently clones)
   - `OpTupleIndex`: move element when tuple is unique (currently clones)
   - ~5 lines per opcode in `src/bytecode/vm/dispatch.rs`

3. **Add closure-capture awareness to use-count analysis**
   - If variable appears in `collect_free_vars()` of any lambda → mark as Shared
   - Prevents consuming a variable captured by a closure
   - Files: `src/bytecode/compiler/expression.rs`

4. **Base function Rc reuse fast paths**
   - Add `Rc::strong_count == 1` → `Rc::try_unwrap` → in-place mutation to:
     `base_map`, `base_filter`, `base_push`, `base_sort`, `base_sort_by`
   - ~20 lines per function in `src/runtime/base/`
   - All 3 backends benefit (shared runtime code)

### Phase 1: Cons list migration

**Estimated effort: 2-3 weeks. Depends on Phase 0 measured.**

Migrate cons lists from `GcHeap` to `Rc`:

1. Add `ConsList` struct and `Value::ConsList(Rc<ConsList>)` variant
2. Implement iterative `Drop` for `ConsList` (stack-overflow prevention)
3. Redirect `OpCons`, `base_list`, `base_to_list` to use `ConsList`
4. Update `OpConsHead`, `OpConsTail`, `OpIsCons`, `OpIsEmptyList` for new variant
5. Handle both `Value::Gc(Cons)` and `Value::ConsList` during migration
6. After migration complete, remove `HeapObject::Cons` path

### Phase 2: ADT unification

**Estimated effort: 1-2 weeks. Independent of Phase 1.**

Eliminate `Value::GcAdt` — unify all ADTs under `Value::Adt`:

1. Change `OpMakeAdt` to allocate via `Rc` instead of `gc_heap.alloc(HeapObject::Adt)`
2. Update JIT/LLVM `rt_*` helpers (`rt_make_adt` etc.) to allocate via `Rc`
3. Remove `Value::GcAdt` variant
4. Remove `HeapObject::Adt`

### Phase 3: HAMT migration

**Estimated effort: 3-4 weeks. Depends on Phase 1 (cons migration proves the pattern).**

Migrate HAMT maps from `GcHeap` to `Rc`:

1. Rewrite `src/runtime/gc/hamt.rs` to use `Rc<HamtNode>` (30+ function signatures change)
2. Add `Value::HamtMap(Rc<HamtNode>)` variant
3. Implement iterative drop for `HamtNode` (same pattern as cons)
4. Update all HAMT base functions (`put`, `get`, `has_key`, `keys`, `values`, `merge`, `delete`)
5. Update hash_ops tests

### Phase 4: Delete GcHeap

**Estimated effort: 1 week. Depends on Phases 1+2+3.**

1. Remove `Value::Gc(GcHandle)` variant
2. Delete `src/runtime/gc/gc_heap.rs`, `heap_object.rs`
3. Remove `GcHeap` from VM struct
4. Remove `gc_heap` parameter from all base function signatures
5. Deprecate `--gc-threshold`, `--no-gc` flags (keep as no-ops with warning)
6. Keep `gc/` directory for `hamt.rs` and `cons.rs` as Rc-based implementations,
   or move them to `runtime/collections/`

### Phase 5: Core IR ownership annotations (Aether evidence)

**Estimated effort: 4-6 weeks. Depends on Phase 0-4 measured.**

Only pursue if Phase 0-4 measurements show Rc overhead is still a bottleneck:

1. Add `CoreExpr::Dup(binder)` and `CoreExpr::Drop(binder)` to Core IR
2. New Core pass: insert dup/drop based on variable usage analysis
3. Optimization pass: fuse dup/drop pairs, elide unnecessary operations
4. Update all 7 existing Core passes to handle Dup/Drop nodes
5. Expose ownership evidence to all 3 backends

### Phase 6: Borrowing analysis

**Estimated effort: 3-4 weeks. Depends on Phase 5.**

1. Skip dup/drop for values whose lifetime is contained within a function scope
   and that do not escape (not stored in closures, ADTs, or returned)
2. Integrate with effect handler boundaries — values captured by continuations
   must not be borrowed across the capture point
3. Inter-procedural borrowing for known call sites (deferred)

### Phase 7: Actor transfer (future)

Transfer semantics for uniquely owned actor messages. Only after copy/rebuild
semantics are fully correct and observable.

## Expected impact

| Phase | What improves | Estimated gain |
|-------|--------------|----------------|
| Phase 0 | Array `map`/`filter`/`push` on unique inputs | 2-4× for these operations |
| Phase 0 | Local variable clone/drop elimination | 10-30% fewer Rc operations |
| Phase 1 | Cons list operations (no GC pauses) | Eliminates GC cycles for list-heavy code |
| Phase 2 | ADT operations (unified path) | Simpler code, slight perf improvement |
| Phase 3 | HAMT operations (no GC pauses) | Eliminates GC cycles for map-heavy code |
| Phase 4 | Overall (no dual memory system) | Simpler runtime, predictable perf |
| Phase 5 | All Rc operations (dup/drop elision) | 10-20% fewer Rc operations |
| Phase 6 | Function-local values (zero Rc overhead) | Near-zero overhead for local computation |

## Drawbacks
[drawbacks]: #drawbacks

1. **Large scope** — 7 phases spanning months of work. Risk of stalling after early phases.
   Mitigated by each phase delivering independent value.
2. **Rc recursive drop** — cons lists and HAMT trees under Rc will stack-overflow without
   iterative drop. Must implement before migration.
3. **HAMT rewrite is high-risk** — 30+ call sites in hamt.rs, deeply interleaved with GcHeap.
   Mitigated by doing cons list migration first (proves the pattern).
4. **Core IR complexity** — Phase 5 adds Dup/Drop nodes to Core IR, complicating all 7
   existing Core passes. Only pursue if Phase 0-4 prove insufficient.
5. **Backend parity constraint** — limits how aggressively each backend can optimize
   independently. This is intentional (correctness > performance).

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not keep tracing GC as the main model?

- Flux already uses `Rc` for most values
- `GcHandle` cannot cross actor boundaries
- Two memory systems is unnecessary complexity
- Perceus-style reuse is a better fit for pure FP optimization

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

### Why Phase 0 before Phase 5?

Phase 0 (extend existing OpConsumeLocal + base function Rc reuse) gives 70-80% of
the benefit of Phase 5 (full Core IR dup/drop) at 10% of the effort. Measure first,
then invest in Core IR changes only if needed.

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
| **0045** (GC) | Superseded as long-term model; GC remains during migration |
| **0068** (uniqueness analysis) | Subsumed — Phase 0 extends existing infra, Phase 5 is full analysis |
| **0069** (Rc::get_mut fast paths) | Subsumed — Phase 0 base function reuse, deferred opcode-level reuse |
| **0070** (GcHandle elimination) | Subsumed — Phases 1-4 implement incrementally |
| **0109** (VM optimizations) | Complementary — dispatch optimizations independent of Aether |
| **0110** (JIT optimizations) | Complementary — JIT Rc elimination benefits from Aether evidence |
| **0111** (LLVM optimizations) | Complementary — LLVM Rc intrinsics benefit from Aether evidence |
| **0112** (shared pipeline) | Complementary — Core IR caching, iterative passes independent |

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should GC be fully eliminated or kept as a fallback for edge cases?
2. Should iterative drop for cons lists use a loop or trampoline pattern?
3. What is the right representation for empty HAMT maps after migration?
4. How should handler-aware uniqueness evidence be represented in Core IR?
5. Should Aether diagnostics surface in `--dump-core` or a separate `--dump-aether` flag?

## Future possibilities
[future-possibilities]: #future-possibilities

- Transfer semantics for uniquely owned cross-actor values
- Region-like short-lived allocation strategies layered under Aether
- Adaptive heuristics for reuse vs rebuild for small values
- Ownership-aware optimization reports in `flux analyze`
- Specialized Aether paths for persistent collection combinators
- `unique` keyword in function signatures for guaranteed ownership transfer
