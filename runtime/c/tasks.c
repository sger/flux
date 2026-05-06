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
#include <errno.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#if defined(_WIN32) || defined(_MSC_VER)
#include <windows.h>
#endif

/* File-scope atomic scope ID counter — shared by both POSIX and Win32 paths.
 * Declared here (before the platform split) so it is always available.     */
#include <stdatomic.h>
static _Atomic(int64_t) next_scope_id = 1;

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

/* ── Windows implementation (Win32) ───────────────────────────────────
 * Mirrors the POSIX path 1:1 using CreateThread / CRITICAL_SECTION /
 * CONDITION_VARIABLE so `flux --test --native` reaches the same Phase
 * 1a Task semantics on Windows as on macOS / Linux.  Cancel semantics,
 * MT-RC promotion, and slot lifecycle are identical to the POSIX path. */
#else

#include <process.h>      /* _beginthreadex / _endthreadex */
#include <stdatomic.h>

#define FLUX_TASK_TABLE_MAX 1024

typedef enum {
    TASK_PENDING   = 0,
    TASK_RUNNING   = 1,
    TASK_DONE      = 2,
    TASK_CANCELLED = 3,
} TaskStatus;

typedef struct {
    int64_t            task_id;        /* 0 = slot unused; real ids start at 1 */
    int64_t            closure;        /* rc-bumped NaN-boxed FluxClosure       */
    int64_t            result;         /* written by worker, read by join       */
    HANDLE             thread;
    CRITICAL_SECTION   mutex;
    CONDITION_VARIABLE finished;
    _Atomic(int32_t)   cancelled_flag; /* 0 = live, 1 = cancel requested       */
    TaskStatus         status;         /* protected by per-slot mutex           */
} FluxTaskEntry;

static FluxTaskEntry    task_table[FLUX_TASK_TABLE_MAX];
static CRITICAL_SECTION task_table_mutex;
static _Atomic(int64_t) next_task_id = 1;
static INIT_ONCE        task_table_once = INIT_ONCE_STATIC_INIT;

static BOOL CALLBACK task_table_do_init(PINIT_ONCE once, PVOID param, PVOID *ctx) {
    (void)once; (void)param; (void)ctx;
    InitializeCriticalSection(&task_table_mutex);
    for (int i = 0; i < FLUX_TASK_TABLE_MAX; i++) {
        task_table[i].task_id = 0;
        InitializeCriticalSection(&task_table[i].mutex);
        InitializeConditionVariable(&task_table[i].finished);
    }
    return TRUE;
}

static void task_table_init_once(void) {
    InitOnceExecuteOnce(&task_table_once, task_table_do_init, NULL, NULL);
}

/* `_beginthreadex` requires `unsigned __stdcall (void *)`, so use that
 * signature directly rather than the `DWORD WINAPI (LPVOID)` variant
 * `CreateThread` wants.  Both layouts are ABI-compatible on x64, but using
 * `_beginthreadex` is required for any thread that calls C runtime
 * functions (malloc, fprintf, etc.) — `CreateThread` skips per-thread CRT
 * setup and can corrupt the heap on exit. */
static unsigned __stdcall flux_task_worker(void *arg) {
    FluxTaskEntry *e = (FluxTaskEntry *)arg;

    /* Bypass bump arena: worker threads allocate via malloc only. */
    flux_worker_thread = 1;

    if (atomic_load_explicit(&e->cancelled_flag, memory_order_acquire)) {
        /* Cancelled before the worker picked up the task — skip body. */
        EnterCriticalSection(&e->mutex);
        e->status = TASK_CANCELLED;
        e->result = FLUX_NONE;
        WakeAllConditionVariable(&e->finished);
        LeaveCriticalSection(&e->mutex);
        flux_drop(e->closure);
        return 0;
    }

    EnterCriticalSection(&e->mutex);
    e->status = TASK_RUNNING;
    LeaveCriticalSection(&e->mutex);

    int64_t result = flux_call_closure_c(e->closure, NULL, 0);
    flux_drop(e->closure);

    EnterCriticalSection(&e->mutex);
    e->result = result;
    e->status = TASK_DONE;
    WakeAllConditionVariable(&e->finished);
    LeaveCriticalSection(&e->mutex);
    return 0;
}

