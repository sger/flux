/*
 * rc.c — Flux Aether RC: Perceus-inspired reference counting.
 *
 * Inspired by Koka's kklib runtime (Perceus RC). Every heap-allocated
 * object has an 8-byte FluxHeader at ptr - 8 containing:
 *   - _Atomic(int32_t) refcount  (sign-bit-encoded, see below)
 *   - uint8_t scan_fsize  (number of child tagged fields to scan on drop)
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
 * Hybrid atomic-on-share refcount (proposal 0174 Phase 1a-iv):
 *
 *   Sign-bit encoding (Lean 4 / Koka scheme):
 *     rc >  0 — single-threaded mode. rc = number of references. Owner
 *               thread increments/decrements with relaxed atomics; on x86
 *               this compiles to plain `mov`. No cross-thread cost.
 *     rc <  0 — thread-shared mode. -rc = number of references. dup uses
 *               relaxed atomic fetch_sub (more refs → more negative); drop
 *               uses acq_rel atomic fetch_add. The last drop (transition
 *               to 0) is the synchronization point: acquire pairs with the
 *               releases of every prior drop so the final freeing thread
 *               sees all writes that happened-before any reference release.
 *     rc == 0 — ready to free in either mode.
 *
 *   Promotion (ST → MT) happens at explicit cross-worker boundaries
 *   (`Channel.send`, `Task.spawn`) via flux_rc_promote_deep. While the
 *   object is ST, the LLVM-emitted inline `rc == 1` uniqueness checks
 *   (in src/llvm/codegen/prelude.rs) work as before because rc=1 means
 *   "1 ST owner = unique." Once promoted, every rc < 0 fails those
 *   inline checks and falls back to flux_drop / fresh allocation, which
 *   then takes the atomic path.
 *
 * Mixing atomic (this file) and non-atomic (LLVM-emitted) accesses to the
 * same word is sound here because the modes do not overlap in practice:
 * while rc > 0, only the owning thread reads/writes; while rc < 0, only
 * flux_dup/flux_drop touch the field, and they use atomic ops. The
 * publication of an MT object (the negation in flux_rc_promote_deep) is
 * the synchronization point that gives every other thread a happens-after
 * view of the prior ST writes.
 *
 * Phase 7 (Proposal 0140): bump arena for fast-path allocation.
 * A 1 MB arena is allocated once at init.  flux_gc_alloc_header tries
 * to bump-allocate from the arena; on overflow it falls back to malloc.
 * flux_drop skips free() for arena-resident objects (range check).
 */

#include "flux_rt.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

/* ── FluxHeader ────────────────────────────────────────────────────── */

typedef struct {
    _Atomic(int32_t) refcount;  /* sign-bit-encoded; see top-of-file comment */
    uint8_t  scan_fsize;        /* number of child tagged fields to scan */
    uint8_t  obj_tag;           /* FLUX_OBJ_STRING, FLUX_OBJ_ARRAY, etc. */
    uint16_t _reserved;
} FluxHeader;

/* ── Refcount helpers ──────────────────────────────────────────────── */

static inline int32_t rc_load_relaxed(FluxHeader *hdr) {
    return atomic_load_explicit(&hdr->refcount, memory_order_relaxed);
}

static inline void rc_store_relaxed(FluxHeader *hdr, int32_t v) {
    atomic_store_explicit(&hdr->refcount, v, memory_order_relaxed);
}

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

    rc_store_relaxed(hdr, 1);
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

    rc_store_relaxed(hdr, 1);
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
 * Word offset where scannable tagged fields start, per object type.
 *
 * Each object type has a fixed-size prefix (metadata) before the
 * tagged fields that flux_drop should recursively scan.
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
 *   → 0 scannable fields (value is raw int, not a tagged pointer)
 */
static int flux_scan_offset(uint8_t obj_tag) {
    switch (obj_tag) {
        case FLUX_OBJ_ARRAY:  return 2;  /* skip tag+pad+len+cap+pad2 (16 bytes) */
        case FLUX_OBJ_TUPLE:  return 1;  /* skip tag+pad+arity (8 bytes) */
        case FLUX_OBJ_ADT:    return 1;  /* skip ctor_tag+field_count (8 bytes) */
        case FLUX_OBJ_CLOSURE: return 3; /* skip fn_ptr + arity/count metadata (24 bytes) */
        default:              return 0;
    }
}

static inline void flux_block_free(void *ptr);

