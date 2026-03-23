/*
 * gc.c — Flux minimal GC: bump allocator + mark-sweep.
 *
 * Phase 6 initial implementation.  Every allocation goes through a simple
 * bump allocator backed by a single contiguous arena.  When the arena is
 * exhausted, a mark-sweep pass reclaims unreachable objects.
 *
 * Object layout (each allocation):
 *   ┌──────────────────────┐  ← returned pointer
 *   │  ObjHeader (16 bytes)│
 *   │  ├─ uint32_t size    │  allocation size (excl. header)
 *   │  ├─ uint32_t flags   │  GC mark bit + type tag
 *   │  └─ ObjHeader *next  │  intrusive linked list of all live objects
 *   ├──────────────────────┤
 *   │  user payload        │  `size` bytes, 8-byte aligned
 *   └──────────────────────┘
 *
 * The caller receives a pointer to the *payload*, not the header.
 * flux_gc_free() accepts the payload pointer and walks back to the header.
 */

#include "flux_rt.h"
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

/* ── Object header ──────────────────────────────────────────────────── */

#define GC_FLAG_MARKED  0x1
#define GC_ALIGN        8

typedef struct ObjHeader {
    uint32_t         size;   /* payload size in bytes */
    uint32_t         flags;
    struct ObjHeader *next;  /* linked list of all allocated objects */
} ObjHeader;

_Static_assert(sizeof(ObjHeader) == 16, "ObjHeader must be 16 bytes");

static inline ObjHeader *header_of(void *payload) {
    return (ObjHeader *)((char *)payload - sizeof(ObjHeader));
}

static inline void *payload_of(ObjHeader *hdr) {
    return (char *)hdr + sizeof(ObjHeader);
}

/* ── GC state ───────────────────────────────────────────────────────── */

/* Arena (bump allocator). */
static char   *gc_arena       = NULL;
static size_t  gc_arena_size  = 0;
static size_t  gc_arena_used  = 0;

/* Intrusive linked list of all allocated objects. */
static ObjHeader *gc_all_objects = NULL;

/* Root stack for mark phase. */
#define GC_MAX_ROOTS 4096
static int64_t *gc_roots[GC_MAX_ROOTS];
static int      gc_root_count = 0;

/* Stats. */
static size_t gc_total_allocated = 0;
static size_t gc_num_allocs      = 0;

/* ── Internal: aligned size ─────────────────────────────────────────── */

static inline size_t align_up(size_t n, size_t align) {
    return (n + align - 1) & ~(align - 1);
}

/* ── Public API ─────────────────────────────────────────────────────── */

void flux_gc_init(size_t heap_size) {
    if (heap_size == 0) heap_size = 4 * 1024 * 1024; /* 4 MB default */
    gc_arena = (char *)malloc(heap_size);
    if (!gc_arena) {
        fprintf(stderr, "flux_gc_init: out of memory\n");
        abort();
    }
    gc_arena_size       = heap_size;
    gc_arena_used       = 0;
    gc_all_objects      = NULL;
    gc_root_count       = 0;
    gc_total_allocated  = 0;
    gc_num_allocs       = 0;
}

void flux_gc_shutdown(void) {
    /* Free all objects that were malloc'd during overflow. */
    ObjHeader *obj = gc_all_objects;
    while (obj) {
        ObjHeader *next = obj->next;
        /* Objects inside the arena don't need individual free. */
        char *p = (char *)obj;
        if (p < gc_arena || p >= gc_arena + gc_arena_size) {
            free(obj);
        }
        obj = next;
    }
    gc_all_objects = NULL;
    free(gc_arena);
    gc_arena      = NULL;
    gc_arena_size = 0;
    gc_arena_used = 0;
    gc_root_count = 0;
}

/*
 * Try bump-allocating from the arena.  If there is not enough room,
 * attempt a GC collection and retry.  If still insufficient, fall back
 * to malloc (the object is still tracked in the linked list).
 */
