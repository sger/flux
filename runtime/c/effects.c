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

/*
 * Yield state (global, single-threaded).
 *
 * This runtime is not concurrency-ready: yield state, evidence state, and the
 * legacy direct-resume counter are process globals. Any future actor/thread
 * runtime must make these thread/fiber-local or move them into an explicit
 * scheduler context.
 *
 * The yield payload globals below currently hold borrowed tagged values while
 * the native yield path unwinds. Correctness depends on LIR continuation
 * lowering preserving those values until flux_yield_prompt consumes them; they
 * are not explicit RC roots yet.
 */

int32_t  flux_yield_yielding    = 0;   /* 0=no, 1=yielding, 2=final */
int32_t  flux_yield_marker      = 0;   /* target handler's marker */
int64_t  flux_yield_clause      = 0;   /* operation clause closure */
int64_t  flux_yield_op_arg      = 0;   /* performed argument (unused for 0-arity) */
int64_t  flux_yield_op_state    = 0;   /* current handler parameter, or 0 when absent */
int32_t  flux_yield_op_arity    = 0;   /* user-visible arity of the op (0 or 1) */
int64_t  flux_yield_conts[8];          /* accumulated continuation closures */
int32_t  flux_yield_conts_count = 0;
int64_t  flux_yield_evv         = 0;   /* current_evv at yield time (slice 5-tr-fix) */

/* ── Evidence vector ───────────────────────────────────────────────── */

/*
 * Evidence vector: a heap-allocated array of evidence entries.
 * Each entry is 5 pointer-tagged words (40 bytes):
 *   [0] htag       — effect tag (tagged int)
 *   [1] marker     — handler instance id (tagged int)
 *   [2] handler    — handler clause closure (tagged pointer)
 *   [3] parent_evv — saved evidence vector
 *   [4] state      — parameterized handler state, or 0 when absent
 *
 * The vector itself is stored as a FluxArray (obj_tag FLUX_OBJ_EVIDENCE)
 * with length = number of entries * 4 words.
 */

#define EVV_ENTRY_WORDS  5
#define EVV_HTAG_OFF     0
#define EVV_MARKER_OFF   1
#define EVV_HANDLER_OFF  2
#define EVV_PARENT_OFF   3
#define EVV_STATE_OFF    4

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
    /* Use scan_fsize=0: evidence entries have a custom scanner in rc.c
     * because the payload starts with a 32-bit count and each packed entry
     * mixes tagged ints with owned heap references. */
    EvvArray *arr = (EvvArray *)flux_gc_alloc_header(size, 0, FLUX_OBJ_EVIDENCE);
    arr->count = count;
    return arr;
}