int64_t flux_task_spawn(int64_t closure) {
    task_table_init_once();

    flux_rc_promote(closure);
    flux_dup(closure);

    EnterCriticalSection(&task_table_mutex);
    FluxTaskEntry *e = NULL;
    for (int i = 0; i < FLUX_TASK_TABLE_MAX; i++) {
        if (task_table[i].task_id == 0) { e = &task_table[i]; break; }
    }
    if (!e) {
        LeaveCriticalSection(&task_table_mutex);
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

    /* _beginthreadex returns a uintptr_t cast of the HANDLE, or 0 on failure.
     * Using it (instead of CreateThread) gives the worker thread proper CRT
     * per-thread initialisation — required because the body calls malloc,
     * fprintf, etc. through the Flux runtime. */
    e->thread = (HANDLE)_beginthreadex(NULL, 0, flux_task_worker, e, 0, NULL);
    if (e->thread == NULL) {
        e->task_id = 0;
        LeaveCriticalSection(&task_table_mutex);
        flux_drop(closure);
        fprintf(stderr,
            "flux: flux_task_spawn: _beginthreadex failed (errno=%d)\n",
            errno);
        abort();
    }
    LeaveCriticalSection(&task_table_mutex);

    return flux_tag_int(id);
}

int64_t flux_task_blocking_join(int64_t task) {
    task_table_init_once();
    int64_t id = flux_untag_int(task);

    EnterCriticalSection(&task_table_mutex);
    FluxTaskEntry *e = NULL;
    for (int i = 0; i < FLUX_TASK_TABLE_MAX; i++) {
        if (task_table[i].task_id == id) { e = &task_table[i]; break; }
    }
    LeaveCriticalSection(&task_table_mutex);

    if (!e) {
        fprintf(stderr,
            "flux: flux_task_blocking_join: unknown task id %" PRId64 "\n", id);
        abort();
    }

    EnterCriticalSection(&e->mutex);
    while (e->status != TASK_DONE && e->status != TASK_CANCELLED) {
        SleepConditionVariableCS(&e->finished, &e->mutex, INFINITE);
    }
    TaskStatus s = e->status;
    int64_t result = e->result;
    LeaveCriticalSection(&e->mutex);

    WaitForSingleObject(e->thread, INFINITE);
    CloseHandle(e->thread);
    e->thread = NULL;

    EnterCriticalSection(&task_table_mutex);
    e->task_id = 0;
    LeaveCriticalSection(&task_table_mutex);

    if (s == TASK_CANCELLED) {
        flux_panic(flux_string_new("TaskCancelled", 13));
        return FLUX_NONE; /* unreachable */
    }
    return result;
}

int64_t flux_task_cancel(int64_t task) {
    task_table_init_once();
    int64_t id = flux_untag_int(task);

    EnterCriticalSection(&task_table_mutex);
    FluxTaskEntry *e = NULL;
    for (int i = 0; i < FLUX_TASK_TABLE_MAX; i++) {
        if (task_table[i].task_id == id) { e = &task_table[i]; break; }
    }
    LeaveCriticalSection(&task_table_mutex);

    if (e) {
        atomic_store_explicit(&e->cancelled_flag, 1, memory_order_release);
    }
    return FLUX_NONE;
}

#endif /* !_WIN32 */

/* ── Fiber primops (proposal 0174 Phase 1b) ─────────────────────────────── *
 * These C entry points are called by LLVM-compiled code when user code       *
 * invokes fiber operations. The scheduler bridge lands in Slice 1b-vi;       *
 * until then every call aborts with a clear message.                         */

static void flux_fiber_unimplemented(const char *which) {
    fprintf(stderr,
        "flux: %s is not yet wired (proposal 0174 Phase 1b-vi).\n"
        "      Use blocking_join / Task.spawn instead of fiber-suspending APIs.\n",
        which);
    abort();
}

int64_t flux_fiber_suspend(int64_t setup_closure) {
    (void)setup_closure;
    flux_fiber_unimplemented("flux_fiber_suspend");
    return FLUX_NONE;
}

int64_t flux_fiber_fork(int64_t body_closure) {
    (void)body_closure;
    flux_fiber_unimplemented("flux_fiber_fork");
    return FLUX_NONE;
}

int64_t flux_fiber_get_context(void) {
    flux_fiber_unimplemented("flux_fiber_get_context");
    return FLUX_NONE;
}

int64_t flux_fiber_fail(int64_t error_value) {
    (void)error_value;
    flux_fiber_unimplemented("flux_fiber_fail");
    return FLUX_NONE;
}

int64_t flux_task_await(int64_t task) {
    (void)task;
    flux_fiber_unimplemented("flux_task_await");
    return FLUX_NONE;
}

/* ── Entry-point / scheduling shims (proposal 0174 Phase 1b) ───────────── */

int64_t flux_fiber_run_async(int64_t closure) {
    return flux_async_run_root(closure);
}

int64_t flux_fiber_yield_now(void) {
    /* No-op on the native sequential path. */
    return FLUX_NONE;
}

int64_t flux_fiber_sleep(int64_t ms) {
    int64_t raw_ms = flux_untag_int(ms);
    uint64_t request_id = flux_async_timer_start(raw_ms);
    if (request_id == 0) {
        fprintf(stderr, "flux_fiber_sleep: async timer registration failed\n");
        abort();
    }
    return flux_async_suspend(flux_tag_int((int64_t)request_id), FLUX_NONE);
}

/* ── Native async helper callbacks used by the Rust scheduler ───────────── */

int64_t flux_async_call0(int64_t closure) {
    return flux_call_closure_c(closure, NULL, 0);
}

int64_t flux_async_resume1(int64_t continuation, int64_t value) {
    int64_t args[1] = { value };
    return flux_call_closure_c(continuation, args, 1);
}

void flux_async_retain(int64_t value) {
    flux_dup(value);
}

void flux_async_release(int64_t value) {
    flux_drop(value);
}

int64_t flux_async_make_tuple2(int64_t left, int64_t right) {
    void *ptr = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_TUPLE);
    *(uint32_t *)((char *)ptr + 4) = 2;
    int64_t *elems = (int64_t *)((char *)ptr + 8);
    elems[0] = left;
    elems[1] = right;
    return (int64_t)ptr;
}

