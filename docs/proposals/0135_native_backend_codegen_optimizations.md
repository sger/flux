- Feature Name: Native Backend Codegen Optimizations
- Start Date: 2026-03-30
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0132 (LIR), Proposal 0133 (Unified CorePrimOp)
- Related: Proposal 0124 (Pointer Tagging — Phase 9 of this proposal)

## Summary

Optimize the LIR → LLVM IR emission pipeline to eliminate unnecessary C runtime calls in the native backend. The current codegen emits C function calls for operations that can be expressed as inline LLVM IR instructions, causing 50-100x overhead on tight loops compared to GHC.

## Motivation

### Current state: binarytrees benchmark

| Backend | Execute (smoke n=8) |
|---------|-------------------|
| Flux VM | 34ms |
| Flux LLVM native | 174ms |
| Haskell (GHC -O2) | 3ms |

The native backend should be **faster** than the VM, not 5x slower. Analysis of the generated LLVM IR for `check_tree` (the hot function) reveals the root cause: every primitive operation goes through a C function call instead of being emitted as inline LLVM instructions.

### What GHC emits for `check_tree`

```
case tree of
  Node l r -> tailCheck l (tailCheck r (a + 1))
  Nil      -> a
```

Compiles to: 1 tag check (pointer bit), 2 field loads (GEP + load), 1 `add` instruction. ~5 machine instructions total.

### What Flux LLVM emits for `check_tree`

```llvm
%t2 = call fastcc i32 @flux_adt_tag(i64 %v0)              ; C call for tag
%ext = call fastcc ptr @flux_adt_field_ptr(i64 %v0, i32 0) ; C call for field 0
%v2 = load i64, ptr %ext, align 8
%ext = call fastcc ptr @flux_adt_field_ptr(i64 %v0, i32 1) ; C call for field 1
%v3 = load i64, ptr %ext, align 8
%v4 = call fastcc i64 @flux_check_tree(i64 %v2)            ; recursive call (ok)
%v6 = call fastcc i64 @flux_check_tree(i64 %v3)            ; recursive call (ok)
%v8 = call ccc i64 @flux_rt_add(i64 %v4, i64 %v6)         ; C call for add
%v9 = call fastcc i64 @flux_tag_int(i64 1)                 ; C call to box literal 1
%v10 = call ccc i64 @flux_rt_add(i64 %v8, i64 %v9)        ; C call for add
```

**8 C function calls** where GHC uses **~5 machine instructions**. Each C call has function call overhead (save/restore registers, stack frame, indirect branch).

### Root causes identified

| Issue | Current | Optimal | Calls saved per occurrence |
|-------|---------|---------|---------------------------|
| Integer literals | `call @flux_tag_int(i64 1)` | `or i64 1, NANBOX_SENTINEL` | 1 C call |
| Generic `Add` | `call @flux_rt_add(i64, i64)` | `untag + add + retag` (3 inline ops) | 1 C call |
| Generic `Gt`/`Lt` | `call @flux_rt_gt(i64, i64)` | `untag + icmp + tag_bool` (3 inline ops) | 1 C call |
| ADT tag extraction | `call @flux_adt_tag(i64)` | `untag_ptr; load i32` (2 inline ops) | 1 C call |
| ADT field access | `call @flux_adt_field_ptr(i64, i32)` | `untag_ptr; GEP; load` (3 inline ops) | 1 C call |
| Bool comparison | `call @flux_rt_eq(result, true)` | `trunc i64 to i1` | 1 C call |
| `TagInt` for computed results | `call @flux_tag_int(i64)` | `and + or` (2 inline ops) | 1 C call |

---

## Design

### Phase 1: Inline integer literal tagging

**Files:** `src/lir/lower.rs`, `src/lir/emit_llvm.rs`

**Change:** When lowering `CoreLit::Int(n)`, check if `n` fits in the 46-bit NaN-box payload (±35 trillion). If so, emit `LirConst::Tagged(nanbox_tag_int(n))` instead of `LirConst::Int(n)`.

