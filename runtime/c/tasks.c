/*
 * tasks.c — Native Task<a> runtime for proposal 0174 D5-b/c.
 *
 * Implements flux_task_spawn / flux_task_blocking_join / flux_task_cancel
 * using POSIX threads.  Each spawn creates a real OS thread; blocking_join
 * waits on a per-task condvar; cancel sets a flag (best-effort, Phase 1a
 * semantics — running tasks complete normally).
 *
 * Phase 1a cancel semantics (matches the VM backend):
 *   - cancelled before pickup  → worker skips body, join raises TaskCancelled
 *   - cancelled while running  → flag set, body still completes
 *   - cancelled after join     → no-op (slot already freed)
 *
 * Thread safety:
 *   - task_table_mutex serialises slot allocation and freeing only; it is
 *     never held during task body execution or cond_wait.
 *   - Per-slot mutex + condvar protect the status field.
 *   - cancelled_flag is _Atomic so the worker can load it without the lock.
 *   - flux_dup/flux_drop use atomic RC (sign-bit encoding, Phase 1a-iv).
 *   - Worker threads set flux_worker_thread = 1 so all Flux allocations go
 *     through malloc, bypassing the non-thread-safe bump arena (rc.c).
 *
 * Windows: CreateThread / CRITICAL_SECTION / CONDITION_VARIABLE will be wired
 * in a follow-up.  The abort() fallback keeps native Task calls loud on that
 * platform until then.
 */

#include "flux_rt.h"
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>

/* ── POSIX implementation (macOS + Linux) ──────────────────────────── */
#if !defined(_WIN32) && !defined(_MSC_VER)

#include <pthread.h>
#include <stdatomic.h>

#define FLUX_TASK_TABLE_MAX 1024

typedef enum {
    TASK_PENDING   = 0,
    TASK_RUNNING   = 1,
    TASK_DONE      = 2,
    TASK_CANCELLED = 3,
} TaskStatus;

typedef struct {
    int64_t          task_id;        /* 0 = slot unused; real ids start at 1 */
    int64_t          closure;        /* rc-bumped NaN-boxed FluxClosure       */
    int64_t          result;         /* written by worker, read by join       */
    pthread_t        thread;
    pthread_mutex_t  mutex;
    pthread_cond_t   finished;
    _Atomic(int32_t) cancelled_flag; /* 0 = live, 1 = cancel requested       */
    TaskStatus       status;         /* protected by per-slot mutex           */
} FluxTaskEntry;

static FluxTaskEntry    task_table[FLUX_TASK_TABLE_MAX];
static pthread_mutex_t  task_table_mutex = PTHREAD_MUTEX_INITIALIZER;
static _Atomic(int64_t) next_task_id     = 1;
static pthread_once_t   task_table_once  = PTHREAD_ONCE_INIT;

static void task_table_do_init(void) {
    for (int i = 0; i < FLUX_TASK_TABLE_MAX; i++) {
        task_table[i].task_id = 0;
        pthread_mutex_init(&task_table[i].mutex, NULL);
        pthread_cond_init(&task_table[i].finished, NULL);
    }
}

static void task_table_init_once(void) {
    pthread_once(&task_table_once, task_table_do_init);
}

static void *flux_task_worker(void *arg) {
    FluxTaskEntry *e = (FluxTaskEntry *)arg;

    /* Bypass bump arena: worker threads allocate via malloc only. */
    flux_worker_thread = 1;

    if (atomic_load_explicit(&e->cancelled_flag, memory_order_acquire)) {
        /* Cancelled before the worker picked up the task — skip body. */
        pthread_mutex_lock(&e->mutex);
        e->status = TASK_CANCELLED;
        e->result = FLUX_NONE;
        pthread_cond_broadcast(&e->finished);
        pthread_mutex_unlock(&e->mutex);
        flux_drop(e->closure);
        return NULL;
    }

    pthread_mutex_lock(&e->mutex);
    e->status = TASK_RUNNING;
    pthread_mutex_unlock(&e->mutex);

    /* Run the Flux closure.  flux_call_closure_c is the ccc trampoline
     * emitted by the LLVM codegen; NULL args = zero-argument call. */
    int64_t result = flux_call_closure_c(e->closure, NULL, 0);
    flux_drop(e->closure);

    pthread_mutex_lock(&e->mutex);
    e->result = result;
    e->status = TASK_DONE;
    pthread_cond_broadcast(&e->finished);
    pthread_mutex_unlock(&e->mutex);
    return NULL;
}