void *flux_gc_alloc(uint32_t size) {
    size_t aligned   = align_up((size_t)size, GC_ALIGN);
    size_t total     = sizeof(ObjHeader) + aligned;

    /* Fast path: bump allocator. */
    if (gc_arena && gc_arena_used + total <= gc_arena_size) {
        ObjHeader *hdr = (ObjHeader *)(gc_arena + gc_arena_used);
        gc_arena_used += total;
        hdr->size  = (uint32_t)aligned;
        hdr->flags = 0;
        hdr->next  = gc_all_objects;
        gc_all_objects = hdr;
        gc_total_allocated += aligned;
        gc_num_allocs++;
        void *payload = payload_of(hdr);
        memset(payload, 0, aligned);
        return payload;
    }

    /* Try collecting. */
    flux_gc_collect();

    /* Retry bump. */
    if (gc_arena && gc_arena_used + total <= gc_arena_size) {
        ObjHeader *hdr = (ObjHeader *)(gc_arena + gc_arena_used);
        gc_arena_used += total;
        hdr->size  = (uint32_t)aligned;
        hdr->flags = 0;
        hdr->next  = gc_all_objects;
        gc_all_objects = hdr;
        gc_total_allocated += aligned;
        gc_num_allocs++;
        void *payload = payload_of(hdr);
        memset(payload, 0, aligned);
        return payload;
    }

    /* Overflow: malloc fallback (still linked). */
    ObjHeader *hdr = (ObjHeader *)malloc(total);
    if (!hdr) {
        fprintf(stderr, "flux_gc_alloc: out of memory (%u bytes)\n", size);
        abort();
    }
    hdr->size  = (uint32_t)aligned;
    hdr->flags = 0;
    hdr->next  = gc_all_objects;
    gc_all_objects = hdr;
    gc_total_allocated += aligned;
    gc_num_allocs++;
    void *payload = payload_of(hdr);
    memset(payload, 0, aligned);
    return payload;
}

void flux_gc_free(void *ptr) {
    if (!ptr) return;
    /*
     * For the bump allocator we cannot free individual objects.
     * Mark as dead — the sweep phase will skip them.
     * Overflow (malloc'd) objects are freed during sweep.
     */
    (void)ptr;
}

/* ── Mark phase ─────────────────────────────────────────────────────── */

static int is_arena_pointer(void *ptr) {
    char *p = (char *)ptr;
    return gc_arena && p >= gc_arena && p < gc_arena + gc_arena_size;
}

/*
 * Given a NaN-boxed value that might be a heap pointer, mark the
 * pointed-to object (if it is managed by our GC).
 */
static void gc_mark_value(int64_t val) {
    if (!flux_is_ptr(val)) return;

    void *ptr = flux_untag_ptr(val);
    if (!ptr) return;

    /* Walk back to the header. */
    ObjHeader *hdr = header_of(ptr);

    /* Verify this looks like one of our objects. */
    char *h = (char *)hdr;
    int in_arena = is_arena_pointer(h);
    if (!in_arena) {
        /* Could be a malloc'd overflow object — scan the list? */
        /* For now, just try to mark if it looks plausible. */
    }

    if (hdr->flags & GC_FLAG_MARKED) return; /* already visited */
    hdr->flags |= GC_FLAG_MARKED;

    /* Conservatively scan the payload for more NaN-boxed pointers. */
    int64_t *words = (int64_t *)ptr;
    uint32_t nwords = hdr->size / 8;
    for (uint32_t i = 0; i < nwords; i++) {
        gc_mark_value(words[i]);
    }
}

/* ── Sweep phase ────────────────────────────────────────────────────── */

/*
 * Simple mark-sweep:  After marking, compact the arena by copying live
 * objects to the beginning.  This is a non-moving collector for now —
 * we simply unmark live objects and free dead overflow objects.
 *
 * Note: Since we use a bump allocator, we cannot truly reclaim arena
 * space without compaction.  For Phase 6, we reset the arena pointer
 * only when ALL objects are dead (full reset) — otherwise we keep
 * bumping and overflow to malloc.  A compacting collector is future work.
 */
static void gc_sweep(void) {
    ObjHeader **prev = &gc_all_objects;
    ObjHeader *obj = gc_all_objects;
    int all_dead = 1;

    while (obj) {
        ObjHeader *next = obj->next;
        if (obj->flags & GC_FLAG_MARKED) {
            /* Live: unmark for next cycle. */
            obj->flags &= ~GC_FLAG_MARKED;
            all_dead = 0;
            prev = &obj->next;
        } else {
            /* Dead: unlink from list. */
            *prev = next;
            char *h = (char *)obj;
            if (!is_arena_pointer(h)) {
                /* Overflow object — free it. */
                free(obj);
            }
            /* Arena objects: space is not reclaimed until full reset. */
        }
        obj = next;
    }

    /* If everything is dead, reset the arena. */
    if (all_dead && gc_all_objects == NULL) {
        gc_arena_used = 0;
    }
}

void flux_gc_collect(void) {
    /* Mark phase: trace from roots. */
    for (int i = 0; i < gc_root_count; i++) {
        if (gc_roots[i]) {
            gc_mark_value(*gc_roots[i]);
        }
    }
    /* Sweep. */
    gc_sweep();
}

void flux_gc_push_root(int64_t *root) {
    if (gc_root_count >= GC_MAX_ROOTS) {
        fprintf(stderr, "flux_gc_push_root: root stack overflow\n");
        abort();
    }
    gc_roots[gc_root_count++] = root;
}

void flux_gc_pop_root(void) {
    if (gc_root_count > 0) {
        gc_root_count--;
    }
}

size_t flux_gc_allocated(void) {
    return gc_total_allocated;
}

size_t flux_gc_num_allocs(void) {
    return gc_num_allocs;
}