**Before:**
```llvm
%v9 = call fastcc i64 @flux_tag_int(i64 1)
```

**After:**
```llvm
%v9 = add i64 9222246136947933185, 0  ; pre-computed NaN-boxed 1
```

**Impact:** Eliminates 1 C call per integer literal. In `check_tree`, saves 1 call per invocation (the literal `1` in `+ 1`). In `make_tree`, saves 1 call (the literal `0` in `depth > 0`).

**Estimated improvement:** ~10% on integer-heavy benchmarks.

### Phase 2: Typed comparisons in AST→Core lowering

**Files:** `src/core/lower_ast/expression.rs`

**Change:** The AST→Core lowering already emits typed `IAdd`/`ISub` when both operands are provably `Int`. Apply the same logic to comparisons: emit `ICmpGt`/`ICmpLt`/`ICmpLe`/`ICmpGe`/`ICmpEq`/`ICmpNe` instead of generic `Gt`/`Lt`/etc. when types are known.

Currently (line ~395):
```rust
">" => CorePrimOp::Gt,  // Always generic
```

After:
```rust
">" if is_int => CorePrimOp::ICmpGt,  // Typed when provable
">" => CorePrimOp::Gt,                // Generic fallback
```

**Before (binarytrees `depth > 0`):**
```llvm
%v1 = call fastcc i64 @flux_tag_int(i64 0)       ; box 0
%v2 = call ccc i64 @flux_rt_gt(i64 %v0, i64 %v1) ; generic compare
%v4 = add i64 9222316505692110849, 0               ; NaN-boxed true
%v5 = call ccc i64 @flux_rt_eq(i64 %v2, i64 %v4) ; compare with true
%v6 = and i64 %v5, 1
%t0 = trunc i64 %v6 to i1
```

**After:**
```llvm
%r0 = call fastcc i64 @flux_untag_int(i64 %v0)
%t0 = icmp sgt i64 %r0, 0
```

**Impact:** Eliminates 4 operations (2 C calls + 2 inline ops) per integer comparison, replacing with 1 untag + 1 `icmp`.

**Estimated improvement:** ~15% on branch-heavy benchmarks.

### Phase 3: Inline TagInt/UntagInt in LLVM emission

**Files:** `src/lir/emit_llvm.rs`

**Change:** Instead of calling `flux_tag_int` / `flux_untag_int` (fastcc C functions), emit inline LLVM IR for NaN-box tag/untag of integers.

**TagInt (for small integers):**
```llvm
; Current: call fastcc i64 @flux_tag_int(i64 %raw)
; Proposed:
%payload = and i64 %raw, 0x3FFFFFFFFFFF       ; mask to 46 bits
%tagged = or i64 %payload, 0x7FFC000000000000  ; apply NaN-box sentinel
```

**UntagInt (sign-extend from 46 bits):**
```llvm
; Current: call fastcc i64 @flux_untag_int(i64 %val)
; Proposed:
%shifted = shl i64 %val, 18
%raw = ashr i64 %shifted, 18  ; sign-extend from bit 45
```

**Note:** BigInt overflow (values outside ±2^45) still needs the C call. Emit an overflow check branch for computed results; skip it for known-small constants.

**Impact:** Eliminates C calls for all tag/untag operations on small integers. In `check_tree`, saves 2+ calls per invocation (the retag after `IAdd`, the untag before `ICmp`).

**Estimated improvement:** ~20% on arithmetic-heavy benchmarks.

### Phase 4: Inline ADT tag extraction and field access

**Files:** `src/lir/emit_llvm.rs`

**Change:** Replace `call @flux_adt_tag(i64 %v)` and `call @flux_adt_field_ptr(i64 %v, i32 %idx)` with inline LLVM IR that does pointer arithmetic directly.

**ADT memory layout** (from `runtime/c/flux_rt.h`):
```
FluxHeader (8 bytes): { i32 refcount, u8 scan_fsize, u8 obj_tag, u16 reserved }
Payload: { i32 ctor_tag, i32 field_count, i64 fields[] }
```