int64_t flux_async_wrap_some(int64_t value) {
    return flux_wrap_some(value);
}

/* ── Fiber combinators (proposal 0174 Phase 1b-vi-d) ───────────────────── */

int64_t flux_fiber_both(int64_t f, int64_t g) {
    uint64_t request_id = flux_async_fiber_both(f, g);
    return flux_async_suspend_request(request_id);
}

int64_t flux_fiber_race(int64_t f, int64_t g) {
    uint64_t request_id = flux_async_fiber_race(f, g);
    return flux_async_suspend_request(request_id);
}

int64_t flux_fiber_timeout(int64_t ms_val, int64_t f) {
    int64_t ms = flux_untag_int(ms_val);
    uint64_t request_id = flux_async_fiber_timeout(ms, f);
    return flux_async_suspend_request(request_id);
}

int64_t flux_fiber_new_scope(int32_t scope_ctor_tag) {
    /* Allocate Scope(id) ADT: payload = 8 (ctor_tag+field_count) + 8 = 16;
     * scan_fsize = 1 (one owned i64 field).                                 */
    int64_t id = atomic_fetch_add_explicit(&next_scope_id, 1,
                                           memory_order_relaxed);
    void *mem = flux_gc_alloc_header(8 + 8, 1, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = scope_ctor_tag;
    hdr[1] = 1; /* field_count */
    int64_t *fields = (int64_t *)((char *)mem + 8);
    fields[0] = flux_tag_int(id);
    return flux_tag_ptr(mem);
}

int64_t flux_fiber_fork_scoped(int64_t s, int64_t f) {
    /* Sequential: run f() inline; scope registration is a no-op. */
    (void)s;
    flux_call_closure_c(f, NULL, 0);
    return FLUX_NONE;
}

int64_t flux_fiber_cancel_scope(int64_t s) {
    /* Sequential: nothing is registered under the scope, nothing to cancel. */
    (void)s;
    return FLUX_NONE;
}
