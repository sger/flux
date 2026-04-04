/*
 * rc.c — Flux Aether RC: Perceus-inspired reference counting.
 *
 * Inspired by Koka's kklib runtime (Perceus RC). Every heap-allocated
 * object has an 8-byte FluxHeader at ptr - 8 containing:
 *   - int32_t refcount    (1 = unique, >1 = shared)
 *   - uint8_t scan_fsize  (number of child NaN-boxed fields to scan on drop)
 *   - uint8_t obj_tag     (FLUX_OBJ_STRING, FLUX_OBJ_ARRAY, etc.)
 *   - uint16_t reserved
 *
 * Memory layout:
 *   ┌─────────────────────────┐  ← malloc'd block
 *   │  FluxHeader (8 bytes)   │
 *   │  ├─ refcount            │
 *   │  ├─ scan_fsize          │
 *   │  ├─ obj_tag             │
 *   │  └─ reserved            │
 *   ├─────────────────────────┤
 *   │  payload                │  ← returned pointer
 *   └─────────────────────────┘
 *
 * flux_dup increments the refcount.
 * flux_drop decrements; when it hits 0, recursively drops scan_fsize
 * child fields then frees the block.
 *
 * Phase 7 (Proposal 0140): bump arena for fast-path allocation.
 * A 1 MB arena is allocated once at init.  flux_gc_alloc_header tries
 * to bump-allocate from the arena; on overflow it falls back to malloc.
 * flux_drop skips free() for arena-resident objects (range check).
 */

#include "flux_rt.h"
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

/* ── FluxHeader ────────────────────────────────────────────────────── */

typedef struct {
    int32_t  refcount;     /* 1 = unique, >1 = shared, 0 = ready to free */
    uint8_t  scan_fsize;   /* number of child NaN-boxed fields to scan */
    uint8_t  obj_tag;      /* FLUX_OBJ_STRING, FLUX_OBJ_ARRAY, etc. */
    uint16_t _reserved;
} FluxHeader;

/* _Static_assert is C11; MSVC uses static_assert in C mode. */
#if defined(_MSC_VER)
static_assert(sizeof(FluxHeader) == 8, "FluxHeader must be 8 bytes");
#else
_Static_assert(sizeof(FluxHeader) == 8, "FluxHeader must be 8 bytes");
#endif

#define FLUX_HEADER_SIZE  sizeof(FluxHeader)
#define FLUX_ALIGN        8

static inline FluxHeader *header_of(void *payload) {
    return (FluxHeader *)((char *)payload - FLUX_HEADER_SIZE);
}

static inline size_t align_up(size_t n, size_t align) {
    return (n + align - 1) & ~(align - 1);
}

/* ── Stats ─────────────────────────────────────────────────────────── */

static size_t gc_total_allocated = 0;
static size_t gc_num_allocs      = 0;

/* ── Bump Arena ───────────────────────────────────────────────────── */

#define FLUX_ARENA_DEFAULT_SIZE  (1 << 20)  /* 1 MB */

static char *arena_base = NULL;

/*
 * Exported bump pointers — shared between C runtime and LLVM inline
 * allocation.  The LLVM fast path stores directly to flux_arena_hp,
 * so the C runtime MUST use the same variable (not a private copy)
 * to avoid allocating overlapping regions.
 */
char *flux_arena_hp    = NULL;
char *flux_arena_limit = NULL;

static inline int is_bump_allocated(void *hdr) {
    return arena_base && (char *)hdr >= arena_base && (char *)hdr < flux_arena_limit;
}

static void arena_init(void) {
    arena_base = (char *)malloc(FLUX_ARENA_DEFAULT_SIZE);
    if (!arena_base) {
        fprintf(stderr, "flux: failed to allocate bump arena (%d bytes)\n",
                FLUX_ARENA_DEFAULT_SIZE);
        abort();
    }
    flux_arena_hp    = arena_base;
    flux_arena_limit = arena_base + FLUX_ARENA_DEFAULT_SIZE;
}

/* ── Allocation ────────────────────────────────────────────────────── */

void *flux_gc_alloc_header(uint32_t payload_size, uint8_t scan_fsize, uint8_t obj_tag) {
    size_t aligned = align_up((size_t)payload_size, FLUX_ALIGN);
    size_t total = FLUX_HEADER_SIZE + aligned;

    FluxHeader *hdr;
    char *new_ptr = flux_arena_hp + total;

    if (arena_base && new_ptr <= flux_arena_limit) {
        /* Fast path: bump allocation from the arena. */
        hdr = (FluxHeader *)flux_arena_hp;
        flux_arena_hp = new_ptr;
    } else {
        /* Slow path: fall back to malloc. */
        hdr = (FluxHeader *)malloc(total);
        if (!hdr) {
            fprintf(stderr, "flux_gc_alloc: out of memory (%u bytes)\n", payload_size);
            abort();
        }
    }

    hdr->refcount   = 1;
    hdr->scan_fsize = scan_fsize;
    hdr->obj_tag    = obj_tag;
    hdr->_reserved  = 0;

    void *payload = (char *)hdr + FLUX_HEADER_SIZE;
    memset(payload, 0, aligned);

    gc_total_allocated += aligned;
    gc_num_allocs++;

    return payload;
}

