- Feature Name: Unified C Runtime — Aether RC
- Start Date: 2026-03-27
- Status: Implemented
- Proposal PR:
- Flux Issue:
- Depends on: None

## Summary

Rewrite the C runtime's memory management from mark-sweep GC to Aether-compatible reference counting. After this proposal, both the LLVM native backend and the C runtime use the same memory layout: an 8-byte refcount word at `ptr - 8`, managed by `flux_dup`/`flux_drop`.

This is the critical prerequisite for all subsequent unification work (unified primops, NaN-boxed VM values, unified PrimOp enum).

## Motivation

### The memory model incompatibility

The C runtime uses **mark-sweep GC** with a 16-byte `ObjHeader`:

```
C runtime (current):
  ┌──────────────────────┐  ← malloc'd block
  │  ObjHeader (16 bytes)│
  │  ├─ uint32_t size    │
  │  ├─ uint32_t flags   │  (GC mark bit)
  │  └─ ObjHeader *next  │  (intrusive linked list)
  ├──────────────────────┤
  │  payload             │  ← returned pointer
  └──────────────────────┘
```

The LLVM native backend uses **Aether RC** with an 8-byte refcount:

```
LLVM Aether (current):
  ┌──────────────────────┐  ← malloc'd block
  │  int64_t refcount    │  (8 bytes, at ptr - 8)
  ├──────────────────────┤
  │  payload             │  ← returned pointer
  └──────────────────────┘
```

**These layouts are incompatible.** The LLVM Aether prelude expects an RC word at `ptr - 8`. The C runtime puts part of `ObjHeader` there. The VM cannot safely call C functions that allocate because the returned pointers have the wrong layout.

### How GHC solves this

GHC has **one** memory manager (the RTS garbage collector). All backends allocate through the same heap. There is no layout mismatch.

### What this proposal achieves

After this change, both backends use the same layout:

| | LLVM native | C runtime |
|---|---|---|
| Allocation | `flux_gc_alloc` | `flux_gc_alloc` |
| Layout | RC word at `ptr - 8` | RC word at `ptr - 8` |
| Retain | `flux_dup` | `flux_dup` |
| Release | `flux_drop` | `flux_drop` |

**Same allocator. Same layout. Same RC functions.**

---

## Design

### Step 1: Koka-inspired header design

Inspired by Koka's Perceus runtime (`kklib`), each heap object gets an 8-byte header with a refcount **and** a `scan_fsize` field. The `scan_fsize` tells `flux_drop` how many child fields to recursively drop — preventing memory leaks for compound objects.

**Koka's header** (for reference):
```c
typedef struct {
    uint8_t   scan_fsize;          // fields to scan on drop
    uint8_t   _field_idx;          // stackless freeing
    uint16_t  tag;                 // constructor tag
    _Atomic(int32_t) refcount;     // reference count (0 = unique)
} kk_header_t;  // 8 bytes
```

**Flux header** (Koka-inspired):
```c
// At ptr - 8, before every heap object payload
typedef struct {
    int32_t  refcount;      // 1 = unique, >1 = shared, 0 = free
    uint8_t  scan_fsize;    // number of child i64 fields to scan on drop
    uint8_t  obj_tag;       // FLUX_OBJ_STRING, FLUX_OBJ_ARRAY, etc.
    uint16_t _reserved;
} FluxHeader;  // 8 bytes

_Static_assert(sizeof(FluxHeader) == 8, "FluxHeader must be 8 bytes");
```

**Memory layout:**
```
  ┌─────────────────────────────┐  ← malloc'd block
  │  FluxHeader (8 bytes)       │
  │  ├─ int32_t refcount        │
  │  ├─ uint8_t scan_fsize      │
  │  ├─ uint8_t obj_tag         │
  │  └─ uint16_t reserved       │
  ├─────────────────────────────┤
  │  payload                    │  ← returned pointer (header at ptr - 8)
  └─────────────────────────────┘
```

### Step 2: Rewrite `flux_gc_alloc` with header

**File: `runtime/c/gc.c`**