int64_t flux_task_spawn(int64_t closure) {
    task_table_init_once();

    /* Promote to MT refcount mode then take an ownership reference. */
    flux_rc_promote(closure);
    flux_dup(closure);

    pthread_mutex_lock(&task_table_mutex);
    FluxTaskEntry *e = NULL;
    for (int i = 0; i < FLUX_TASK_TABLE_MAX; i++) {
        if (task_table[i].task_id == 0) { e = &task_table[i]; break; }
    }
    if (!e) {
        pthread_mutex_unlock(&task_table_mutex);
        flux_drop(closure);
        fprintf(stderr,
            "flux: flux_task_spawn: task table full (max %d concurrent tasks)\n",
            FLUX_TASK_TABLE_MAX);
        abort();
    }

    int64_t id = atomic_fetch_add_explicit(&next_task_id, 1,
                                           memory_order_relaxed);
    e->task_id = id;
    e->closure = closure;
    e->result  = FLUX_NONE;
    e->status  = TASK_PENDING;
    atomic_store_explicit(&e->cancelled_flag, 0, memory_order_relaxed);

    pthread_create(&e->thread, NULL, flux_task_worker, e);
    pthread_mutex_unlock(&task_table_mutex);

    return flux_tag_int(id);
}

int64_t flux_task_blocking_join(int64_t task) {
    task_table_init_once();
    int64_t id = flux_untag_int(task);

    pthread_mutex_lock(&task_table_mutex);
    FluxTaskEntry *e = NULL;
    for (int i = 0; i < FLUX_TASK_TABLE_MAX; i++) {
        if (task_table[i].task_id == id) { e = &task_table[i]; break; }
    }
    pthread_mutex_unlock(&task_table_mutex);

    if (!e) {
        fprintf(stderr,
            "flux: flux_task_blocking_join: unknown task id %" PRId64 "\n", id);
        abort();
    }

    pthread_mutex_lock(&e->mutex);
    while (e->status != TASK_DONE && e->status != TASK_CANCELLED) {
        pthread_cond_wait(&e->finished, &e->mutex);
    }
    TaskStatus s = e->status;
    int64_t result = e->result;
    pthread_mutex_unlock(&e->mutex);

    pthread_join(e->thread, NULL);

    /* Free the slot for reuse. */
    pthread_mutex_lock(&task_table_mutex);
    e->task_id = 0;
    pthread_mutex_unlock(&task_table_mutex);

    if (s == TASK_CANCELLED) {
        /* Raise a Flux panic so assert_throws / try blocks see it. */
        flux_panic(flux_string_new("TaskCancelled", 13));
        return FLUX_NONE; /* unreachable */
    }
    return result;
}

int64_t flux_task_cancel(int64_t task) {
    task_table_init_once();
    int64_t id = flux_untag_int(task);

    pthread_mutex_lock(&task_table_mutex);
    FluxTaskEntry *e = NULL;
    for (int i = 0; i < FLUX_TASK_TABLE_MAX; i++) {
        if (task_table[i].task_id == id) { e = &task_table[i]; break; }
    }
    pthread_mutex_unlock(&task_table_mutex);

    if (e) {
        /* Best-effort: running tasks complete normally (Phase 1a). */
        atomic_store_explicit(&e->cancelled_flag, 1, memory_order_release);
    }
    /* Slot not found → already joined; cancel is a no-op. */
    return FLUX_NONE;
}

/* ── Windows fallback ──────────────────────────────────────────────── */
#else

static void flux_task_unimplemented(const char *which) {
    fprintf(stderr,
        "flux: %s is not yet implemented on the Windows native backend\n"
        "      (proposal 0174 D5-b Windows follow-up). "
        "Use the VM backend (drop --native) until then.\n",
        which);
    abort();
}

int64_t flux_task_spawn(int64_t closure) {
    (void)closure;
    flux_task_unimplemented("flux_task_spawn");
    return FLUX_NONE;
}

int64_t flux_task_blocking_join(int64_t task) {
    (void)task;
    flux_task_unimplemented("flux_task_blocking_join");
    return FLUX_NONE;
}

int64_t flux_task_cancel(int64_t task) {
    (void)task;
    flux_task_unimplemented("flux_task_cancel");
    return FLUX_NONE;
}

#endif /* !_WIN32 */