**Inline tag extraction:**
```llvm
; Current: %tag = call fastcc i32 @flux_adt_tag(i64 %v)
; Proposed:
%payload = and i64 %v, 0x3FFFFFFFFFFF
%ptr = shl i64 %payload, 3                    ; untag pointer
%raw_ptr = inttoptr i64 %ptr to ptr
%tag = load i32, ptr %raw_ptr, align 4        ; ctor_tag at offset 0
```

**Inline field access:**
```llvm
; Current: %fptr = call fastcc ptr @flux_adt_field_ptr(i64 %v, i32 0)
;          %field = load i64, ptr %fptr, align 8
; Proposed:
%payload = and i64 %v, 0x3FFFFFFFFFFF
%ptr = shl i64 %payload, 3
%raw_ptr = inttoptr i64 %ptr to ptr
%field_ptr = getelementptr i8, ptr %raw_ptr, i64 8  ; skip ctor_tag(4) + field_count(4)
%field = load i64, ptr %field_ptr, align 8           ; field 0
; For field N: offset = 8 + N*8
```

**Impact:** Eliminates 1 C call per tag check and 1 C call per field access. In `check_tree`, saves 3 C calls per invocation (1 tag + 2 fields).

**Estimated improvement:** ~25% on ADT-heavy benchmarks (binarytrees, rbtree, cfold).

### Phase 5: Inline generic arithmetic for known-integer contexts

**Files:** `src/lir/lower.rs`

**Change:** When the LIR lowerer encounters `CorePrimOp::Add` on values that came from `CorePrimOp::IAdd`/`ICmpGt`/integer literals/other integer-producing operations, emit inline `IAdd` instead of `PrimCall(Add)`.

This is a **local type propagation** within the LIR lowering: track which `LirVar`s are known to hold integers and use that to specialize generic operations.

**Implementation:**
```rust
struct FnLower {
    // ... existing fields
    int_vars: HashSet<LirVar>,  // Variables known to hold NaN-boxed integers
}
```

When a variable is produced by `LirConst::Int`, `TagInt`, `IAdd`, `ICmpGt`, etc., mark it as `int_vars`. When lowering `CorePrimOp::Add` where both operands are in `int_vars`, emit `IAdd` instead of `PrimCall(Add)`.

**Impact:** Fixes the `check_tree` case where `Add(check_tree(left), check_tree(right))` stays generic because the return type of `check_tree` isn't tracked. If `check_tree`'s return is annotated as `Int` in type inference, the Core lowering should already emit `IAdd`. This phase catches the cases where type information is lost.

**Estimated improvement:** ~15% on benchmarks where recursive function return types are unresolved.

### Phase 6: Boolean result optimization

**Files:** `src/lir/emit_llvm.rs`

**Change:** When a comparison result (`flux_rt_gt` etc.) is immediately used in a branch, skip the NaN-boxed boolean and use the raw `i1` result directly.

**Before:**
```llvm
%v2 = call ccc i64 @flux_rt_gt(i64 %v0, i64 %v1)  ; returns NaN-boxed Bool
%v4 = add i64 9222316505692110849, 0                 ; load true constant
%v5 = call ccc i64 @flux_rt_eq(i64 %v2, i64 %v4)   ; compare with true
%v6 = and i64 %v5, 1
%t0 = trunc i64 %v6 to i1
br i1 %t0, ...
```

**After (with Phase 2 typed comparisons):**
```llvm
%r0 = call fastcc i64 @flux_untag_int(i64 %v0)
%t0 = icmp sgt i64 %r0, 0
br i1 %t0, ...
```

**After (with Phase 3 inline untag):**
```llvm
%shifted = shl i64 %v0, 18
%r0 = ashr i64 %shifted, 18
%t0 = icmp sgt i64 %r0, 0
br i1 %t0, ...
```

**Impact:** Eliminates the entire compare-with-true pattern (2 C calls + 3 inline ops → 0).

**Estimated improvement:** ~5% (already mostly fixed by Phase 2).

---

## Projected impact