/*
 * Evidence vectors are not compatible with the generic scan_fsize scanner:
 * their payload starts with a 32-bit count, followed by packed 5-word entries
 * that mix tagged ints and owned references.
 */
#define FLUX_EVV_ENTRY_WORDS  5
#define FLUX_EVV_HANDLER_OFF  2
#define FLUX_EVV_PARENT_OFF   3
#define FLUX_EVV_STATE_OFF    4

typedef struct {
    int32_t count;
    int64_t data[];
} FluxEvvArray;

static void flux_drop_evidence(void *ptr) {
    FluxEvvArray *evv = (FluxEvvArray *)ptr;
    for (int32_t i = 0; i < evv->count; i++) {
        int64_t *entry = &evv->data[i * FLUX_EVV_ENTRY_WORDS];
        flux_drop(entry[FLUX_EVV_HANDLER_OFF]);
        flux_drop(entry[FLUX_EVV_PARENT_OFF]);
        flux_drop(entry[FLUX_EVV_STATE_OFF]);
    }
    flux_block_free(ptr);
}

void flux_dup(int64_t val) {
    if (!flux_is_ptr(val)) return;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;
    FluxHeader *hdr = header_of(ptr);
    int32_t rc = rc_load_relaxed(hdr);
    if (rc > 0) {
        /* ST mode: only the owning thread is here, plain store is fine. */
        rc_store_relaxed(hdr, rc + 1);
    } else {
        /* MT mode: |rc| owners; one more reference makes rc more negative. */
        atomic_fetch_sub_explicit(&hdr->refcount, 1, memory_order_relaxed);
    }
}

/*
 * Decrement refcount of a child field.  If the child becomes ready-to-free
 * (refcount drops to 0), return it for further processing; otherwise
 * return NULL. Honours the same sign-bit encoding as flux_drop.
 */
static inline void *flux_field_should_free(void *ptr, int field_idx) {
    int64_t *fields = (int64_t *)ptr;
    int64_t val = fields[field_idx];
    if (!flux_is_ptr(val)) return NULL;
    void *child = flux_untag_ptr(val);
    if (!child) return NULL;
    FluxHeader *hdr = header_of(child);
    int32_t rc = rc_load_relaxed(hdr);
    if (rc > 0) {
        int32_t new_rc = rc - 1;
        rc_store_relaxed(hdr, new_rc);
        if (new_rc > 0) return NULL;
    } else {
        /* Last MT drop pairs with all prior releases. */
        int32_t old = atomic_fetch_add_explicit(&hdr->refcount, 1,
                                                memory_order_acq_rel);
        if (old + 1 != 0) return NULL;
    }
    return child;
}

static inline void flux_block_free(void *ptr) {
    FluxHeader *hdr = header_of(ptr);
    if (!is_bump_allocated(hdr)) {
        free(hdr);
    }
}

/*
 * Stackless recursive drop.
 *
 * Uses the _reserved field in FluxHeader to store the current field
 * index during traversal, and overwrites field[0] with the parent
 * pointer to maintain an explicit parent chain — no call stack needed.
 *
 * This prevents stack overflow when freeing deep structures like long
 * lists (100K+ Cons cells).
 */
static void flux_drop_free_recx(void *ptr) {
    void *parent = NULL;
    uint16_t scan_fsize;
    uint16_t i;

    int offset;
    int64_t *fields;

move_down:
    scan_fsize = header_of(ptr)->scan_fsize;
    offset = flux_scan_offset(header_of(ptr)->obj_tag);
    fields = (int64_t *)((char *)ptr + offset * 8);

    if (scan_fsize == 0) {
        /* Leaf node: free directly. */
        flux_block_free(ptr);
    }
    else if (scan_fsize == 1) {
        /* Single child: free block and tail-call into child. */
        void *child = flux_field_should_free(fields, 0);
        flux_block_free(ptr);
        if (child) { ptr = child; goto move_down; }
    }
    else {
        /* Multiple children: iterate fields, saving progress in header. */
        i = 0;

    scan_fields:
        do {
            void *child = flux_field_should_free(fields, i);
            i++;
            if (child) {
                if (i < scan_fsize) {
                    /* Save progress: parent pointer in field[0],
                     * current index in _reserved. */
                    fields[0] = (int64_t)parent;
                    header_of(ptr)->_reserved = i;
                    parent = ptr;
                }
                else {
                    /* Last field: free block, continue with child. */
                    flux_block_free(ptr);
                }
                ptr = child;
                goto move_down;
            }
        } while (i < scan_fsize);
        flux_block_free(ptr);
    }

    /* Move up along the parent chain. */
    if (parent) {
        ptr = parent;
        offset = flux_scan_offset(header_of(ptr)->obj_tag);
        fields = (int64_t *)((char *)ptr + offset * 8);
        parent = (void *)fields[0];
        scan_fsize = header_of(ptr)->scan_fsize;
        i = (uint16_t)header_of(ptr)->_reserved;
        goto scan_fields;
    }
}

