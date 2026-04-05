/*
 * effects.c — Koka-style yield-based algebraic effect handlers (Proposal 0134).
 *
 * Replaces the previous setjmp/longjmp implementation with a yield-based
 * continuation composition model inspired by Koka's Perceus runtime.
 *
 * Algorithm overview:
 *   1. `handle` installs an evidence entry (handler) into the evidence vector
 *      and enters a prompt loop.
 *   2. `perform` sets a global yield flag and returns a sentinel value.
 *      Every function, as it returns, checks the yield flag and adds itself
 *      to a continuation array (via flux_yield_extend).
 *   3. The prompt loop detects the yield, composes the accumulated
 *      continuations into a single closure, and calls the handler clause
 *      with (resume_closure, performed_arg).
 *   4. `resume` calls the composed continuation with the resume value.
 *
 * Both the VM (Rust) and native (C) backends implement the same algorithm,
 * eliminating parity bugs from algorithmic differences.
 */

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ── Yield state (global, single-threaded) ─────────────────────────── */

int32_t  flux_yield_yielding    = 0;   /* 0=no, 1=yielding, 2=final */
int32_t  flux_yield_marker      = 0;   /* target handler's marker */
int64_t  flux_yield_clause      = 0;   /* operation clause closure */
int64_t  flux_yield_op_arg      = 0;   /* performed argument */
int64_t  flux_yield_conts[8];          /* accumulated continuation closures */
int32_t  flux_yield_conts_count = 0;

/* ── Evidence vector ───────────────────────────────────────────────── */

/*
 * Evidence vector: a heap-allocated array of evidence entries.
 * Each entry is 4 pointer-tagged words (32 bytes):
 *   [0] htag       — effect tag (tagged int)
 *   [1] marker     — handler instance id (tagged int)
 *   [2] handler    — handler clause closure (tagged pointer)
 *   [3] parent_evv — saved evidence vector
 *
 * The vector itself is stored as a FluxArray (obj_tag FLUX_OBJ_EVIDENCE)
 * with length = number of entries * 4 words.
 */

#define EVV_ENTRY_WORDS  4
#define EVV_HTAG_OFF     0
#define EVV_MARKER_OFF   1
#define EVV_HANDLER_OFF  2
#define EVV_PARENT_OFF   3

/* Layout: { int32_t count; int64_t entries[]; } where entries are packed. */
typedef struct {
    int32_t count;  /* number of evidence entries */
    int64_t data[]; /* count * EVV_ENTRY_WORDS words */
} EvvArray;

static int64_t current_evv = 0;  /* tagged ptr to EvvArray (0 = empty/FLUX_NONE) */
static int32_t marker_counter = 0;

/* ── Helpers ───────────────────────────────────────────────────────── */

static EvvArray *evv_unbox(int64_t evv) {
    if (evv == 0 || !flux_is_ptr(evv)) return NULL;
    return (EvvArray *)flux_untag_ptr(evv);
}

static int64_t evv_box(EvvArray *arr) {
    if (!arr) return 0;
    return flux_tag_ptr(arr);
}

static EvvArray *evv_alloc(int32_t count) {
    uint32_t size = (uint32_t)(sizeof(int32_t) + (size_t)count * EVV_ENTRY_WORDS * sizeof(int64_t));
    /* Use scan_fsize=0 since evidence entries are managed explicitly. */
    EvvArray *arr = (EvvArray *)flux_gc_alloc_header(size, 0, FLUX_OBJ_EVIDENCE);
    arr->count = count;
    return arr;
}

/* ── Public API ────────────────────────────────────────────────────── */

int64_t flux_evv_get(void) {
    return current_evv;
}

void flux_evv_set(int64_t evv) {
    current_evv = evv;
}

int64_t flux_fresh_marker(void) {
    marker_counter++;
    return flux_tag_int((int64_t)marker_counter);
}

/*
 * Insert an evidence entry into the evidence vector.
 * Returns a new evidence vector with the entry added.
 * Entries are appended (not sorted) for simplicity — linear lookup by htag.
 */