```c
// Allocate with explicit header fields
void *flux_gc_alloc_header(uint32_t payload_size, uint8_t scan_fsize, uint8_t obj_tag) {
    size_t aligned = (payload_size + 7) & ~(size_t)7;
    size_t total = sizeof(FluxHeader) + aligned;
    FluxHeader *hdr = (FluxHeader *)malloc(total);
    if (!hdr) { fprintf(stderr, "out of memory\n"); abort(); }
    hdr->refcount = 1;
    hdr->scan_fsize = scan_fsize;
    hdr->obj_tag = obj_tag;
    hdr->_reserved = 0;
    void *payload = (char *)hdr + sizeof(FluxHeader);
    memset(payload, 0, aligned);
    return payload;
}

// Backward-compatible wrapper (scan_fsize = 0, no recursive drop)
void *flux_gc_alloc(uint32_t size) {
    return flux_gc_alloc_header(size, 0, 0);
}
```

**How scan_fsize is set per object type:**

```c
// String: 0 scannable fields (data is raw bytes)
FluxString *s = flux_gc_alloc_header(size, 0, FLUX_OBJ_STRING);

// Array of 5 elements: 5 scannable fields
FluxArray *a = flux_gc_alloc_header(size, 5, FLUX_OBJ_ARRAY);

// Cons cell (2 fields: head, tail): 2 scannable
void *cons = flux_gc_alloc_header(24, 2, FLUX_OBJ_ADT);

// ADT with 3 fields: 3 scannable
void *adt = flux_gc_alloc_header(size, 3, FLUX_OBJ_ADT);
```

### Step 3: Implement `flux_dup` / `flux_drop` with recursive scanning

```c
void flux_dup(int64_t val) {
    if (!flux_is_ptr(val)) return;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;
    FluxHeader *hdr = (FluxHeader *)((char *)ptr - 8);
    hdr->refcount++;
}

void flux_drop(int64_t val) {
    if (!flux_is_ptr(val)) return;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;
    FluxHeader *hdr = (FluxHeader *)((char *)ptr - 8);
    if (--hdr->refcount > 0) return;

    // Recursively drop child fields before freeing
    int offset = flux_scan_offset(hdr->obj_tag);
    int64_t *fields = (int64_t *)((char *)ptr + offset * 8);
    for (int i = 0; i < hdr->scan_fsize; i++) {
        flux_drop(fields[i]);
    }
    free(hdr);
}

// Word offset where scannable NaN-boxed fields start
static int flux_scan_offset(uint8_t obj_tag) {
    switch (obj_tag) {
        case FLUX_OBJ_STRING: return 0;  // no scannable fields
        case FLUX_OBJ_ARRAY:  return 2;  // skip {tag,pad,len,cap,pad2} → fields at offset 16
        case FLUX_OBJ_TUPLE:  return 1;  // skip {tag,pad,arity} → fields at offset 8
        case FLUX_OBJ_ADT:    return 1;  // skip {ctor_tag,field_count} → fields at offset 8
        default:              return 0;
    }
}

int flux_rc_is_unique(int64_t val) {
    if (!flux_is_ptr(val)) return 1;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return 1;
    FluxHeader *hdr = (FluxHeader *)((char *)ptr - 8);
    return hdr->refcount == 1;
}
```

### Step 4: Update `flux_gc_free`

```c
void flux_gc_free(void *ptr) {
    if (!ptr) return;
    free((char *)ptr - sizeof(FluxHeader));
}
```

### Step 4: Delete mark-sweep GC

Remove entirely from `gc.c`:
- `ObjHeader` struct and linked list (`gc_all_objects`)
- `gc_mark_value()`, `gc_sweep()`, `flux_gc_collect()`
- Root stack (`gc_roots`, `flux_gc_push_root`, `flux_gc_pop_root`)
- Arena allocator state (`gc_arena`, `gc_arena_size`, `gc_arena_used`)

Keep as no-ops for API compatibility:
- `flux_gc_init()`, `flux_gc_shutdown()`
- `flux_gc_collect()`, `flux_gc_push_root()`, `flux_gc_pop_root()`

### Step 5: Update LLVM Aether prelude

**File: `src/core_to_llvm/codegen/prelude.rs`**