### Per-phase cumulative improvement on binarytrees (smoke n=8)

| Phase | Description | Est. time | Cumulative |
|-------|-------------|-----------|------------|
| Current | Baseline | 174ms | 174ms |
| Phase 1 | Inline integer literals | ~157ms | 157ms |
| Phase 2 | Typed comparisons | ~130ms | 130ms |
| Phase 3 | Inline TagInt/UntagInt | ~100ms | 100ms |
| Phase 4 | Inline ADT tag/field | ~70ms | 70ms |
| Phase 5 | Local type propagation | ~55ms | 55ms |
| Phase 6 | Boolean result optimization | ~50ms | 50ms |

**Target:** ~50ms (from 174ms), a **3.5x improvement**. Still ~17x slower than GHC's 3ms due to remaining overhead (closure dispatch, RC allocation, NaN-box representation), but competitive with the VM's 34ms.

### Remaining gap to GHC (for future proposals)

| Overhead | GHC approach | Flux approach | Fix |
|----------|-------------|---------------|-----|
| Allocation | Bump allocator (~2 instructions) | `flux_gc_alloc_header` (C call + malloc) | Arena/bump allocator |
| Closure dispatch | Direct calls (known functions) | `flux_call_closure` (indirect) | Whole-program devirtualization |
| NaN-boxing | Unboxed/strict by default | Every value NaN-boxed | Unboxing pass (like GHC's worker/wrapper) |
| RC overhead | Generational GC (batch) | Per-value dup/drop | Elide RC on stack-local values |

These are architectural changes beyond codegen optimization and would be separate proposals.

---

## Migration phases

Each phase is independently shippable and testable:

1. **Phase 1** — Change `lower_lit` to emit `LirConst::Tagged` for small integers. ~20 lines changed.
2. **Phase 2** — Add `is_int`/`is_float` checks to comparison lowering in `expression.rs`. ~30 lines changed.
3. **Phase 3** — Add inline LLVM IR emission for `TagInt`/`UntagInt` in `emit_llvm.rs`. ~40 lines changed. Requires overflow check for computed values.
4. **Phase 4** — Replace `flux_adt_tag`/`flux_adt_field_ptr` calls with inline pointer arithmetic. ~60 lines changed.
5. **Phase 5** — Add `int_vars` tracking to `FnLower` for local type propagation. ~50 lines changed.
6. **Phase 6** — Peephole optimization for boolean-in-branch pattern. ~30 lines changed.

---

## Verification

Each phase should be verified with:

```bash
# Correctness
cargo test --all --all-features

# Performance (compare before/after)
cargo run --release --features core_to_llvm -- benchmarks/flux/binarytrees_smoke.flx --native --stats --no-cache

# LLVM IR inspection (verify inline ops replace C calls)
cargo run --features core_to_llvm -- benchmarks/flux/binarytrees_smoke.flx --emit-llvm | grep -c "call.*flux_rt_add\|call.*flux_tag_int\|call.*flux_adt_tag"

# Cross-language comparison
time ./target/binarytrees_native  # Flux native
time ./target/release/binarytrees_hs 8  # Haskell
```

## Drawbacks

- **Inline NaN-boxing is fragile**: If the NaN-box layout constants get out of sync between LLVM IR and C runtime, values will be silently corrupted. Must share constants via a single source of truth.
- **BigInt overflow**: Inline `TagInt` only works for small integers. Need a branch for overflow to the C `flux_tag_int` path. Adds code size.
- **Maintenance**: More LLVM IR emission code in `emit_llvm.rs`. Each inline pattern is ~10-20 lines replacing a 1-line `call_c`.

---

## Architectural optimizations (future phases)

These address the remaining 50ms → 3ms gap to GHC. Each is a significant project and would warrant its own proposal. Documented here as the roadmap based on GHC source analysis (`/Users/s.gerokostas/Downloads/Github/ghc`).

### Phase 7: Bump allocator (GHC's nursery model)

**Current Flux:** Every allocation calls `flux_gc_alloc_header()` → `malloc()`. Each allocation is a C function call + system allocator overhead.

**GHC approach:** A dedicated CPU register (`r12` on x86_64, `r21` on ARM64) holds the heap pointer `Hp`. Allocation is:
```asm
; Allocate 3 words (header + 2 fields) for Node constructor
add r12, r12, #24        ; bump Hp
cmp r12, [HpLim]         ; check against nursery limit
ja  gc_entry              ; branch to GC if overflow (rare)
str info_ptr, [r12-24]   ; store info table
str left,     [r12-16]   ; store field 0
str right,    [r12-8]    ; store field 1
```
**2-3 instructions** for the fast path (bump + compare). No function call. No lock. Each thread has its own nursery (256KB-4MB). GC is triggered only when the nursery fills up.

**Flux implementation path:**
- Add a `flux_bump_alloc(size)` fast path in C that uses a pre-allocated arena
- In LLVM emission, emit inline bump allocation: `load @hp; add @hp, size; cmp @hp, @hp_lim; br overflow, alloc_ok`
- Fall back to `flux_gc_alloc_header` on overflow (which triggers collection)
- Integrate with Aether RC: drop-triggered frees return to a free-list, not the arena

**Estimated impact:** ~3-5x improvement on allocation-heavy benchmarks (binarytrees, rbtree).

### Phase 8: Known-call optimization (GHC's DirectEntry)

**Current Flux:** Most function calls go through `flux_call_closure(closure, args, nargs)` — an indirect call that: untags the closure pointer, loads the function pointer from the closure struct, checks arity, branches on saturated/under-saturated/over-saturated, then calls.

**GHC approach:** GHC classifies every call site at compile time:
- **DirectEntry** (known function, known arity): emit `call @function_name(args...)` — direct jump, no closure dereference.
- **SlowCall** (unknown function): dispatch through `stg_ap_*` apply routines that handle arity mismatch.

GHC tracks `LambdaFormInfo` for every binding — whether it's a known re-entrant function, a thunk, a constructor, or unknown. Imported functions carry this info in `.hi` interface files.

**Flux current state:** LIR already has `CallKind::Direct` vs `CallKind::Indirect`. The LLVM emitter already emits direct `call @flux_<name>(args...)` for `CallKind::Direct`. But the LIR lowering doesn't classify enough calls as Direct — in particular, recursive calls within the same module and calls to imported Flow.* library functions often fall through to Indirect.

**Flux implementation path:**
- Extend the LIR lowering to track all top-level function `LirFuncId`s (already partially done via `binder_func_id_map`)
- For calls where the target is a known `LirFuncId`, emit `CallKind::Direct` even when the call goes through a local variable (lambda lift detection)
- For self-recursive calls, always emit Direct
- For imported module functions, propagate arity information through the module interface

**Estimated impact:** ~2x improvement on call-heavy benchmarks. Eliminates closure dispatch overhead for ~80% of calls.

### Phase 9: Replace NaN-boxing with pointer tagging (Proposal 0124)

**See [Proposal 0124 — Pointer Tagging](0124_pointer_tagging.md) for full design.**

Replace NaN-boxing with 1-bit pointer tagging (like OCaml, Lean 4, Koka):
- `LSB = 1` → 63-bit signed integer (`val >> 1` to untag — 1 instruction)
- `LSB = 0` → heap pointer (zero-cost, natural alignment)
- Floats → heap-boxed (rare in practice, mitigated by unboxing pass)

This is the single highest-impact architectural change. Current NaN-boxing requires:
- 3 instructions to tag an integer (mask + sentinel OR)
- 3 instructions to untag (mask + sign-extend shift pair)
- C function call for overflow detection (`flux_tag_int`)

Pointer tagging requires:
- 1 instruction to tag (`shl 1; or 1`)
- 1 instruction to untag (`ashr 1`)
- Tagged arithmetic possible without untagging: `tagged_add(a, b) = a + b - 1`

Both GHC (3-bit pointer tags), Koka (1-bit, `kklib/include/kklib.h:900-1005`), and OCaml (1-bit) use pointer tagging. This also fixes the 46-bit integer overflow parity gap between VM (64-bit) and native (46-bit NaN-box).

**Estimated impact:** ~1.5x improvement on pattern-match-heavy benchmarks, eliminates all `flux_tag_int`/`flux_untag_int` calls.

### Phase 10: Worker/Wrapper unboxing (GHC's demand analysis)

**Current Flux:** Every value is NaN-boxed. A function `fn add(a: Int, b: Int) -> Int` receives NaN-boxed values, untags them, computes, and retags. For tight loops, the untag/retag overhead per iteration is significant.

**GHC approach:** Strictness/demand analysis identifies functions that always evaluate their arguments. The Worker/Wrapper transformation splits:
```haskell
-- Wrapper (inlined at call sites):
add :: Int -> Int -> Int
add (I# a) (I# b) = I# ($wadd a b)

-- Worker (operates on unboxed Int#):
$wadd :: Int# -> Int# -> Int#
$wadd a b = a +# b
```

The worker passes raw machine integers in registers. No boxing/unboxing in the hot path. The wrapper is inlined by the simplifier, so callers also avoid boxing.

**Flux implementation path:**
- Add a demand analysis pass on Core IR that identifies strict arguments
- Generate worker functions that take unboxed `i64` parameters directly (raw, not NaN-boxed)
- Generate wrapper functions that untag arguments and call the worker
- Inline wrappers at call sites during a Core-level simplification pass
- In LIR, worker functions use raw `i64` types instead of NaN-boxed values

**Estimated impact:** ~3-5x improvement on arithmetic-heavy tight loops. This is the single biggest win for closing the gap to GHC on numeric code.

### Summary: projected cumulative improvement

| Phase | Description | Est. execution time | vs GHC |
|-------|-------------|-------------------|--------|
| Current | Baseline | 174ms | 58x |
| 1-6 | Codegen inlining (this proposal) | ~50ms | 17x |
| 7 | Bump allocator | ~15ms | 5x |
| 8 | Known-call optimization | ~8ms | 2.7x |
| 9 | Pointer tagging | ~6ms | 2x |
| 10 | Worker/Wrapper unboxing | ~4ms | 1.3x |

The remaining 1.3x gap to GHC would be due to:
- GHC's STG machine vs Flux's closure representation differences
- GHC's decades of optimization passes (simplifier, specialization, inlining heuristics)
- GHC's hand-tuned RTS (C-- backend, platform-specific register allocation)

---

## Prior art

### GHC (Haskell)

Source: `/Users/s.gerokostas/Downloads/Github/ghc`

- **Allocation** (`rts/sm/Storage.c`): Bump-pointer nursery allocator. Heap pointer `Hp` pinned to CPU register (`r12` on x86_64, `r21` on ARM64). Allocation is 2-3 instructions: bump Hp, compare with HpLim, conditional branch to GC. Per-capability nurseries (256KB-4MB) eliminate thread contention.
- **Function dispatch** (`compiler/GHC/StgToCmm/Closure.hs`): `getCallMethod` classifies every call site. Known functions with matching arity → `DirectEntry` (direct jump to code label). Unknown functions → `SlowCall` through `stg_ap_*` apply routines. `LambdaFormInfo` tracks arity/type for every binding, serialized in `.hi` interface files for cross-module optimization.
- **Pointer tagging** (`rts/include/rts/storage/ClosureMacros.h`): Low 3 bits of heap pointers encode constructor tag (values 0-7). `GET_TAG(p) = p & 0x7` — 1 AND instruction, no dereference. Constructors tagged at allocation time. Pattern matching checks tag bits before loading fields.
- **Unboxing** (`compiler/GHC/Core/Opt/WorkWrap/`): Demand analysis (`DmdAnal.hs`) identifies strict arguments. Worker/Wrapper splits functions: wrapper unboxes `Int → Int#`, worker operates on raw machine words in registers. Wrappers are inlined at call sites by the simplifier.
- **Value representation**: No NaN-boxing. `Int` is a heap-allocated closure `I# Int#`. `Int#` is an unlifted raw machine word. Strictness analysis + worker/wrapper eliminate most boxing in practice.

### Koka

Source: `/Users/s.gerokostas/Downloads/Github/koka`

- **Allocation** (`kklib/include/kklib.h:500-642`): Uses **mimalloc** (Microsoft's allocator) with per-thread heaps. `kk_malloc_small()` → `mi_heap_malloc_small()`. Not a bump allocator, but mimalloc's thread-local free-lists are very fast (~10 instructions for small allocs). Perceus reuse (`kk_block_drop_reuse()`) returns unique blocks directly for re-allocation, often avoiding malloc entirely.
- **Function dispatch** (`src/Backend/C/FromCore.hs:1894-2032`): Two paths:
  - Known direct calls (line 2019): `ppName(getName(tname)) <.> arguments` — generates direct C function call.
  - Unknown closure calls (line 2021): `kk_function_call(restp, argtps, f, args, ctx)` macro — extracts function pointer from closure struct (`->fun` field), casts, and calls. One pointer dereference + indirect call.
  - Tail calls (line 2767): `goto kk__tailcall;` — goto-based trampoline for tail recursion.
- **Value representation** (`kklib/include/kklib.h:900-1005`, `kklib/include/kklib/box.h`): **1-bit pointer tagging** (like OCaml, not NaN-boxing):
  - LSB = 0 → heap pointer (zero-cost boxing for pointers)
  - LSB = 1 → small integer encoded as `2*n + 1` (zero-cost boxing for smis)
  - Small integer arithmetic: SOFA encoding (`4*n + 1`) in `integer.h` for fast overflow detection
  - Doubles: Strategy A1 — box if 11-bit exponent fits in 10 bits (~99% of common doubles without allocation)
  - ADT singletons (zero-field constructors): encoded as tagged values (no heap allocation)
- **Perceus RC** (`kklib/include/kklib.h:100-737`): `int32_t` refcount starting at 0 (unique). `kk_block_dup()` increments, `kk_block_drop()` decrements + frees. `kk_block_drop_reuse()` returns memory for constructor reuse when refcount hits 0.
- **FBIP** (`test/bench/koka/binarytrees-fbip.kk`): First-class Builder Insertion Passing. Tree construction uses tail-recursive builder pattern + Perceus reuse to eliminate allocation overhead. Builder constructors (`BuildRight`, `BuildNode`) are immediately consumed and their memory reused.

### Koka vs Flux comparison

| Feature | Koka | Flux (current) | Gap |
|---------|------|----------------|-----|
| Allocation | mimalloc + Perceus reuse | `flux_gc_alloc_header` → malloc | Koka ~3x faster |
| Integer boxing | 1-bit tag, `2*n+1` | NaN-box, `flux_tag_int()` C call | Koka zero-cost |
| Known calls | Direct C function call | `flux_call_closure` indirect | Koka ~2x faster |
| ADT tag check | Load tag from block header | `flux_adt_tag()` C call | Koka ~3x faster |
| Tail calls | `goto` trampoline | LIR TailCall → LLVM `musttail` | Comparable |
| RC system | Perceus (compile-time reuse) | Aether (compile-time dup/drop) | Similar design |
| Constructor reuse | `kk_block_drop_reuse()` | `DropReuse` LIR instruction | Similar design |

**Key Koka advantage:** 1-bit pointer tagging means boxing/unboxing is nearly free (1 shift instruction vs Flux's multi-instruction NaN-box encode/decode). This is the single biggest difference.

**Key shared advantage:** Both Flux (Aether) and Koka (Perceus) use compile-time reference counting with constructor reuse. Flux's `DropReuse` LIR instruction is the direct equivalent of Koka's `kk_block_drop_reuse()`.

### OCaml

- Tags are 1-bit pointer tags (lowest bit: 0=pointer, 1=immediate integer, like Koka). Field access is direct GEP without function calls. Uses generational GC with a minor heap (bump allocator).