int64_t flux_evv_insert(int64_t evv, int64_t htag, int64_t marker, int64_t handler) {
    EvvArray *old = evv_unbox(evv);
    int32_t old_count = old ? old->count : 0;
    int32_t new_count = old_count + 1;

    EvvArray *arr = evv_alloc(new_count);

    /* Copy existing entries. */
    if (old && old_count > 0) {
        memcpy(arr->data, old->data,
               (size_t)old_count * EVV_ENTRY_WORDS * sizeof(int64_t));
    }

    /* Append new entry at the end (most recent = highest priority). */
    int64_t *entry = &arr->data[old_count * EVV_ENTRY_WORDS];
    entry[EVV_HTAG_OFF]    = htag;
    entry[EVV_MARKER_OFF]  = marker;
    entry[EVV_HANDLER_OFF] = handler;
    entry[EVV_PARENT_OFF]  = evv;  /* save parent evv for restoration */

    return evv_box(arr);
}

/*
 * Look up evidence by effect tag (htag).
 * Searches from the end (most recently installed handler first).
 * Returns the entry index or -1 if not found.
 */
static int evv_lookup(EvvArray *arr, int64_t htag) {
    if (!arr) return -1;
    for (int i = arr->count - 1; i >= 0; i--) {
        if (arr->data[i * EVV_ENTRY_WORDS + EVV_HTAG_OFF] == htag) {
            return i;
        }
    }
    return -1;
}

/*
 * Perform an effect: set yield state and return sentinel.
 *
 * htag:  effect tag (tagged int)
 * optag: operation tag (tagged int) — currently unused for single-op dispatch
 * arg:   the performed argument (tagged value)
 *
 * The caller must check flux_yield_yielding after every call and propagate
 * the sentinel + extend continuations as needed.
 */
int64_t flux_yield_to(int64_t htag, int64_t optag, int64_t arg) {
    (void)optag;  /* reserved for multi-op dispatch */

    EvvArray *arr = evv_unbox(current_evv);
    int idx = evv_lookup(arr, htag);

    if (idx < 0) {
        fprintf(stderr, "flux_yield_to: unhandled effect (htag=0x%llx)\n",
                (unsigned long long)(uint64_t)htag);
        abort();
    }

    int64_t *entry = &arr->data[idx * EVV_ENTRY_WORDS];
    int32_t m = (int32_t)flux_untag_int(entry[EVV_MARKER_OFF]);
    int64_t clause = entry[EVV_HANDLER_OFF];

    flux_yield_yielding    = 1;
    flux_yield_marker      = m;
    flux_yield_clause      = clause;
    flux_yield_op_arg      = arg;
    flux_yield_conts_count = 0;

    return FLUX_YIELD_SENTINEL;
}

/*
 * Perform an effect using the direct (tail-resumptive) fast path.
 *
 * Instead of setting the yield flag and unwinding, this directly calls the
 * handler clause with (resume_closure, arg). The resume_closure is provided
 * by the caller (typically an identity function for tail-resumptive handlers).
 *
 * This is correct when the handler always calls resume in tail position.
 * For the general case (non-tail-resumptive), use flux_yield_to + yield checks.
 */
int64_t flux_perform_direct(int64_t htag, int64_t optag, int64_t arg, int64_t resume) {
    (void)optag;  /* reserved for multi-op dispatch */

    EvvArray *arr = evv_unbox(current_evv);
    int idx = evv_lookup(arr, htag);

    if (idx < 0) {
        fprintf(stderr, "flux_perform_direct: unhandled effect (htag=0x%llx)\n",
                (unsigned long long)(uint64_t)htag);
        abort();
    }

    int64_t *entry = &arr->data[idx * EVV_ENTRY_WORDS];
    int64_t clause = entry[EVV_HANDLER_OFF];

    /* Direct call: clause(resume, arg) */
    int64_t args[2] = { resume, arg };
    return flux_call_closure_c(clause, args, 2);
}

/*
 * Extend the continuation chain during yield propagation.
 * Called by each function as it unwinds after detecting yield.
 *
 * cont: a closure representing "the rest of this function's computation"
 */
