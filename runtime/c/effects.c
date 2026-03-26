/*
 * effects.c — Algebraic effect handler stack and continuations.
 *
 * Uses setjmp/longjmp for one-shot continuations.  This matches the
 * semantics of Flux's algebraic effects where each continuation is
 * resumed at most once (one-shot / linear).
 *
 * Handler stack layout:
 *   Each handler is pushed before entering a `handle` block and popped
 *   on exit.  When `perform` is called, the handler stack is searched
 *   top-down for a matching effect tag.  The handler function is invoked
 *   with the performed argument and a continuation value.
 *
 * Continuation representation:
 *   A continuation captures a setjmp point.  When resumed, longjmp
 *   restores the stack frame and passes the resume value.  Since
 *   continuations are one-shot, resuming invalidates the continuation.
 */

#include "flux_rt.h"
#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ── Handler stack ──────────────────────────────────────────────────── */

#define MAX_HANDLERS 256

typedef struct {
    int64_t  effect_tag;   /* NaN-boxed tag identifying the effect */
    void    *handler_fn;   /* handler function pointer */
    void    *resume_fn;    /* resume wrapper pointer */
    jmp_buf  env;          /* setjmp state for this handler frame */
    int      active;       /* 1 = installed, 0 = consumed/popped */
} HandlerFrame;

static HandlerFrame handler_stack[MAX_HANDLERS];
static int handler_top = 0;

/* ── Continuation ───────────────────────────────────────────────────── */

/*
 * Continuation object (GC-allocated).
 * Contains everything needed to resume at the perform site.
 */
typedef struct {
    jmp_buf  env;       /* perform-site setjmp state */
    int64_t  result;    /* value passed by resume() */
    int      resumed;   /* 0 = pending, 1 = resumed */
} Continuation;

/*
 * Thread-local state for the currently active perform/resume exchange.
 * This avoids passing data through longjmp (which only passes an int).
 */
static int64_t  perform_arg       = 0;
static int64_t  perform_effect    = 0;
static Continuation *active_cont  = NULL;

/* ── Public API ─────────────────────────────────────────────────────── */

void flux_push_handler(int64_t effect_tag, void *handler_fn, void *resume_fn) {
    if (handler_top >= MAX_HANDLERS) {
        fprintf(stderr, "flux_push_handler: handler stack overflow\n");
        abort();
    }
    HandlerFrame *frame = &handler_stack[handler_top];
    frame->effect_tag = effect_tag;
    frame->handler_fn = handler_fn;
    frame->resume_fn  = resume_fn;
    frame->active     = 1;
    handler_top++;
}

void flux_pop_handler(void) {
    if (handler_top > 0) {
        handler_top--;
        handler_stack[handler_top].active = 0;
    }
}

/*
 * Perform an effect: search the handler stack for a matching handler,
 * capture the current continuation, and invoke the handler.
 *
 * Returns the value that the handler passes to resume().
 */
int64_t flux_perform(int64_t effect_tag, int64_t arg) {
    /* Find the nearest matching handler. */
    int found = -1;
    for (int i = handler_top - 1; i >= 0; i--) {
        if (handler_stack[i].active && handler_stack[i].effect_tag == effect_tag) {
            found = i;
            break;
        }
    }

    if (found < 0) {
        fprintf(stderr, "flux_perform: unhandled effect (tag=0x%llx)\n",
                (unsigned long long)(uint64_t)effect_tag);
        abort();
    }

    /* Allocate a continuation object. */
    Continuation *cont = (Continuation *)flux_gc_alloc((uint32_t)sizeof(Continuation));
    cont->result  = 0;
    cont->resumed = 0;

    /* Save the perform site so resume() can return here. */
    if (setjmp(cont->env) != 0) {
        /* We get here when resume() calls longjmp. */
        return cont->result;
    }

    /* Store state for the handler. */
    perform_arg    = arg;
    perform_effect = effect_tag;
    active_cont    = cont;

    /*
     * Call the handler function.
     * Handler signature: int64_t handler(int64_t arg, int64_t continuation)
     * The continuation is passed as a NaN-boxed pointer to the Continuation.
     */
    typedef int64_t (*HandlerFn)(int64_t, int64_t);
    HandlerFn handler = (HandlerFn)handler_stack[found].handler_fn;

    int64_t cont_val = flux_tag_ptr(cont);

    /* Deactivate this handler frame (handlers don't recurse into themselves). */
    handler_stack[found].active = 0;

    int64_t handler_result = handler(arg, cont_val);

    /* If the handler returns without resuming, the continuation is abandoned. */
    /* Reactivate the handler frame for future perform calls. */
    handler_stack[found].active = 1;

    return handler_result;
}

/*
 * Resume a captured continuation with a value.
 * This returns control to the perform site.
 */
int64_t flux_resume(int64_t continuation, int64_t value) {
    Continuation *cont = (Continuation *)flux_untag_ptr(continuation);
    if (!cont) {
        fprintf(stderr, "flux_resume: null continuation\n");
        abort();
    }
    if (cont->resumed) {
        fprintf(stderr, "flux_resume: continuation already resumed (one-shot violation)\n");
        abort();
    }

    cont->resumed = 1;
    cont->result  = value;

    /* Jump back to the perform site. */
    longjmp(cont->env, 1);

    /* Unreachable. */
    return flux_make_none();
}
