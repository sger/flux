- Feature Name: Pointer Tagging — Replace NaN-Boxing with Tagged Integers
- Start Date: 2026-03-25
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0119 (Typed LLVM Codegen)

## Summary

Replace the NaN-boxing value representation in the native (`core_to_llvm`) backend with pointer tagging. Integers become 63-bit (1 tag bit), pointers are naturally aligned (tag bit = 0), and floats are heap-boxed. This eliminates the 46-bit integer overflow parity gap between the VM and native backends, simplifies tag/untag to single-instruction shifts, and aligns Flux with the representation used by OCaml, Lean 4, Erlang/BEAM, and most ML-family compilers.

## Motivation

### The 46-bit problem

The current native backend uses NaN-boxing: all values are encoded as 64-bit IEEE 754 doubles, with NaN payloads carrying integers, booleans, and pointers in a 46-bit field.

```
NaN-boxed integer layout:
  [18-bit NaN sentinel + type tag] [46-bit payload]
  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  Total: 64 bits, but only 46 bits for the integer value
```

This limits integers to ±2^45 ≈ ±35 trillion. The VM backend uses full 64-bit `i64` values. Programs that exceed 46 bits produce different results:

```flux
fn sum_to(n, acc) {
    if n <= 0 { acc } else { sum_to(n - 1, acc + n) }
}
fn main() with IO { print(sum_to(100000000, 0)) }
```

| Backend | Result | Correct? |
|---------|--------|----------|
| VM | 5000000050000000 | Yes |
| Native | 3819213385856 | No — 46-bit overflow |

This is not a bug in any specific operation — it's structural. Every integer that passes through a NaN-box boundary loses its upper 18 bits.

### The NaN-boxing tax on integers

Even when integers fit in 46 bits, NaN-boxing imposes overhead:

```llvm
; Tag an integer (3 instructions):
%masked = and i64 %raw, 0x3FFFFFFFFFFF      ; mask to 46 bits
%tagged = or  i64 %masked, 0xFFF8000000000000 ; add NaN sentinel

; Untag an integer (3 instructions):
%payload = and i64 %val, 0x3FFFFFFFFFFF      ; extract 46-bit payload
%shifted = shl i64 %payload, 18              ; sign-extend step 1
%raw     = ashr i64 %shifted, 18             ; sign-extend step 2
```

6 instructions per tag+untag cycle. With pointer tagging:

```llvm
; Tag an integer (1 instruction):
%tagged = or i64 %raw_shifted, 1    ; set tag bit (after shl 1)

; Untag an integer (1 instruction):
%raw = ashr i64 %tagged, 1          ; arithmetic shift right
```

2 instructions total. 3x fewer instructions per tag/untag boundary.

### What OCaml does

OCaml has used pointer tagging since 1996. The encoding:

```
Integer:  [63-bit signed value] [1]    ← lowest bit = 1
Pointer:  [64-bit aligned address] [0] ← lowest bit = 0 (natural alignment)
```

- **Integer check**: `val & 1` — one AND instruction
- **Untag integer**: `val >> 1` — one arithmetic shift
- **Tag integer**: `(val << 1) | 1` — one shift + one OR
- **Pointer access**: `val` directly (bit 0 is already 0 for aligned pointers)

OCaml integers are 63-bit signed: range ±2^62 ≈ ±4.6 quintillion. No practical program overflows this.

**Floats in OCaml**: Always heap-boxed (a pointer to a 64-bit double on the heap). This is acceptable because:
1. Float-heavy code is uncommon in most programs
2. LLVM can optimize float-heavy loops with FluxRep::FloatRep unboxing (Proposal 0119)
3. The GC allocator is fast (~4ns per allocation)

### Lean 4's approach

Lean 4 uses an almost identical scheme:

```
Scalar (tagged):    [63-bit value] [1]
Object (pointer):   [aligned pointer] [0]
```

Lean calls unboxed scalars "unboxed" and heap objects "boxed." The compiler's type information determines which representation to use — exactly like Flux's FluxRep.

---

## Reference-level explanation

### New value encoding

```
Bit layout (64 bits):

Integer:   [63-bit signed integer value] [1]
           bit 63 ..................... bit 1  bit 0 = 1 (tag)

Pointer:   [64-bit aligned heap address] [0]
           bit 63 ..................... bit 1  bit 0 = 0 (natural alignment)

Boolean:   encoded as integer: true = 3 (0b11), false = 1 (0b01)

Unit/None: encoded as integer: 1 (0b01) — same as false, distinguished by type

Float:     heap-boxed: pointer to { header, f64 value }
```