int64_t flux_yield_extend(int64_t cont) {
    if (flux_yield_conts_count >= 8) {
        /* Overflow: compose existing conts into one, then add the new one. */
        int64_t composed = flux_compose_conts();
        flux_yield_conts[0] = composed;
        flux_yield_conts_count = 1;
    }
    flux_yield_conts[flux_yield_conts_count++] = cont;
    return FLUX_YIELD_SENTINEL;
}

/*
 * Compose accumulated continuations into a single closure.
 *
 * Given conts[0..n], builds a closure that when called with a value v,
 * computes: cont_n(...(cont_1(cont_0(v))))
 *
 * i.e., cont_0 is the innermost (closest to perform), cont_n is outermost.
 *
 * For a single continuation, returns it directly.
 * For zero continuations, returns None (identity).
 */
int64_t flux_compose_conts(void) {
    if (flux_yield_conts_count == 0) {
        return flux_make_none();
    }
    if (flux_yield_conts_count == 1) {
        int64_t result = flux_yield_conts[0];
        flux_yield_conts_count = 0;
        return result;
    }

    /*
     * Compose multiple continuations by chaining:
     * Start with cont_0, wrap it so calling the result calls cont_0 first,
     * then passes the result to cont_1, etc.
     *
     * We do this by creating a "chain" — for now, just fold:
     * composed(v) = cont_n(cont_{n-1}(...(cont_0(v))...))
     *
     * Implementation: store all conts into an array, create a trampoline
     * closure that iterates through them. For simplicity in Phase 1,
     * we build a chain by wrapping pairs.
     *
     * Phase 1 approach: return an array of continuations and have
     * flux_yield_prompt iterate through them. This avoids needing to
     * build composed closures in C.
     */

    /* Package continuations into an array for flux_yield_prompt to iterate. */
    int64_t *elems = flux_yield_conts;
    int32_t count = flux_yield_conts_count;
    int64_t arr = flux_array_new(elems, count);
    flux_yield_conts_count = 0;
    return arr;
}

/*
 * Check if currently yielding.
 * Returns the yielding flag (0, 1, or 2) for LLVM to branch on.
 */
int32_t flux_is_yielding(void) {
    return flux_yield_yielding;
}

/*
 * Prompt loop: check if a yield is targeted at this handler.
 *
 * marker:      this handler's marker (tagged int)
 * saved_evv:   the evidence vector before this handler was installed
 * body_result: the result of the handled body expression
 *
 * Returns:
 *   - body_result if not yielding (pure path)
 *   - the handler clause result if this marker matches
 *   - FLUX_YIELD_SENTINEL if yielding but marker doesn't match (propagate)
 */
int64_t flux_yield_prompt(int64_t marker, int64_t saved_evv, int64_t body_result) {
    /* Restore evidence vector. */
    current_evv = saved_evv;

    if (flux_yield_yielding == 0) {
        /* Pure path: body completed without performing. */
        return body_result;
    }

    int32_t m = (int32_t)flux_untag_int(marker);

    if (flux_yield_marker != m) {
        /* Not our yield — propagate upward. */
        return FLUX_YIELD_SENTINEL;
    }

    /* This yield is for us. Compose continuations into a resume closure. */
    int64_t clause = flux_yield_clause;
    int64_t op_arg = flux_yield_op_arg;

    /* Build the resume closure from accumulated continuations. */
    int64_t resume_cont = flux_compose_conts();

    /* Clear yield state. */
    flux_yield_yielding    = 0;
    flux_yield_marker      = 0;
    flux_yield_clause      = 0;
    flux_yield_op_arg      = 0;
    flux_yield_conts_count = 0;

    /*
     * Call the handler clause: clause(resume_cont, op_arg)
     *
     * The clause is a closure with signature (resume, arg) -> result.
     * `resume_cont` is either:
     *   - A single continuation closure (call it with a value to resume)
     *   - An array of continuations (flux_yield_prompt iterates them)
     *   - None (no continuation — perform was the last expression)
     */
    int64_t args[2] = { resume_cont, op_arg };
    int64_t result = flux_call_closure_c(clause, args, 2);

    return result;
}