/*
 * Backward-compatible allocator (scan_fsize = 0).
 * Existing code that calls flux_gc_alloc() will not get recursive drop.
 * Migration: switch callers to flux_gc_alloc_header() with correct scan_fsize.
 */
void *flux_gc_alloc(uint32_t size) {
    return flux_gc_alloc_header(size, 0, 0);
}

/*
 * Slow-path allocator for Phase 7b inline LLVM bump allocation.
 * Called when the inline bump check fails (arena full or uninitialized).
 * Uses malloc and initializes the header.
 */
void *flux_bump_alloc_slow(uint32_t payload_size, uint8_t scan_fsize, uint8_t obj_tag) {
    size_t aligned = align_up((size_t)payload_size, FLUX_ALIGN);
    size_t total = FLUX_HEADER_SIZE + aligned;

    FluxHeader *hdr = (FluxHeader *)malloc(total);
    if (!hdr) {
        fprintf(stderr, "flux_bump_alloc_slow: out of memory (%u bytes)\n", payload_size);
        abort();
    }

    hdr->refcount   = 1;
    hdr->scan_fsize = scan_fsize;
    hdr->obj_tag    = obj_tag;
    hdr->_reserved  = 0;

    void *payload = (char *)hdr + FLUX_HEADER_SIZE;
    memset(payload, 0, aligned);

    gc_total_allocated += aligned;
    gc_num_allocs++;

    return payload;
}

void flux_gc_free(void *ptr) {
    if (!ptr) return;
    FluxHeader *hdr = header_of(ptr);
    if (!is_bump_allocated(hdr)) {
        free(hdr);
    }
}

/* ── Reference Counting ────────────────────────────────────────────── */

/*
 * Word offset where scannable NaN-boxed fields start, per object type.
 *
 * Each object type has a fixed-size prefix (metadata) before the
 * NaN-boxed fields that flux_drop should recursively scan.
 *
 * FluxString: { uint8_t tag, pad[3], uint32_t len, char data[] }
 *   → 0 scannable fields (data is raw bytes, scan_fsize should be 0)
 *
 * FluxArray:  { uint8_t tag, pad[3], uint32_t len, uint32_t cap, pad2, i64 elements[] }
 *   → prefix = 16 bytes = 2 words, elements at offset 2
 *
 * FluxTuple:  { uint8_t tag, pad[3], uint32_t arity, i64 elements[] }
 *   → prefix = 8 bytes = 1 word, elements at offset 1
 *
 * ADT:        { int32_t ctor_tag, int32_t field_count, i64 fields[] }
 *   → prefix = 8 bytes = 1 word, fields at offset 1
 *
 * BigInt:     { uint8_t tag, pad[7], int64_t value }
 *   → 0 scannable fields (value is raw int, not NaN-boxed)
 */
static int flux_scan_offset(uint8_t obj_tag) {
    switch (obj_tag) {
        case FLUX_OBJ_ARRAY:  return 2;  /* skip tag+pad+len+cap+pad2 (16 bytes) */
        case FLUX_OBJ_TUPLE:  return 1;  /* skip tag+pad+arity (8 bytes) */
        case FLUX_OBJ_ADT:    return 1;  /* skip ctor_tag+field_count (8 bytes) */
        default:              return 0;
    }
}

void flux_dup(int64_t val) {
    if (!flux_is_ptr(val)) return;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;
    FluxHeader *hdr = header_of(ptr);
    hdr->refcount++;
}

void flux_drop(int64_t val) {
    if (!flux_is_ptr(val)) return;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;
    FluxHeader *hdr = header_of(ptr);
    if (--hdr->refcount > 0) return;

    /* Recursively drop child NaN-boxed fields before freeing. */
    if (hdr->scan_fsize > 0) {
        int offset = flux_scan_offset(hdr->obj_tag);
        int64_t *fields = (int64_t *)((char *)ptr + offset * 8);
        for (int i = 0; i < (int)hdr->scan_fsize; i++) {
            flux_drop(fields[i]);
        }
    }

    if (!is_bump_allocated(hdr)) {
        free(hdr);
    }
    /* Bump-allocated objects: memory stays in the arena.
     * Phase 7c will add free-list recycling. */
}

int flux_rc_is_unique(int64_t val) {
    if (!flux_is_ptr(val)) return 1;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return 1;
    FluxHeader *hdr = header_of(ptr);
    return hdr->refcount == 1;
}

/* ── Lifecycle (no-ops for API compatibility) ──────────────────────── */

void flux_gc_init(size_t heap_size) {
    (void)heap_size;
    gc_total_allocated = 0;
    gc_num_allocs      = 0;
    arena_init();
}

void flux_gc_shutdown(void) {
    if (arena_base) {
        free(arena_base);
        arena_base       = NULL;
        flux_arena_hp    = NULL;
        flux_arena_limit = NULL;
    }
}

void flux_gc_collect(void) {
    /* No-op: Aether RC handles memory deterministically. */
}

void flux_gc_push_root(int64_t *root) {
    (void)root; /* No-op: no tracing GC, no root stack. */
}

void flux_gc_pop_root(void) {
    /* No-op. */
}

/* ── Stats ─────────────────────────────────────────────────────────── */

size_t flux_gc_allocated(void) {
    return gc_total_allocated;
}

size_t flux_gc_num_allocs(void) {
    return gc_num_allocs;
}
