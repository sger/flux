/*
 * rc.c — Flux Aether RC: Perceus-inspired reference counting.
 *
 * Inspired by Koka's kklib runtime (Perceus RC). Every heap-allocated
 * object has an 8-byte FluxHeader at ptr - 8 containing:
 *   - _Atomic(int32_t) refcount  (sign-bit-encoded, see below)
 *   - uint8_t scan_fsize  (number of child tagged fields to scan on drop;
 *                          arrays and map nodes count theirs in the payload)
 *   - uint8_t obj_tag     (FLUX_OBJ_STRING, FLUX_OBJ_ARRAY, etc.)
 *   - uint8_t progress    (the stackless drop's field index)
 *   - uint8_t size_class  (the block's free-list class, 0 for none)
 *
 * Memory layout:
 *   ┌─────────────────────────┐  ← malloc'd block
 *   │  FluxHeader (8 bytes)   │
 *   │  ├─ refcount            │
 *   │  ├─ scan_fsize          │
 *   │  ├─ obj_tag             │
 *   │  ├─ progress            │
 *   │  └─ size_class          │
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
 * A 1 MB arena is allocated once at init, for the thread that inits.
 * flux_gc_alloc_header tries to bump-allocate from the arena; on overflow,
 * or on any other thread, it falls back to malloc.
 * flux_drop skips free() for arena-resident objects (range check).
 *
 * Freed blocks of up to 256 bytes go on per-thread size-class free lists,
 * and allocation takes from those before the arena or malloc (see "Free
 * lists" below).
 */

#include "flux_rt.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

/* Set to 1 in task worker threads (tasks.c), which keep no free lists (see
 * "Free lists" below). */
__thread int flux_worker_thread = 0;

/* ── FluxHeader ────────────────────────────────────────────────────── */

typedef struct {
    _Atomic(int32_t) refcount;  /* sign-bit-encoded; see top-of-file comment */
    uint8_t  scan_fsize;        /* number of child tagged fields to scan */
    uint8_t  obj_tag;           /* FLUX_OBJ_STRING, FLUX_OBJ_ARRAY, etc. */
    uint8_t  progress;          /* see save_progress */
    uint8_t  size_class;        /* see "Free lists"; 0 = never recycled */
} FluxHeader;

/* ── Refcount helpers ──────────────────────────────────────────────── */

static inline int32_t rc_load_relaxed(FluxHeader *hdr) {
    return atomic_load_explicit(&hdr->refcount, memory_order_relaxed);
}

static inline void rc_store_relaxed(FluxHeader *hdr, int32_t v) {
    atomic_store_explicit(&hdr->refcount, v, memory_order_relaxed);
}

/*
 * A thread-local count (rc > 0) is changed only by the thread that owns its
 * value. ThreadSanitizer treats every atomic access as race-free, so in its
 * build the change is a plain write: a second thread that changes it without
 * synchronising with the owner is then a data race it reports (audit L7).
 * Generated code changes a thread-local count in place without calling here
 * (flux_dup_inline and flux_drop_inline in src/llvm/codegen/prelude.rs), and
 * it is not built with ThreadSanitizer, so those changes go unseen
 * (docs/known_issues.md#ki-159).
 */
static inline void rc_store_local(FluxHeader *hdr, int32_t v) {
#ifdef FLUX_TSAN_COUNTS
    *(int32_t *)&hdr->refcount = v;
#else
    rc_store_relaxed(hdr, v);
#endif
}

/*
 * A count at or past FLUX_RC_STICKY in either mode never changes again, and
 * its block is never freed. Static objects the compiler emits (string
 * literals, closures of top-level functions) start there: they are read-only
 * and have no debug-heap prefix, so the test comes before anything else
 * touches the header. A heap count that grows to the boundary sticks too,
 * instead of overflowing into the other mode (audit L8).
 */
#define FLUX_RC_STICKY 0x40000000