### Tag check

```c
static inline bool is_int(int64_t val) { return val & 1; }
static inline bool is_ptr(int64_t val) { return !(val & 1); }
```

### Integer operations

```c
static inline int64_t tag_int(int64_t raw) { return (raw << 1) | 1; }
static inline int64_t untag_int(int64_t val) { return val >> 1; }  // arithmetic shift

// Direct tagged arithmetic (no untag needed!):
// tag_int(a) + tag_int(b) - 1 == tag_int(a + b)
// This means: tagged_add(a, b) = a + b - 1
static inline int64_t tagged_add(int64_t a, int64_t b) { return a + b - 1; }
static inline int64_t tagged_sub(int64_t a, int64_t b) { return a - b + 1; }
```

The remarkable property: addition and subtraction can be done **directly on tagged values** without untagging, by adjusting for the tag bit. This eliminates even the shift instructions for simple arithmetic.

### Float boxing

```c
typedef struct { uint32_t header; double value; } FluxFloat;

static inline int64_t box_float(double f) {
    FluxFloat *obj = flux_gc_alloc(sizeof(FluxFloat));
    obj->header = FLUX_OBJ_FLOAT;
    obj->value = f;
    return (int64_t)obj;  // pointer, bit 0 = 0
}

static inline double unbox_float(int64_t val) {
    FluxFloat *obj = (FluxFloat *)val;
    return obj->value;
}
```

### GC integration

The GC already traces pointer fields. With pointer tagging:
- **Integer values** (bit 0 = 1): skip during GC scan — not a pointer
- **Pointer values** (bit 0 = 0): trace as heap pointer

This is simpler than NaN-boxing's GC, which must check the NaN sentinel and type tag to determine if a value is a pointer.

### ADT field storage

ADT constructors store fields as tagged values:

```
Some(42):
  heap: [header: FLUX_OBJ_ADT] [tag: "Some"] [nfields: 1] [field0: 85]
                                                                    ^^
                                                        tag_int(42) = (42 << 1) | 1 = 85
```

Pattern match extraction: `untag_int(field0)` = `85 >> 1` = `42`.

---

## Implementation phases

### Phase 1 — C runtime conversion (~1 week)

Modify `runtime/c/` to use pointer tagging instead of NaN-boxing:

**Files:**
- `runtime/c/flux_rt.h` — new tag/untag macros, remove NaN-box constants
- `runtime/c/flux_rt.c` — update all value creation/inspection
- `runtime/c/gc.c` — update GC scan to use `val & 1` check
- `runtime/c/string.c` — strings are pointers (bit 0 = 0), no change needed
- `runtime/c/hamt.c` — HAMT nodes are pointers, no change needed
- `runtime/c/array.c` — array elements store tagged values

**Key changes:**
- `flux_make_int(n)` → `(n << 1) | 1`
- `flux_as_int(val)` → `val >> 1`
- `flux_is_int(val)` → `val & 1`
- `flux_is_ptr(val)` → `!(val & 1)`
- `flux_make_bool(b)` → `(b << 1) | 1` (true=3, false=1)
- `flux_make_float(f)` → heap-allocate, return pointer
- All `NANBOX_SENTINEL` references → removed

### Phase 2 — LLVM codegen conversion (~1 week)

Modify `src/core_to_llvm/` to emit pointer-tagged values:

**Files:**
- `src/core_to_llvm/codegen/prelude.rs` — replace `FluxNanboxLayout` with `FluxPtrTagLayout`
- `src/core_to_llvm/codegen/expr.rs` — update literal encoding, tag/untag helpers
- `src/core_to_llvm/codegen/arith.rs` — update arithmetic helpers

**Key changes:**
- `tagged_int_bits(n)` → `(n << 1) | 1`
- `emit_untag_int` → single `ashr i64 %val, 1`
- `emit_tag_int` → `or i64 (shl i64 %raw, 1), 1`
- `tagged_bool_bits(true)` → `3`, `tagged_bool_bits(false)` → `1`
- Float literals → heap-allocate via `flux_box_float` call

### Phase 3 — VM nanbox.rs alignment (~3 days)

The VM's `nanbox.rs` uses a Rust-level NaN-boxing scheme. Two options:
1. Convert VM to pointer tagging too (full parity)
2. Keep VM as-is (64-bit integers) and only convert native backend