Replace inline LLVM IR for `flux_dup`/`flux_drop` (~120 lines) with external C function declarations:

```rust
// Before: ~60 lines of inline LLVM IR per function
fn emit_dup(...) { /* extract payload, shift, load RC, increment, store */ }
fn emit_drop(...) { /* extract payload, shift, load RC, decrement, conditional free */ }

// After: declare external C functions
fn emit_dup(...) {
    // declare ccc void @flux_dup(i64)
}
fn emit_drop(...) {
    // declare ccc void @flux_drop(i64)
}
```

**File: `src/core_to_llvm/codegen/aether.rs`**

Update call conventions for dup/drop calls: `Fastcc` → `Ccc` (they're now C functions, not inline LLVM).

### Step 6: Declare in header

**File: `runtime/c/flux_rt.h`**

```c
void flux_dup(int64_t val);
void flux_drop(int64_t val);
int  flux_rc_is_unique(int64_t val);
```

---

## Files modified

| File | Change | Lines |
|------|--------|-------|
| `runtime/c/gc.c` | Rewrite: delete ObjHeader/mark-sweep, add RC alloc + dup/drop | ~150 rewritten |
| `runtime/c/flux_rt.h` | Add flux_dup/drop/rc_is_unique declarations | ~3 added |
| `src/core_to_llvm/codegen/prelude.rs` | Replace inline dup/drop with external C declarations | ~120 removed, ~15 added |
| `src/core_to_llvm/codegen/aether.rs` | Fastcc → Ccc for dup/drop calls | ~4 changed |

---

## Verification

1. `cargo build` — compiles (VM unaffected, C runtime not yet linked)
2. `cargo build --features core_to_llvm` — LLVM backend compiles with new C declarations
3. `cargo test --all` — all existing tests pass
4. Native backend parity:
   ```bash
   cargo run --features core_to_llvm -- examples/basics/fibonacci.flx --native --no-cache
   # Must produce correct output (0, 1, 1, 2, 3, 5, 8, 55)
   ```
5. C runtime smoke test:
   ```bash
   cd runtime/c && make smoke_test && ./smoke_test
   ```

## What this enables

After this proposal, the C runtime's memory layout is Aether-compatible. This unblocks:

- **Proposal 0131 Phase 1+**: Link C runtime into VM binary (`build.rs` + `cc` crate)
- **Proposal 0133**: Unified PrimOp enum — VM calls C functions directly
- **NaN-boxed VM values**: VM passes raw `i64` to C with zero conversion

## Comparison: Flux header vs Koka header

| | Koka (`kk_header_t`) | Flux (`FluxHeader`) |
|---|---|---|
| Size | 8 bytes | 8 bytes |
| Refcount | `int32_t` (atomic, 0 = unique) | `int32_t` (non-atomic, 1 = unique) |
| Scan fields | `uint8_t scan_fsize` | `uint8_t scan_fsize` |
| Type tag | `uint16_t tag` (constructor tag) | `uint8_t obj_tag` (object type) |
| Thread safety | Atomic ops for shared refs | Non-atomic (single-threaded) |
| Sticky refs | Overflow to negative = never freed | Not needed (no threads) |
| Stackless free | `_field_idx` for iterative drop | Not yet (future optimization) |

Koka's `_field_idx` enables stackless freeing — instead of recursion, it walks fields iteratively using the field index as a cursor. This prevents stack overflow on deeply nested structures. Flux can add this later if needed.

## Drawbacks

- **No cycle detection**: Aether RC cannot handle reference cycles. Flux values must remain acyclic (DAGs only). This is already enforced by the language design (immutable values, no mutable cells).
- **Stack overflow on deep drop**: `flux_drop` recurses into child fields. Very deep structures (e.g., 100K-element cons list) could overflow the C stack. Mitigated by: Koka's stackless freeing pattern can be adopted later using `_field_idx`.
- **All existing C code needs updating**: Functions like `flux_string_new`, `flux_array_new` must call `flux_gc_alloc_header` with correct `scan_fsize` instead of `flux_gc_alloc`. This is mechanical but touches every allocation site.