static void evv_dup_owned_fields(const int64_t *entry) {
    flux_dup(entry[EVV_HANDLER_OFF]);
    flux_dup(entry[EVV_PARENT_OFF]);
    flux_dup(entry[EVV_STATE_OFF]);
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
int64_t flux_evv_insert(int64_t evv, int64_t htag, int64_t marker, int64_t handler, int64_t state) {
    EvvArray *old = evv_unbox(evv);
    int32_t old_count = old ? old->count : 0;
    int32_t new_count = old_count + 1;

    EvvArray *arr = evv_alloc(new_count);

    /* Copy existing entries. */
    if (old && old_count > 0) {
        memcpy(arr->data, old->data,
               (size_t)old_count * EVV_ENTRY_WORDS * sizeof(int64_t));
        for (int32_t i = 0; i < old_count; i++) {
            evv_dup_owned_fields(&arr->data[i * EVV_ENTRY_WORDS]);
        }
    }

    /* Append new entry at the end (most recent = highest priority). */
    int64_t *entry = &arr->data[old_count * EVV_ENTRY_WORDS];
    entry[EVV_HTAG_OFF]    = htag;
    entry[EVV_MARKER_OFF]  = marker;
    entry[EVV_HANDLER_OFF] = handler;
    entry[EVV_PARENT_OFF]  = evv;  /* save parent evv for restoration */
    entry[EVV_STATE_OFF]   = state;
    evv_dup_owned_fields(entry);

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
 * Look up evidence by marker (raw, not tagged). Used by the prompt loop in
 * `flux_yield_prompt` to handle foreign-marker yields whose inner prompt has
 * already unwound (slice 5-tr-nested).
 */
static int evv_lookup_by_marker(EvvArray *arr, int32_t marker) {
    if (!arr) return -1;
    for (int i = arr->count - 1; i >= 0; i--) {
        int64_t entry_marker =
            arr->data[i * EVV_ENTRY_WORDS + EVV_MARKER_OFF];
        if ((int32_t)flux_untag_int(entry_marker) == marker) {
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
 * arity: user-visible arity of the op (0 for `() -> T`, 1 for `A -> T`).
 *        Needed so flux_yield_prompt can call the clause with the correct
 *        number of args: `(resume)` when arity=0 (no user arg) or
 *        `(resume, arg)` when arity=1.
 *
 * The caller must check flux_yield_yielding after every call and propagate
 * the sentinel + extend continuations as needed.
 */
int64_t flux_yield_to(int64_t htag, int64_t optag, int64_t arg, int64_t arity) {
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
    flux_yield_op_state    = entry[EVV_STATE_OFF];
    flux_yield_op_arity    = (int32_t)flux_untag_int(arity);
    flux_yield_conts_count = 0;
    /* Slice 5-tr-fix: capture the evv at yield time so the composed
     * continuation can re-install it on resume. Nested handlers rely on
     * this because an inner handle may unwind (restoring its parent evv)
     * before the outer handle's clause decides to resume into the inner
     * scope. */
    flux_yield_evv         = current_evv;

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
 *
 * Legacy direct-perform fallback: when the handler clause short-circuits
 * (returns without invoking `resume`), naive direct-perform silently returns
 * the clause's value to the perform site — producing wrong answers because
 * the short-circuit should unwind the entire handle-block, not feed the
 * value back as the perform's result.
 *
 * Detecting the short-circuit requires the caller to set
 * `flux_resume_called = 0` before calling flux_perform_direct, then check it
 * after. The `resume` closure passed in is synthesized by the compiler; when
 * it is the identity closure, we cannot observe whether it ran. To make the
 * observation possible without rewriting the compiler's closure synthesis,
 * the direct-perform path wraps the clause call in a shared-flag check:
 * short-circuit detection currently reports a structured runtime error so
 * the failure is loud and diagnosable, rather than silently corrupting the
 * result. The default native path uses yield checks for full continuation
 * support; this direct path remains as an opt-out/legacy fast path and keeps
 * loud diagnostics for handler shapes it cannot represent.
 */

/*
 * Shared counter used by the resume-shape detector. Each invocation of the
 * compiler-synthesised identity-resume closure increments it; after the
 * clause returns, flux_perform_direct inspects the counter to classify the
 * handler shape:
 *   - counter == 0 : short-circuit (non-TR discard-style)   → E1200
 *   - counter == 1 : tail-resumptive                         → ok
 *   - counter >= 2 : multi-shot                              → E1201
 *
 * Single-threaded today, fits the rest of the runtime's global-state model.
 * This is a concurrency blocker until resume/effect state becomes fiber-local
 * or is threaded through an explicit runtime context.
 */
int32_t flux_resume_called = 0;
int32_t flux_direct_resume_marker = 0;

static void flux_update_state_for_marker(int32_t marker, int64_t next_state) {
    EvvArray *arr = evv_unbox(current_evv);
    int idx = evv_lookup_by_marker(arr, marker);
    if (idx >= 0) {
        int64_t *slot = &arr->data[idx * EVV_ENTRY_WORDS + EVV_STATE_OFF];
        flux_dup(next_state);
        flux_drop(*slot);
        *slot = next_state;
    }
}

/*
 * Called by the compiler-emitted identity-resume closure. Bumps the counter
 * and returns its argument. When flux_perform_direct sees the counter in an
 * unsupported band after the clause returns, it reports a structured error.
 *
 * Kept for the legacy direct-perform path. The default native lowering routes
 * performs through flux_yield_to + prompt handling, which supports non-TR
 * unwinding and native multi-shot composition.
 */
int64_t flux_resume_mark_called(int64_t value) {
    flux_resume_called += 1;
    return value;
}

/*
 * Closure-entry wrapper for flux_resume_mark_called.
 *
 * The compiler's MakeExternClosure LIR instruction emits a reference to
 * `<symbol>.closure_entry`, which the LLVM backend declares as an external
 * function with signature `i64 (i64 closure_raw, i8* args_ptr, i32 nargs)`.
 * C source can't contain `.` in identifiers, so we declare the wrapper with
 * an asm() label to give it the required LLVM symbol name.  On Mach-O the
 * assembler prefixes an underscore to C symbols; the asm() label must match
 * what the LLVM backend emits, including that platform-specific prefix.
 *
 * Portability note: this GNU/Clang asm-label spelling is only covered by the
 * Unix-like native toolchains used today. Windows/MSVC native support needs a
 * dedicated symbol-export strategy for closure-entry names containing dots.
 */
#if defined(__APPLE__)
#  define FLUX_CLOSURE_ENTRY_SYMBOL "_flux_resume_mark_called.closure_entry"
#else
#  define FLUX_CLOSURE_ENTRY_SYMBOL "flux_resume_mark_called.closure_entry"
#endif

int64_t flux_resume_mark_called_closure_entry(int64_t closure_raw, int64_t *args_ptr, int32_t nargs)
    __asm__(FLUX_CLOSURE_ENTRY_SYMBOL);

int64_t flux_resume_mark_called_closure_entry(int64_t closure_raw, int64_t *args_ptr, int32_t nargs) {
    (void)closure_raw;
    if (nargs >= 2 && flux_direct_resume_marker != 0) {
        flux_update_state_for_marker(flux_direct_resume_marker, args_ptr[1]);
    }
    return flux_resume_mark_called(args_ptr[0]);
}

int64_t flux_perform_direct(int64_t htag, int64_t optag, int64_t arg, int64_t resume, int64_t arity) {
    (void)optag;  /* reserved for multi-op dispatch */

    EvvArray *arr = evv_unbox(current_evv);
    int idx = evv_lookup(arr, htag);

    if (idx < 0) {
        fprintf(stderr, "flux_perform_direct: unhandled effect (htag=0x%llx)\n",
                (unsigned long long)(uint64_t)htag);
        abort();
    }

    int64_t *entry = &arr->data[idx * EVV_ENTRY_WORDS];
    int32_t marker = (int32_t)flux_untag_int(entry[EVV_MARKER_OFF]);
    int64_t clause = entry[EVV_HANDLER_OFF];
    int64_t state = entry[EVV_STATE_OFF];

    /* Save & reset the counter so nested performs don't confuse the detector. */
    int32_t saved_count = flux_resume_called;
    int32_t saved_direct_marker = flux_direct_resume_marker;
    flux_resume_called = 0;
    flux_direct_resume_marker = marker;

    /*
     * Direct call: clause(resume, arg0, ..., argN).
     *
     * Today native direct-perform lowering only materializes zero-arg and
     * one-arg effect operations:
     *   - arity == 0: clause(resume)
     *   - arity == 1: clause(resume, arg)
     */
    int64_t argc = flux_untag_int(arity);
    int64_t result;
    if (state != 0 && argc <= 0) {
        int64_t args[2] = { resume, state };
        result = flux_call_closure_c(clause, args, 2);
    } else if (state != 0) {
        int64_t args[3] = { resume, arg, state };
        result = flux_call_closure_c(clause, args, 3);
    } else if (argc <= 0) {
        int64_t args[1] = { resume };
        result = flux_call_closure_c(clause, args, 1);
    } else {
        int64_t args[2] = { resume, arg };
        result = flux_call_closure_c(clause, args, 2);
    }

    /*
     * Classify the clause's resume behaviour:
     *   0  → short-circuit / discard (non-TR)                       E1200
     *   1  → tail-resumptive, correct answer in `result`             ok
     *   2+ → multi-shot (composed branches never materialise on the  E1201
     *        native fast path — the identity closure can't split
     *        execution into independent continuation tails).
     *
     * Both error bands require Phase 3 proper (yield-based unwinding
     * + multi-shot continuation composition) to lift the restriction.
     */
    if (flux_resume_called == 0) {
        fprintf(stderr,
            "error[E1200]: Non-Tail-Resumptive Handler On Native\n"
            "\n"
            "A handler clause returned without invoking `resume`. This is\n"
            "the exception/short-circuit pattern, which the native backend\n"
            "cannot yet express — the short-circuit would need to unwind\n"
            "the entire handle-block, which requires continuation-capture\n"
            "support planned for Proposal 0162 Phase 3 proper.\n"
            "\n"
            "The VM backend supports this shape. Run with --native disabled\n"
            "(the default) until Phase 3 lands.\n");
        abort();
    }
    if (flux_resume_called >= 2) {
        fprintf(stderr,
            "error[E1201]: Multi-Shot Handler On Native\n"
            "\n"
            "A handler clause invoked `resume` more than once (%d times).\n"
            "Multi-shot effect handlers (search, backtracking,\n"
            "non-determinism) require composing the captured\n"
            "continuation into independent branches — the native backend's\n"
            "direct-dispatch fast path cannot express this today.\n"
            "\n"
            "Proposal 0162 Phase 3 proper adds multi-shot continuation\n"
            "composition via the yield/prompt runtime.  Until then the VM\n"
            "backend also does not support multi-shot (it enforces\n"
            "one-shot continuations), so this program has no valid\n"
            "execution path — the handler needs to be rewritten.\n",
            (int)flux_resume_called);
        abort();
    }

    /* Restore the outer counter; we're now unwinding past this perform. */
    flux_resume_called = saved_count;
    flux_direct_resume_marker = saved_direct_marker;

    return result;
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
 * Trampoline entry for a multi-cont composed resume closure.
 *
 * The closure captures a single int64 value — the tagged pointer to the
 * conts-array the compose step built. When the clause calls the composed
 * closure with (v), this entry:
 *   1. Reads the conts array from the captures.
 *   2. Calls cont_0(v), then cont_1(result), ..., cont_n(result).
 *   3. Returns the final result.
 *
 * Signature matches the standard closure-entry convention:
 *   (i64 closure, ptr args, i32 nargs) -> i64.
 *
 * Exposed under `flux_compose_trampoline.closure_entry` via an asm() label
 * so flux_make_closure can install it as a function pointer. The `.entry`
 * token can't appear in a C identifier, so we follow the same pattern as
 * flux_resume_mark_called_closure_entry in this file.
 *
 * See the portability note above: Windows/MSVC needs a separate spelling for
 * these exported closure-entry symbols.
 */
#if defined(__APPLE__)
#  define FLUX_COMPOSE_TRAMPOLINE_SYMBOL "_flux_compose_trampoline.closure_entry"
#else
#  define FLUX_COMPOSE_TRAMPOLINE_SYMBOL "flux_compose_trampoline.closure_entry"
#endif

int64_t flux_compose_trampoline_closure_entry(int64_t closure_raw, int64_t *args_ptr, int32_t nargs)
    __asm__(FLUX_COMPOSE_TRAMPOLINE_SYMBOL);

int64_t flux_compose_trampoline_closure_entry(int64_t closure_raw, int64_t *args_ptr, int32_t nargs) {
    (void)nargs; /* always 1 for a resume(v) call */

    /* Unpack captures: conts array plus target marker. FluxClosure layout mirrors
     * flux_rt.c / llvm/codegen/closure.rs — 24-byte header then payload. */
    void *clo_ptr = flux_untag_ptr(closure_raw);
    int64_t *payload = (int64_t *)((char *)clo_ptr + 24);
    int64_t conts_arr = payload[0];
    int32_t marker = (int32_t)flux_untag_int(payload[1]);

    if (nargs >= 2) {
        flux_update_state_for_marker(marker, args_ptr[1]);
    }

    int64_t count = flux_untag_int(flux_array_len(conts_arr));
    int64_t result = args_ptr[0];
    for (int64_t i = 0; i < count; i++) {
        int64_t cont = flux_array_get(conts_arr, flux_tag_int(i));
        int64_t arg_slot[1] = { result };
        result = flux_call_closure_c(cont, arg_slot, 1);
    }
    return result;
}

/*
 * Compose accumulated continuations into a single resume closure.
 *
 * Given conts[0..n], returns a closure that when called with a value v,
 * computes: cont_n(...(cont_1(cont_0(v))))
 *
 * i.e., cont_0 is the innermost (closest to perform), cont_n is outermost.
 *
 * Zero conts: returns None — the clause is expected not to call resume.
 * Single cont: returns it directly — no trampoline overhead.
 * Multiple conts: packages into an array and wraps in a trampoline closure.
 */
int64_t flux_compose_conts(void) {
    if (flux_yield_conts_count == 0) {
        return flux_make_none();
    }
    if (flux_yield_conts_count == 1 && flux_yield_op_state == 0) {
        int64_t result = flux_yield_conts[0];
        flux_yield_conts_count = 0;
        return result;
    }

    /* Package continuations into an array. */
    int64_t *elems = flux_yield_conts;
    int32_t count = flux_yield_conts_count;
    int64_t arr = flux_array_new(elems, count);
    flux_yield_conts_count = 0;

    /* Build a trampoline closure by hand. The FluxClosure layout
     * (runtime/c/flux_rt.c lines 1025-1032):
     *   { void *fn_ptr; int32_t remaining_arity; int32_t capture_count;
     *     int32_t applied_count; int32_t _pad; int64_t payload[]; }
     * Header size = 24 bytes; payload is i64-aligned. We stash the conts
     * array (tagged pointer) as the single capture. */
    int64_t marker = flux_tag_int((int64_t)flux_yield_marker);
    uint32_t payload_bytes = (uint32_t)(2 * sizeof(int64_t)); /* conts + marker */
    uint32_t size = 24 + payload_bytes;
    void *mem = flux_gc_alloc_header(size, 2 /* scan_fsize = 2 captures */,
                                     FLUX_OBJ_CLOSURE);
    struct FluxClosureLayout {
        void   *fn_ptr;
        int32_t remaining_arity;
        int32_t capture_count;
        int32_t applied_count;
        int32_t _pad;
        int64_t payload[];
    };
    struct FluxClosureLayout *clo = (struct FluxClosureLayout *)mem;
    clo->fn_ptr          = (void *)flux_compose_trampoline_closure_entry;
    clo->remaining_arity = (flux_yield_op_state != 0) ? 2 : 1;
    clo->capture_count   = 2;
    clo->applied_count   = 0;
    clo->_pad            = 0;
    clo->payload[0]      = arr;
    clo->payload[1]      = marker;

    return flux_tag_ptr(mem);
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
    int32_t m = (int32_t)flux_untag_int(marker);
    int64_t result = body_result;

    /* Prompt loop: handle re-yields that occur while invoking the clause.
     * A clause that calls `resume(v)` may trigger a nested perform inside
     * the composed continuation, which re-sets yielding=1. We catch the
     * re-yield here and run another iteration. */
    while (flux_yield_yielding != 0) {
        int is_ours = (flux_yield_marker == m);
        int64_t yield_evv_local = flux_yield_evv;

        if (!is_ours) {
            /* Slice 5-tr-nested: the re-yield targets a different handler.
             * Its C prompt frame may have already unwound for nested shapes
             * like `handle Inner {} handle Outer {}`. Look up the marker in
             * the yield-site evv: if found, service it inline using its
             * recorded clause and evv. Otherwise propagate. */
            EvvArray *arr = evv_unbox(yield_evv_local);
            int idx = evv_lookup_by_marker(arr, flux_yield_marker);
            if (idx < 0) {
                current_evv = saved_evv;
                return FLUX_YIELD_SENTINEL;
            }
            /* Service inline. The clause + evv come from the yield state
             * itself — flux_yield_clause was set by flux_yield_to based on
             * evv_lookup(htag) at the yield site, so it's the correct
             * foreign handler's clause. */
        }

        /* Keep the yield-site evv active for the duration of the clause
         * call so that any resume → composed-continuation → nested perform
         * finds the full installed handler chain (slice 5-tr-fix). */

        int64_t clause   = flux_yield_clause;
        int64_t op_arg   = flux_yield_op_arg;
        int64_t op_state = flux_yield_op_state;
        int32_t op_arity = flux_yield_op_arity;
        int64_t yield_evv = yield_evv_local;

        /* Build the resume closure from accumulated continuations. */
        int64_t resume_cont = flux_compose_conts();

        /* Clear yield state so the clause sees a clean slate (it may re-yield). */
        flux_yield_yielding    = 0;
        flux_yield_marker      = 0;
        flux_yield_clause      = 0;
        flux_yield_op_arg      = 0;
        flux_yield_op_state    = 0;
        flux_yield_op_arity    = 0;
        flux_yield_conts_count = 0;
        flux_yield_evv         = 0;

        /* Re-install the evv that was active at the yield site so the clause
         * — including any resume it calls into the composed continuation —
         * sees the full handler chain the yield was performed under (slice
         * 5-tr-fix: matters for nested handlers where an inner handle may
         * have unwound before the outer prompt decides to resume). */
        current_evv = yield_evv;

        /* Call the handler clause: clause(resume, [arg], [state]). */
        if (op_state != 0 && op_arity <= 0) {
            int64_t args[2] = { resume_cont, op_state };
            result = flux_call_closure_c(clause, args, 2);
        } else if (op_state != 0) {
            int64_t args[3] = { resume_cont, op_arg, op_state };
            result = flux_call_closure_c(clause, args, 3);
        } else if (op_arity <= 0) {
            int64_t args[1] = { resume_cont };
            result = flux_call_closure_c(clause, args, 1);
        } else {
            int64_t args[2] = { resume_cont, op_arg };
            result = flux_call_closure_c(clause, args, 2);
        }
        /* Loop: if the clause re-yielded (resume inside triggered a nested
         * perform targeting this same handler), iterate. Otherwise exit. */
    }

    /* No more yields — restore parent evv and return the final value. */
    current_evv = saved_evv;
    return result;
}