Option 2 is simpler and maintains backward compatibility. The VM already uses full 64-bit integers — pointer tagging would reduce VM integers to 63 bits unnecessarily.

Recommended: **Option 2** — only convert the native backend. Document that VM uses 64-bit integers and native uses 63-bit. The 1-bit difference (2^62 vs 2^63) is irrelevant in practice.

### Phase 4 — Worker/wrapper integration (~2 days)

Update the Proposal 0119 worker/wrapper to use the new tag format:
- Worker untag: `ashr i64 %arg, 1` (one instruction)
- Worker retag: `or i64 (shl i64 %result, 1), 1` (two instructions)
- Or: use tagged arithmetic directly in the wrapper

### Phase 5 — Parity testing (~2 days)

- `scripts/check_core_to_llvm_parity.sh` — all examples must match
- Add integer overflow test cases near the 63-bit boundary
- Benchmark: measure speedup from simpler tag/untag

---

## Drawbacks

- **Float performance regression**: Floats become heap-allocated instead of inline NaN-boxed. Float-heavy code (rare in Flux) will be slower. Mitigated by FluxRep::FloatRep unboxing for typed float functions.

- **Memory overhead for floats**: Each float uses 16 bytes (header + value) instead of 8 bytes. Mitigated by: floats are uncommon as stored values; temporaries in typed code use FluxRep::FloatRep (unboxed).

- **Breaking change for native backend**: All C runtime functions change calling convention. Existing compiled `.o` files and LLVM IR are incompatible. Mitigated by: native backend is still feature-gated; no stable ABI promise.

- **1-bit integer range loss**: 63-bit vs 64-bit integers. The lost bit is irrelevant — ±4.6 quintillion covers all practical use cases.

---

## Rationale and alternatives

### Why not keep NaN-boxing with wider integers?

The NaN-box format is fundamentally limited to 46-bit payloads because IEEE 754 NaN values only have 51 mantissa bits, minus 4 for the type tag, minus 1 for the quiet NaN bit = 46 bits. There's no way to get more integer range without abandoning NaN-boxing entirely.

### Why not use 128-bit values?

Double the memory footprint for every value. Cache pressure would negate any performance gain. No mainstream language does this.

### Why not use boxed integers (like GHC)?

GHC heap-allocates every `Int` (boxed) and uses `Int#` (unboxed) in optimized code. This works for GHC because its optimizer is extremely sophisticated (worker/wrapper, strictness analysis, demand analysis). Flux's optimizer is simpler — pointer tagging gives 63-bit integers for free without heap allocation.

### Why not tagged pointers with 2-3 tag bits (like GHC)?

GHC uses the lowest 2-3 bits of pointers for constructor tags (exploiting 8-byte alignment). This is more sophisticated but: (a) requires 8-byte alignment guarantees, (b) needs masking on every pointer dereference, (c) is mainly useful for constructor tag dispatch which Flux handles differently. One tag bit is simpler and sufficient.

---

## Prior art

| Language | Scheme | Int bits | Float | Tag check |
|----------|--------|----------|-------|-----------|
| OCaml | 1-bit tag | 63 | Boxed | `val & 1` |
| Lean 4 | 1-bit tag | 63 | Boxed | `val & 1` |
| Erlang/BEAM | Multi-bit tag | 60 | Boxed | `val & 0xF` |
| Lua/LuaJIT | NaN-boxing | 46 | Inline | NaN check |
| JavaScript (V8) | Pointer tagging | 31/63 | Inline (Smi) or boxed | `val & 1` |
| Flux (current) | NaN-boxing | 46 | Inline | NaN check |
| **Flux (proposed)** | **1-bit tag** | **63** | **Boxed** | **`val & 1`** |

---

## Future possibilities

- **Tagged arithmetic**: `tagged_add(a, b) = a + b - 1` works directly on tagged integers without any shift. This could eliminate untag/retag entirely for simple arithmetic in the non-worker path.

- **Unboxed float arrays**: `Array<Float>` could store raw `f64` values instead of boxed pointers, similar to OCaml's float arrays. Requires compile-time knowledge of element type (available via FluxRep).

- **Compressed pointers**: On 64-bit systems with ≤48-bit address space, the upper 16 bits of pointers are unused. These could encode additional type information without extra memory.

- **Polymorphic inline cache**: For polymorphic call sites, cache the last-seen tag and specialize. Pointer tagging makes the tag check (1 instruction) faster than NaN-box tag extraction (3 instructions).