void flux_drop(int64_t val) {
    if (!flux_is_ptr(val)) return;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;
    FluxHeader *hdr = header_of(ptr);
    int32_t rc = rc_load_relaxed(hdr);
    if (rc > 0) {
        int32_t new_rc = rc - 1;
        rc_store_relaxed(hdr, new_rc);
        if (new_rc > 0) return;
    } else {
        int32_t old = atomic_fetch_add_explicit(&hdr->refcount, 1,
                                                memory_order_acq_rel);
        if (old + 1 != 0) return;
    }

    if (hdr->obj_tag == FLUX_OBJ_EVIDENCE) {
        flux_drop_evidence(ptr);
    } else if (hdr->scan_fsize > 0) {
        flux_drop_free_recx(ptr);
    } else {
        flux_block_free(ptr);
    }
}

int flux_rc_is_unique(int64_t val) {
    if (!flux_is_ptr(val)) return 1;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return 1;
    FluxHeader *hdr = header_of(ptr);
    /* Unique = the caller is the sole ST owner. A shared (rc < 0) object
     * is never reported as unique even if -rc happens to be 1, because
     * other threads may still hold the reference and the inline-reuse
     * fast path is not safe to take. */
    return rc_load_relaxed(hdr) == 1;
}

/*
 * True if `val` has been shared across threads (rc < 0). Used by the
 * scheduler to decide whether a value crossing a worker boundary still
 * needs promotion.
 */
int flux_rc_is_shared(int64_t val) {
    if (!flux_is_ptr(val)) return 0;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return 0;
    return rc_load_relaxed(header_of(ptr)) < 0;
}

/*
 * Recursively promote `val` from ST to MT mode (proposal 0174 Phase 1a-iv).
 *
 * Walks the object graph and atomically negates every refcount it finds
 * still in ST mode. Idempotent — already-shared subgraphs are skipped.
 *
 * Synchronization: the caller must own the only reference to `val` (and
 * everything reachable from it) at the moment of the call. The release
 * semantics on the negation publish all prior writes from the owning
 * thread; subsequent threads that load the negative rc with acquire
 * semantics (in flux_drop) see a consistent view.
 *
 * This walks the same field offsets flux_drop_free_recx uses, so any
 * object the recursive drop can scan, this can promote. Evidence vectors
 * have a custom layout and are handled separately.
 */
static void flux_rc_promote_recurse(void *ptr);

static void flux_rc_promote_evidence(void *ptr) {
    FluxEvvArray *evv = (FluxEvvArray *)ptr;
    for (int32_t i = 0; i < evv->count; i++) {
        int64_t *entry = &evv->data[i * FLUX_EVV_ENTRY_WORDS];
        flux_rc_promote(entry[FLUX_EVV_HANDLER_OFF]);
        flux_rc_promote(entry[FLUX_EVV_PARENT_OFF]);
        flux_rc_promote(entry[FLUX_EVV_STATE_OFF]);
    }
}

static void flux_rc_promote_recurse(void *ptr) {
    FluxHeader *hdr = header_of(ptr);
    int offset = flux_scan_offset(hdr->obj_tag);
    int64_t *fields = (int64_t *)((char *)ptr + offset * 8);
    if (hdr->obj_tag == FLUX_OBJ_EVIDENCE) {
        flux_rc_promote_evidence(ptr);
        return;
    }
    for (uint8_t i = 0; i < hdr->scan_fsize; i++) {
        flux_rc_promote(fields[i]);
    }
}

void flux_rc_promote(int64_t val) {
    if (!flux_is_ptr(val)) return;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;
    FluxHeader *hdr = header_of(ptr);
    int32_t rc = rc_load_relaxed(hdr);
    if (rc <= 0) {
        /* Already shared (or freed-in-progress) — nothing to do. */
        return;
    }
    /* Negate with release ordering so subsequent acquire loads in
     * flux_drop on other threads see all of this thread's prior writes
     * to the payload. */
    atomic_store_explicit(&hdr->refcount, -rc, memory_order_release);
    flux_rc_promote_recurse(ptr);
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