static inline int rc_sticky(int32_t rc) {
    return rc >= FLUX_RC_STICKY || rc <= -FLUX_RC_STICKY;
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

/* ── Debug heap (0188 S10c) ─────────────────────────────────────────── */

/*
 * Built with -DFLUX_DEBUG_HEAP into libflux_rt_debug.a, which
 * `flux --debug-heap` links instead of libflux_rt.a. Generated code is the
 * same: it asks flux_can_use_bump_arena, which says no, so every block is
 * malloc'd here behind a prefix that records where it was allocated.
 *
 *   ┌──────────────────────────┐
 *   │ FluxDebugPrefix          │  magic, size, allocation and free sites
 *   ├──────────────────────────┤
 *   │ FluxHeader (8 bytes)     │
 *   ├──────────────────────────┤
 *   │ payload                  │
 *   └──────────────────────────┘
 *
 * A freed block is never given back: it keeps rc 0, its payload is filled
 * with 0xDEAD…, an odd word that reads as an Int and never as a pointer,
 * and its free site is recorded. The next dup or drop of it stops the
 * program with both sites and the Flux trace.
 */
#ifdef FLUX_DEBUG_HEAP

#define FLUX_SITE_DEPTH  16
#define FLUX_LIVE_MAGIC  0x464c55584c495645ull  /* "FLUXLIVE" */
#define FLUX_FREED_MAGIC 0x464c555846524545ull  /* "FLUXFREE" */
#define FLUX_POISON      0xDEADDEADDEADDEADull

typedef struct FluxDebugPrefix {
    uint64_t  magic;
    uint64_t  size;   /* aligned payload bytes */
    uintptr_t alloc_site[FLUX_SITE_DEPTH];
    uintptr_t free_site[FLUX_SITE_DEPTH];
    /* The live list, in allocation order (0190 S15). */
    struct FluxDebugPrefix *prev;
    struct FluxDebugPrefix *next;
} FluxDebugPrefix;

#if defined(_MSC_VER)
static_assert(sizeof(FluxDebugPrefix) % 8 == 0, "FluxDebugPrefix keeps 8-byte alignment");
#else
_Static_assert(sizeof(FluxDebugPrefix) % 8 == 0, "FluxDebugPrefix keeps 8-byte alignment");
#endif

/* Implemented in Rust (src/runtime/backtrace_abi.rs). */
void   flux_capture_site(uintptr_t *out, size_t len);
size_t flux_describe_site(const uintptr_t *site, size_t len, char *buf, size_t cap);

static inline FluxDebugPrefix *prefix_of(FluxHeader *hdr) {
    return (FluxDebugPrefix *)((char *)hdr - sizeof(FluxDebugPrefix));
}

/* Every block not yet freed, so that the end of the program can name the
 * ones nothing dropped (0190 S15). Task workers allocate too, hence the
 * lock; a spinlock, because MSVC has no pthreads and the sections are a few
 * pointer writes. */
static FluxDebugPrefix *live_first = NULL;
static FluxDebugPrefix *live_last  = NULL;
static size_t           live_count = 0;
static atomic_flag      live_lock  = ATOMIC_FLAG_INIT;
/* Every block allocated and freed so far, for FLUX_HEAP_STATS (0189 F3-0).
 * In-place reuse goes through neither, so a reused cell counts once. */
static size_t           heap_allocs = 0;
static size_t           heap_frees  = 0;

static void live_lock_acquire(void) {
    while (atomic_flag_test_and_set_explicit(&live_lock, memory_order_acquire)) {
    }
}

static void live_lock_release(void) {
    atomic_flag_clear_explicit(&live_lock, memory_order_release);
}

static void live_link(FluxDebugPrefix *prefix) {
    live_lock_acquire();
    prefix->prev = live_last;
    prefix->next = NULL;
    if (live_last) {
        live_last->next = prefix;
    } else {
        live_first = prefix;
    }
    live_last = prefix;
    live_count++;
    heap_allocs++;
    live_lock_release();
}

static void live_unlink(FluxDebugPrefix *prefix) {
    live_lock_acquire();
    if (prefix->prev) {
        prefix->prev->next = prefix->next;
    } else {
        live_first = prefix->next;
    }
    if (prefix->next) {
        prefix->next->prev = prefix->prev;
    } else {
        live_last = prefix->prev;
    }
    prefix->prev = prefix->next = NULL;
    live_count--;
    heap_frees++;
    live_lock_release();
}

static FluxHeader *debug_alloc(size_t aligned) {
    FluxDebugPrefix *prefix =
        (FluxDebugPrefix *)malloc(sizeof(FluxDebugPrefix) + FLUX_HEADER_SIZE + aligned);
    if (!prefix) {
        fprintf(stderr, "flux_gc_alloc: out of memory (%zu bytes)\n", aligned);
        abort();
    }
    prefix->magic = FLUX_LIVE_MAGIC;
    prefix->size  = aligned;
    flux_capture_site(prefix->alloc_site, FLUX_SITE_DEPTH);
    memset(prefix->free_site, 0, sizeof prefix->free_site);
    live_link(prefix);
    return (FluxHeader *)(prefix + 1);
}

/* Quarantines a block instead of freeing it. */
static void debug_free(FluxHeader *hdr) {
    FluxDebugPrefix *prefix = prefix_of(hdr);
    if (prefix->magic != FLUX_LIVE_MAGIC) return;
    live_unlink(prefix);
    prefix->magic = FLUX_FREED_MAGIC;
    flux_capture_site(prefix->free_site, FLUX_SITE_DEPTH);
    rc_store_relaxed(hdr, 0);
    hdr->scan_fsize = 0;
    uint64_t *payload = (uint64_t *)((char *)hdr + FLUX_HEADER_SIZE);
    for (uint64_t i = 0; i < prefix->size / 8; i++) {
        payload[i] = FLUX_POISON;
    }
}

static const char *debug_kind(uint8_t obj_tag) {
    switch (obj_tag) {
        case FLUX_OBJ_STRING:   return "a String";
        case FLUX_OBJ_ADT:      return "a constructor value";
        case FLUX_OBJ_TUPLE:    return "a tuple";
        case FLUX_OBJ_ARRAY:    return "an Array";
        case FLUX_OBJ_CLOSURE:  return "a closure";
        case FLUX_OBJ_BIGINT:   return "a BigInt";
        case FLUX_OBJ_EVIDENCE: return "an effect handler";
        case FLUX_OBJ_FLOAT:    return "a Float";
        case FLUX_OBJ_THUNK:    return "a thunk";
        case FLUX_OBJ_MAP:      return "a Map node";
        default:                return "a value";
    }
}

/* Stops the program at an operation on a freed block. */
static void debug_check(FluxHeader *hdr, const char *operation) {
    FluxDebugPrefix *prefix = prefix_of(hdr);
    if (prefix->magic != FLUX_FREED_MAGIC) return;
    char allocated[512], freed[512], message[1200];
    flux_describe_site(prefix->alloc_site, FLUX_SITE_DEPTH, allocated, sizeof allocated);
    flux_describe_site(prefix->free_site, FLUX_SITE_DEPTH, freed, sizeof freed);
    int len = snprintf(message, sizeof message,
                       "use after free: %s %s that was already freed "
                       "(allocated at %s, freed at %s)",
                       operation, debug_kind(hdr->obj_tag), allocated, freed);
    if (len < 0) len = 0;
    if ((size_t)len >= sizeof message) len = (int)sizeof message - 1;
    flux_heap_fault(message, (uint32_t)len);
}

#define DEBUG_CHECK(hdr, operation) debug_check((hdr), (operation))

#define FLUX_LEAK_SITES 10

void flux_debug_heap_report_stats(void) {
    const char *wanted = getenv("FLUX_HEAP_STATS");
    if (!wanted || !*wanted || strcmp(wanted, "0") == 0) return;
    live_lock_acquire();
    size_t allocs = heap_allocs, frees = heap_frees, live = live_count;
    live_lock_release();
    fprintf(stderr, "heap: allocs=%zu frees=%zu live=%zu\n", allocs, frees, live);
    fflush(stderr);
}

void flux_debug_heap_report_leaks(void) {
    /* Opt-in while nearly every program still leaks (KI-119, KI-121):
     * --debug-heap alone stays a use-after-free check. */
    const char *wanted = getenv("FLUX_LEAK_CHECK");
    if (!wanted || !*wanted || strcmp(wanted, "0") == 0) return;
    live_lock_acquire();
    size_t count = live_count;
    if (count == 0) {
        live_lock_release();
        return;
    }
    fprintf(stderr, "leak: %zu block%s still live at exit\n", count, count == 1 ? "" : "s");
    size_t shown = 0;
    for (FluxDebugPrefix *p = live_first; p && shown < FLUX_LEAK_SITES; p = p->next, shown++) {
        FluxHeader *hdr = (FluxHeader *)(p + 1);
        char allocated[512];
        flux_describe_site(p->alloc_site, FLUX_SITE_DEPTH, allocated, sizeof allocated);
        fprintf(stderr, "  %s (%llu bytes) allocated at %s\n",
                debug_kind(hdr->obj_tag), (unsigned long long)p->size, allocated);
    }
    if (count > shown) {
        fprintf(stderr, "  and %zu more\n", count - shown);
    }
    live_lock_release();
    fflush(stderr);
    exit(FLUX_EXIT_LEAK);
}

#else

#define DEBUG_CHECK(hdr, operation) ((void)0)

#endif /* FLUX_DEBUG_HEAP */

/* ── Bump Arena ───────────────────────────────────────────────────── */

#define FLUX_ARENA_DEFAULT_SIZE  (1 << 20)  /* 1 MB */

/* The arena's bounds, for every thread: a block from it can be released on
 * a worker, which must leave it alone rather than free() it. */
static char *arena_base = NULL;
static char *arena_end  = NULL;

/*
 * The bump pointers, shared with the LLVM inline allocation in
 * src/llvm/codegen/prelude.rs. Each thread has its own, and only the thread
 * that runs arena_init gets an arena: everywhere else both stay NULL, the
 * bump never fits, and allocation takes the slow path. So the inline path
 * needs no test of which thread it is on, and the debug heap, which makes no
 * arena, sends every block through the slow path that records it.
 */
__thread char *flux_arena_hp    = NULL;
__thread char *flux_arena_limit = NULL;

#ifndef FLUX_DEBUG_HEAP
static inline int is_bump_allocated(void *hdr) {
    return arena_base && (char *)hdr >= arena_base && (char *)hdr < arena_end;
}
#endif

static void arena_init(void) {
#ifdef FLUX_DEBUG_HEAP
    /* No arena: every block is recorded (see "Debug heap" above). */
    return;
#endif
    arena_base = (char *)malloc(FLUX_ARENA_DEFAULT_SIZE);
    if (!arena_base) {
        fprintf(stderr, "flux: failed to allocate bump arena (%d bytes)\n",
                FLUX_ARENA_DEFAULT_SIZE);
        abort();
    }
    arena_end        = arena_base + FLUX_ARENA_DEFAULT_SIZE;
    flux_arena_hp    = arena_base;
    flux_arena_limit = arena_end;
}

/* ── Free lists ────────────────────────────────────────────────────── */

/*
 * A block whose payload is at most FLUX_SMALL_MAX bytes records its class,
 * payload / 8, in its header. When it is freed it goes on its thread's list
 * for that class, linked through its first payload word, and the next
 * allocation of that size takes it back: the same block every time, instead
 * of a malloc and free, or an arena block that is never used again.
 *
 * Only threads that may use the arena keep lists. A worker frees with free()
 * and leaves arena blocks alone, as before, so no list is ever shared. A
 * block a worker allocated can still end up on the root thread's list; it
 * came from malloc, which any thread may reuse.
 *
 * Each thread keeps at most FLUX_FREE_CAP bytes on its lists, so a program
 * that frees a large structure gives the rest back to malloc.
 *
 * The debug heap has no lists: every freed block stays quarantined.
 */
#define FLUX_SMALL_MAX   256
#define FLUX_NUM_CLASSES (FLUX_SMALL_MAX / FLUX_ALIGN + 1)
#define FLUX_FREE_CAP    ((size_t)4 << 20)

static inline uint8_t size_class_of(size_t aligned) {
    return aligned <= FLUX_SMALL_MAX ? (uint8_t)(aligned / FLUX_ALIGN) : 0;
}

#ifndef FLUX_DEBUG_HEAP

static __thread FluxHeader *free_lists[FLUX_NUM_CLASSES];
static __thread size_t      free_bytes = 0;

static inline FluxHeader **free_link(FluxHeader *hdr) {
    return (FluxHeader **)((char *)hdr + FLUX_HEADER_SIZE);
}

static inline FluxHeader *free_list_pop(uint8_t size_class) {
    FluxHeader *hdr = free_lists[size_class];
    if (hdr) {
        free_lists[size_class] = *free_link(hdr);
        free_bytes -= FLUX_HEADER_SIZE + (size_t)size_class * FLUX_ALIGN;
    }
    return hdr;
}

static inline int free_list_push(FluxHeader *hdr) {
    uint8_t size_class = hdr->size_class;
    size_t bytes = FLUX_HEADER_SIZE + (size_t)size_class * FLUX_ALIGN;
    if (free_bytes + bytes > FLUX_FREE_CAP) return 0;
    *free_link(hdr) = free_lists[size_class];
    free_lists[size_class] = hdr;
    free_bytes += bytes;
    return 1;
}

/* Empties the calling thread's lists before the arena they may point into
 * is freed. */
static void free_lists_release(void) {
    for (int c = 1; c < FLUX_NUM_CLASSES; c++) {
        FluxHeader *hdr = free_lists[c];
        while (hdr) {
            FluxHeader *next = *free_link(hdr);
            if (!is_bump_allocated(hdr)) free(hdr);
            hdr = next;
        }
        free_lists[c] = NULL;
    }
    free_bytes = 0;
}

#endif /* !FLUX_DEBUG_HEAP */

static inline void flux_block_free(void *ptr) {
    FluxHeader *hdr = header_of(ptr);
#ifdef FLUX_DEBUG_HEAP
    debug_free(hdr);
#else
    if (hdr->size_class && !flux_worker_thread && free_list_push(hdr)) return;
    if (!is_bump_allocated(hdr)) {
        free(hdr);
    }
#endif
}

/* ── Allocation ────────────────────────────────────────────────────── */

static inline void *init_block(FluxHeader *hdr, size_t aligned, uint8_t scan_fsize,
                               uint8_t obj_tag) {
    rc_store_relaxed(hdr, 1);
    hdr->scan_fsize = scan_fsize;
    hdr->obj_tag    = obj_tag;
    hdr->progress   = 0;
    hdr->size_class = size_class_of(aligned);

    void *payload = (char *)hdr + FLUX_HEADER_SIZE;
    memset(payload, 0, aligned);

    return payload;
}

void *flux_gc_alloc_header(uint32_t payload_size, uint8_t scan_fsize, uint8_t obj_tag) {
    size_t aligned = align_up((size_t)payload_size, FLUX_ALIGN);
    size_t total = FLUX_HEADER_SIZE + aligned;

    FluxHeader *hdr = NULL;

#ifdef FLUX_DEBUG_HEAP
    hdr = debug_alloc(aligned);
#else
    uint8_t size_class = size_class_of(aligned);
    if (size_class && !flux_worker_thread) {
        hdr = free_list_pop(size_class);
    }
#endif

    if (!hdr && flux_arena_hp) {
        char *hp = flux_arena_hp;
        if (total <= (size_t)(flux_arena_limit - hp)) {
            /* Fast path: bump allocation from this thread's arena. */
            hdr = (FluxHeader *)hp;
            flux_arena_hp = hp + total;
        }
    }

    if (!hdr) {
        /* Slow path: fall back to malloc. Worker threads never touch the
         * process-global bump pointers, which keeps root-thread bump
         * allocation single-writer even during native parallel re-entry. */
        hdr = (FluxHeader *)malloc(total);
        if (!hdr) {
            fprintf(stderr, "flux_gc_alloc: out of memory (%u bytes)\n", payload_size);
            abort();
        }
    }

    return init_block(hdr, aligned, scan_fsize, obj_tag);
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
 * Called when the inline bump check fails (arena full or uninitialized),
 * or on a worker thread. Takes a recycled block, else mallocs one.
 */
void *flux_bump_alloc_slow(uint32_t payload_size, uint8_t scan_fsize, uint8_t obj_tag) {
    size_t aligned = align_up((size_t)payload_size, FLUX_ALIGN);
    size_t total = FLUX_HEADER_SIZE + aligned;

#ifdef FLUX_DEBUG_HEAP
    (void)total;
    FluxHeader *hdr = debug_alloc(aligned);
#else
    FluxHeader *hdr = NULL;
    uint8_t size_class = size_class_of(aligned);
    if (size_class && !flux_worker_thread) {
        hdr = free_list_pop(size_class);
    }
    if (!hdr) {
        hdr = (FluxHeader *)malloc(total);
        if (!hdr) {
            fprintf(stderr, "flux_bump_alloc_slow: out of memory (%u bytes)\n", payload_size);
            abort();
        }
    }
#endif

    return init_block(hdr, aligned, scan_fsize, obj_tag);
}

void flux_gc_free(void *ptr) {
    if (!ptr) return;
    flux_block_free(ptr);
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
        case FLUX_OBJ_MAP:    return 2;  /* skip kind+nfields+hash (16 bytes) */
        default:              return 0;
    }
}

/*
 * Arrays and map nodes keep their field count in the payload's second word,
 * since it is known only at run time and can exceed the header's one byte.
 */
static inline int counted_in_payload(void *ptr) {
    uint8_t tag = header_of(ptr)->obj_tag;
    return tag == FLUX_OBJ_ARRAY || tag == FLUX_OBJ_MAP;
}

/* How many fields the drop and promotion walks visit. */
static inline uint32_t scan_count(void *ptr) {
    if (counted_in_payload(ptr)) {
        return ((uint32_t *)ptr)[1];
    }
    return header_of(ptr)->scan_fsize;
}

/*
 * Where the stackless drop keeps a block's next field index while it walks a
 * child. The header's `progress` byte is enough for a count that fits in
 * `scan_fsize`, but too small for a long array or collision node; those keep
 * it in their fourth word instead, an array's unused `_pad2` and half a map
 * node's hash, which a node being freed no longer needs.
 */
static inline void save_progress(void *ptr, uint32_t i) {
    if (counted_in_payload(ptr)) {
        ((uint32_t *)ptr)[3] = i;
    } else {
        header_of(ptr)->progress = (uint8_t)i;
    }
}

static inline uint32_t load_progress(void *ptr) {
    if (counted_in_payload(ptr)) {
        return ((uint32_t *)ptr)[3];
    }
    return header_of(ptr)->progress;
}

/*
 * Evidence vectors are not compatible with the generic scan_fsize scanner:
 * their payload starts with a 32-bit count, followed by packed 6-word entries
 * that mix tagged ints and owned references.
 */
#define FLUX_EVV_ENTRY_WORDS  6
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

void flux_dup_range(const int64_t *vals, uint32_t n) {
    for (uint32_t i = 0; i < n; i++) flux_dup(vals[i]);
}

void flux_dup(int64_t val) {
    if (!flux_is_ptr(val)) return;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;
    FluxHeader *hdr = header_of(ptr);
    int32_t rc = rc_load_relaxed(hdr);
    if (rc_sticky(rc)) return;
    DEBUG_CHECK(hdr, "copying");
    if (rc > 0) {
        /* ST mode: only the owning thread is here, plain store is fine. */
        rc_store_local(hdr, rc + 1);
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
    if (rc_sticky(rc)) return NULL;
    DEBUG_CHECK(hdr, "dropping");
    if (rc > 0) {
        int32_t new_rc = rc - 1;
        rc_store_local(hdr, new_rc);
        if (new_rc > 0) return NULL;
    } else {
        /* Last MT drop pairs with all prior releases. */
        int32_t old = atomic_fetch_add_explicit(&hdr->refcount, 1,
                                                memory_order_acq_rel);
        if (old + 1 != 0) return NULL;
    }
    return child;
}

/*
 * Stackless recursive drop.
 *
 * Stores the current field index in the block during traversal (see
 * save_progress), and overwrites field[0] with the parent
 * pointer to maintain an explicit parent chain — no call stack needed.
 *
 * This prevents stack overflow when freeing deep structures like long
 * lists (100K+ Cons cells).
 */
static void flux_drop_free_recx(void *ptr) {
    void *parent = NULL;
    uint32_t scan_fsize;
    uint32_t i;

    int offset;
    int64_t *fields;

move_down:
    scan_fsize = scan_count(ptr);
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
                     * current index in the block. */
                    fields[0] = (int64_t)parent;
                    save_progress(ptr, i);
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
        scan_fsize = scan_count(ptr);
        i = load_progress(ptr);
        goto scan_fields;
    }
}

void flux_drop(int64_t val) {
    if (!flux_is_ptr(val)) return;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;
    FluxHeader *hdr = header_of(ptr);
    int32_t rc = rc_load_relaxed(hdr);
    if (rc_sticky(rc)) return;
    DEBUG_CHECK(hdr, "dropping");
    if (rc > 0) {
        int32_t new_rc = rc - 1;
        rc_store_local(hdr, new_rc);
        if (new_rc > 0) return;
    } else {
        int32_t old = atomic_fetch_add_explicit(&hdr->refcount, 1,
                                                memory_order_acq_rel);
        if (old + 1 != 0) return;
    }

    if (hdr->obj_tag == FLUX_OBJ_EVIDENCE) {
        flux_drop_evidence(ptr);
    } else if (scan_count(ptr) > 0) {
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
    int32_t rc = rc_load_relaxed(hdr);
    if (rc_sticky(rc)) return 0;
    DEBUG_CHECK(hdr, "reusing");
    /* Unique = the caller is the sole ST owner. A shared (rc < 0) object
     * is never reported as unique even if -rc happens to be 1, because
     * other threads may still hold the reference and the inline-reuse
     * fast path is not safe to take. */
    return rc == 1;
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

/* True if `val` is counted by one thread only (rc > 0, not sticky). */
int flux_rc_is_thread_local(int64_t val) {
    if (!flux_is_ptr(val)) return 0;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return 0;
    int32_t rc = rc_load_relaxed(header_of(ptr));
    return rc > 0 && !rc_sticky(rc);
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
    uint32_t count = scan_count(ptr);
    for (uint32_t i = 0; i < count; i++) {
        flux_rc_promote(fields[i]);
    }
}

void flux_rc_promote(int64_t val) {
    if (!flux_is_ptr(val)) return;
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;
    FluxHeader *hdr = header_of(ptr);
    int32_t rc = rc_load_relaxed(hdr);
    if (rc <= 0 || rc_sticky(rc)) {
        /* Already shared (or freed-in-progress), or never freed: a static
         * object is read-only and any thread may read it. */
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
    arena_init();
}

void flux_gc_shutdown(void) {
#ifndef FLUX_DEBUG_HEAP
    free_lists_release();
#endif
    if (arena_base) {
        free(arena_base);
        arena_base       = NULL;
        arena_end        = NULL;
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
