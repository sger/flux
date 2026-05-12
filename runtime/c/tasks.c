/*
 * tasks.c — Native Task<a> runtime for proposal 0174 D5-b/c.
 *
 * Implements flux_task_spawn / flux_task_blocking_join / flux_task_cancel /
 * flux_task_await using POSIX or Win32 threads. Each spawn creates a real OS
 * thread; blocking_join waits on a per-task condvar; Task.await suspends the
 * current native fiber and resumes through the async scheduler.
 *
 * Phase 1a cancel semantics (matches the VM backend):
 *   - cancelled before join    → join raises TaskCancelled
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
/* ── POSIX implementation (macOS + Linux) ──────────────────────────── */
#if !defined(_WIN32) && !defined(_MSC_VER)

#include <pthread.h>
#include <stdatomic.h>

typedef enum {
    TASK_PENDING   = 0,
    TASK_RUNNING   = 1,
    TASK_DONE      = 2,
    TASK_CANCELLED = 3,
} TaskStatus;

typedef struct FluxTaskEntry {
    int64_t          task_id;        /* real ids start at 1 */
    int64_t          closure;        /* rc-bumped NaN-boxed FluxClosure       */
    int64_t          result;         /* written by worker, read by join       */
    pthread_t        thread;
    uint64_t         await_request;  /* 0 = no fiber-suspending awaiter       */
    pthread_mutex_t  mutex;
    pthread_cond_t   finished;
    _Atomic(int32_t) cancelled_flag; /* 0 = live, 1 = cancel requested       */
    TaskStatus       status;         /* protected by per-slot mutex           */
    struct FluxTaskEntry *next;
} FluxTaskEntry;

static FluxTaskEntry   *task_registry = NULL;
static pthread_mutex_t  task_table_mutex = PTHREAD_MUTEX_INITIALIZER;
static _Atomic(int64_t) next_task_id     = 1;
static _Atomic(size_t)  task_live_count  = 0;
static _Atomic(size_t)  task_high_water  = 0;

static void task_table_init_once(void) {
}

static FluxTaskEntry *flux_task_entry_new(void) {
    FluxTaskEntry *e = (FluxTaskEntry *)calloc(1, sizeof(FluxTaskEntry));
    if (!e) return NULL;
    pthread_mutex_init(&e->mutex, NULL);
    pthread_cond_init(&e->finished, NULL);
    return e;
}

static void flux_task_entry_destroy(FluxTaskEntry *e) {
    if (!e) return;
    pthread_cond_destroy(&e->finished);
    pthread_mutex_destroy(&e->mutex);
    free(e);
}

static void flux_task_register_entry(FluxTaskEntry *e) {
    pthread_mutex_lock(&task_table_mutex);
    e->next = task_registry;
    task_registry = e;
    size_t live = atomic_fetch_add_explicit(&task_live_count, 1, memory_order_relaxed) + 1;
    if (live > atomic_load_explicit(&task_high_water, memory_order_relaxed)) {
        atomic_store_explicit(&task_high_water, live, memory_order_relaxed);
    }
    pthread_mutex_unlock(&task_table_mutex);
}

static FluxTaskEntry *flux_task_lookup(int64_t id) {
    FluxTaskEntry *e = NULL;
    pthread_mutex_lock(&task_table_mutex);
    for (FluxTaskEntry *cur = task_registry; cur != NULL; cur = cur->next) {
        if (cur->task_id == id) { e = cur; break; }
    }
    pthread_mutex_unlock(&task_table_mutex);
    return e;
}

static void flux_task_free_slot(FluxTaskEntry *e) {
    pthread_mutex_lock(&task_table_mutex);
    FluxTaskEntry **cur = &task_registry;
    while (*cur != NULL) {
        if (*cur == e) {
            *cur = e->next;
            if (atomic_load_explicit(&task_live_count, memory_order_relaxed) > 0) {
                atomic_fetch_sub_explicit(&task_live_count, 1, memory_order_relaxed);
            }
            break;
        }
        cur = &(*cur)->next;
    }
    e->task_id = 0;
    e->next = NULL;
    pthread_mutex_unlock(&task_table_mutex);
    flux_task_entry_destroy(e);
}

static void flux_task_publish_async_completion(FluxTaskEntry *e) {
    uint64_t await_request = 0;
    int64_t completion = FLUX_NONE;

    pthread_mutex_lock(&e->mutex);
    await_request = e->await_request;
    if (await_request != 0) {
        if (e->status == TASK_DONE) {
            completion = flux_wrap_some(e->result);
            e->result = FLUX_NONE;
        }
        e->await_request = 0;
    }
    pthread_cond_broadcast(&e->finished);
    pthread_mutex_unlock(&e->mutex);

    if (await_request != 0) {
        flux_task_free_slot(e);
        flux_async_task_complete(await_request, completion);
    }
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
        pthread_mutex_unlock(&e->mutex);
        flux_task_publish_async_completion(e);
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
    if (atomic_load_explicit(&e->cancelled_flag, memory_order_acquire)) {
        flux_drop(result);
        e->result = FLUX_NONE;
        e->status = TASK_CANCELLED;
    } else {
        e->result = result;
        e->status = TASK_DONE;
    }
    pthread_mutex_unlock(&e->mutex);
    flux_task_publish_async_completion(e);
    return NULL;
}

static int64_t flux_task_spawn_inner(int64_t closure, int move_owned, const char *which) {
    task_table_init_once();

    if (move_owned) {
        if (!flux_rc_is_unique(closure)) {
            flux_rc_promote(closure);
            flux_dup(closure);
        }
    } else {
        /* Promote to MT refcount mode then take an ownership reference. */
        flux_rc_promote(closure);
        flux_dup(closure);
    }

    FluxTaskEntry *e = flux_task_entry_new();
    if (!e) {
        flux_drop(closure);
        fprintf(stderr,
            "flux: %s: failed to allocate task entry "
            "(live=%zu high_water=%zu)\n",
            which,
            atomic_load_explicit(&task_live_count, memory_order_relaxed),
            atomic_load_explicit(&task_high_water, memory_order_relaxed));
        abort();
    }

    int64_t id = atomic_fetch_add_explicit(&next_task_id, 1,
                                           memory_order_relaxed);
    e->task_id = id;
    e->closure = closure;
    e->result  = FLUX_NONE;
    e->status  = TASK_PENDING;
    e->await_request = 0;
    atomic_store_explicit(&e->cancelled_flag, 0, memory_order_relaxed);
    flux_task_register_entry(e);

    int err = pthread_create(&e->thread, NULL, flux_task_worker, e);
    if (err != 0) {
        flux_task_free_slot(e);
        flux_drop(closure);
        fprintf(stderr,
            "flux: %s: pthread_create failed (errno=%d, "
            "live=%zu high_water=%zu)\n",
            which,
            err,
            atomic_load_explicit(&task_live_count, memory_order_relaxed),
            atomic_load_explicit(&task_high_water, memory_order_relaxed));
        abort();
    }

    /* Register with the active run's root-task safety net so an unawaited
     * task is cancelled at run_async teardown instead of leaking the OS
     * thread until process exit. No-op when no run is active. */
    flux_async_register_root_task(id);

    return flux_tag_int(id);
}

int64_t flux_task_spawn(int64_t closure) {
    return flux_task_spawn_inner(closure, 0, "flux_task_spawn");
}

int64_t flux_task_spawn_move(int64_t closure) {
    return flux_task_spawn_inner(closure, 1, "flux_task_spawn_move");
}

int64_t flux_task_blocking_join(int64_t task) {
    task_table_init_once();
    int64_t id = flux_untag_int(task);

    /* User has taken ownership of the handle — drop the root-task safety net. */
    flux_async_deregister_root_task(id);

    FluxTaskEntry *e = flux_task_lookup(id);

    if (!e) {
        fprintf(stderr,
            "flux: flux_task_blocking_join: unknown task id %" PRId64 "\n", id);
        abort();
    }

    pthread_mutex_lock(&e->mutex);
    if (e->await_request != 0) {
        pthread_mutex_unlock(&e->mutex);
        fprintf(stderr,
            "flux: flux_task_blocking_join: task %" PRId64 " already awaited\n", id);
        abort();
    }
    while (e->status != TASK_DONE && e->status != TASK_CANCELLED) {
        pthread_cond_wait(&e->finished, &e->mutex);
    }
    TaskStatus s = e->status;
    int64_t result = e->result;
    e->result = FLUX_NONE;
    pthread_mutex_unlock(&e->mutex);

    pthread_join(e->thread, NULL);

    /* Free the slot for reuse. */
    flux_task_free_slot(e);

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

    /* User has taken ownership of the handle — drop the root-task safety net. */
    flux_async_deregister_root_task(id);

    FluxTaskEntry *e = flux_task_lookup(id);

    if (e) {
        atomic_store_explicit(&e->cancelled_flag, 1, memory_order_release);
        pthread_mutex_lock(&e->mutex);
        if (e->status == TASK_DONE) {
            flux_drop(e->result);
            e->result = FLUX_NONE;
            e->status = TASK_CANCELLED;
            pthread_cond_broadcast(&e->finished);
        }
        pthread_mutex_unlock(&e->mutex);
    }
    /* Slot not found → already joined; cancel is a no-op. */
    return FLUX_NONE;
}

int64_t flux_task_await(int64_t task) {
    task_table_init_once();
    int64_t id = flux_untag_int(task);

    /* User has taken ownership of the handle — drop the root-task safety net. */
    flux_async_deregister_root_task(id);

    FluxTaskEntry *e = flux_task_lookup(id);

    if (!e) {
        fprintf(stderr,
            "flux: flux_task_await: unknown task id %" PRId64 "\n", id);
        abort();
    }

    pthread_mutex_lock(&e->mutex);
    if (e->await_request != 0) {
        pthread_mutex_unlock(&e->mutex);
        fprintf(stderr,
            "flux: flux_task_await: task %" PRId64 " already awaited\n", id);
        abort();
    }
    if (e->status == TASK_DONE || e->status == TASK_CANCELLED) {
        TaskStatus s = e->status;
        int64_t result = e->result;
        e->result = FLUX_NONE;
        pthread_mutex_unlock(&e->mutex);

        uint64_t request_id = flux_async_task_await_request();
        if (request_id == 0) {
            if (s == TASK_DONE) {
                flux_drop(result);
            }
            fprintf(stderr,
                "flux: flux_task_await: called outside Async.run_async\n");
            abort();
        }
        int64_t completion = s == TASK_DONE ? flux_wrap_some(result) : FLUX_NONE;
        pthread_join(e->thread, NULL);
        flux_task_free_slot(e);
        flux_async_task_complete(request_id, completion);
        return flux_async_suspend_request(request_id);
    }

    uint64_t request_id = flux_async_task_await_request();
    if (request_id == 0) {
        pthread_mutex_unlock(&e->mutex);
        fprintf(stderr,
            "flux: flux_task_await: called outside Async.run_async\n");
        abort();
    }

    e->await_request = request_id;
    pthread_detach(e->thread);
    pthread_mutex_unlock(&e->mutex);
    return flux_async_suspend_request(request_id);
}

/* ── Windows implementation (Win32) ───────────────────────────────────
 * Mirrors the POSIX path 1:1 using CreateThread / CRITICAL_SECTION /
 * CONDITION_VARIABLE so `flux --test --native` reaches the same Phase
 * Task semantics on Windows as on macOS / Linux.  Cancel semantics,
 * MT-RC promotion, and slot lifecycle are identical to the POSIX path. */
#else

#include <process.h>      /* _beginthreadex / _endthreadex */
#include <stdatomic.h>

typedef enum {
    TASK_PENDING   = 0,
    TASK_RUNNING   = 1,
    TASK_DONE      = 2,
    TASK_CANCELLED = 3,
} TaskStatus;

typedef struct FluxTaskEntry {
    int64_t            task_id;        /* real ids start at 1 */
    int64_t            closure;        /* rc-bumped NaN-boxed FluxClosure       */
    int64_t            result;         /* written by worker, read by join       */
    HANDLE             thread;
    uint64_t           await_request;  /* 0 = no fiber-suspending awaiter       */
    CRITICAL_SECTION   mutex;
    CONDITION_VARIABLE finished;
    _Atomic(int32_t)   cancelled_flag; /* 0 = live, 1 = cancel requested       */
    TaskStatus         status;         /* protected by per-slot mutex           */
    struct FluxTaskEntry *next;
} FluxTaskEntry;

static FluxTaskEntry   *task_registry = NULL;
static CRITICAL_SECTION task_table_mutex;
static _Atomic(int64_t) next_task_id = 1;
static _Atomic(size_t)  task_live_count = 0;
static _Atomic(size_t)  task_high_water = 0;
static INIT_ONCE        task_table_once = INIT_ONCE_STATIC_INIT;

static BOOL CALLBACK task_table_do_init(PINIT_ONCE once, PVOID param, PVOID *ctx) {
    (void)once; (void)param; (void)ctx;
    InitializeCriticalSection(&task_table_mutex);
    return TRUE;
}

static void task_table_init_once(void) {
    InitOnceExecuteOnce(&task_table_once, task_table_do_init, NULL, NULL);
}

static FluxTaskEntry *flux_task_entry_new(void) {
    FluxTaskEntry *e = (FluxTaskEntry *)calloc(1, sizeof(FluxTaskEntry));
    if (!e) return NULL;
    InitializeCriticalSection(&e->mutex);
    InitializeConditionVariable(&e->finished);
    return e;
}

static void flux_task_entry_destroy(FluxTaskEntry *e) {
    if (!e) return;
    DeleteCriticalSection(&e->mutex);
    free(e);
}

static void flux_task_register_entry(FluxTaskEntry *e) {
    EnterCriticalSection(&task_table_mutex);
    e->next = task_registry;
    task_registry = e;
    size_t live = atomic_fetch_add_explicit(&task_live_count, 1, memory_order_relaxed) + 1;
    if (live > atomic_load_explicit(&task_high_water, memory_order_relaxed)) {
        atomic_store_explicit(&task_high_water, live, memory_order_relaxed);
    }
    LeaveCriticalSection(&task_table_mutex);
}

static FluxTaskEntry *flux_task_lookup(int64_t id) {
    FluxTaskEntry *e = NULL;
    EnterCriticalSection(&task_table_mutex);
    for (FluxTaskEntry *cur = task_registry; cur != NULL; cur = cur->next) {
        if (cur->task_id == id) { e = cur; break; }
    }
    LeaveCriticalSection(&task_table_mutex);
    return e;
}

static void flux_task_free_slot(FluxTaskEntry *e) {
    EnterCriticalSection(&task_table_mutex);
    FluxTaskEntry **cur = &task_registry;
    while (*cur != NULL) {
        if (*cur == e) {
            *cur = e->next;
            if (atomic_load_explicit(&task_live_count, memory_order_relaxed) > 0) {
                atomic_fetch_sub_explicit(&task_live_count, 1, memory_order_relaxed);
            }
            break;
        }
        cur = &(*cur)->next;
    }
    e->task_id = 0;
    e->next = NULL;
    LeaveCriticalSection(&task_table_mutex);
    flux_task_entry_destroy(e);
}

static void flux_task_publish_async_completion(FluxTaskEntry *e) {
    uint64_t await_request = 0;
    int64_t completion = FLUX_NONE;

    EnterCriticalSection(&e->mutex);
    await_request = e->await_request;
    if (await_request != 0) {
        if (e->status == TASK_DONE) {
            completion = flux_wrap_some(e->result);
            e->result = FLUX_NONE;
        }
        e->await_request = 0;
    }
    WakeAllConditionVariable(&e->finished);
    LeaveCriticalSection(&e->mutex);

    if (await_request != 0) {
        flux_task_free_slot(e);
        flux_async_task_complete(await_request, completion);
    }
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
        LeaveCriticalSection(&e->mutex);
        flux_task_publish_async_completion(e);
        flux_drop(e->closure);
        return 0;
    }

    EnterCriticalSection(&e->mutex);
    e->status = TASK_RUNNING;
    LeaveCriticalSection(&e->mutex);

    int64_t result = flux_call_closure_c(e->closure, NULL, 0);
    flux_drop(e->closure);

    EnterCriticalSection(&e->mutex);
    if (atomic_load_explicit(&e->cancelled_flag, memory_order_acquire)) {
        flux_drop(result);
        e->result = FLUX_NONE;
        e->status = TASK_CANCELLED;
    } else {
        e->result = result;
        e->status = TASK_DONE;
    }
    LeaveCriticalSection(&e->mutex);
    flux_task_publish_async_completion(e);
    return 0;
}

static int64_t flux_task_spawn_inner(int64_t closure, int move_owned, const char *which) {
    task_table_init_once();

    if (move_owned) {
        if (!flux_rc_is_unique(closure)) {
            flux_rc_promote(closure);
            flux_dup(closure);
        }
    } else {
        flux_rc_promote(closure);
        flux_dup(closure);
    }

    FluxTaskEntry *e = flux_task_entry_new();
    if (!e) {
        flux_drop(closure);
        fprintf(stderr,
            "flux: %s: failed to allocate task entry "
            "(live=%zu high_water=%zu)\n",
            which,
            atomic_load_explicit(&task_live_count, memory_order_relaxed),
            atomic_load_explicit(&task_high_water, memory_order_relaxed));
        abort();
    }

    int64_t id = atomic_fetch_add_explicit(&next_task_id, 1,
                                           memory_order_relaxed);
    e->task_id = id;
    e->closure = closure;
    e->result  = FLUX_NONE;
    e->status  = TASK_PENDING;
    e->await_request = 0;
    atomic_store_explicit(&e->cancelled_flag, 0, memory_order_relaxed);
    flux_task_register_entry(e);

    /* _beginthreadex returns a uintptr_t cast of the HANDLE, or 0 on failure.
     * Using it (instead of CreateThread) gives the worker thread proper CRT
     * per-thread initialisation — required because the body calls malloc,
     * fprintf, etc. through the Flux runtime. */
    e->thread = (HANDLE)_beginthreadex(NULL, 0, flux_task_worker, e, 0, NULL);
    if (e->thread == NULL) {
        flux_task_free_slot(e);
        flux_drop(closure);
        fprintf(stderr,
            "flux: %s: _beginthreadex failed (errno=%d, "
            "live=%zu high_water=%zu)\n",
            which,
            errno,
            atomic_load_explicit(&task_live_count, memory_order_relaxed),
            atomic_load_explicit(&task_high_water, memory_order_relaxed));
        abort();
    }

    /* Register with the active run's root-task safety net so an unawaited
     * task is cancelled at run_async teardown instead of leaking the OS
     * thread until process exit. No-op when no run is active. */
    flux_async_register_root_task(id);

    return flux_tag_int(id);
}

int64_t flux_task_spawn(int64_t closure) {
    return flux_task_spawn_inner(closure, 0, "flux_task_spawn");
}

int64_t flux_task_spawn_move(int64_t closure) {
    return flux_task_spawn_inner(closure, 1, "flux_task_spawn_move");
}

int64_t flux_task_blocking_join(int64_t task) {
    task_table_init_once();
    int64_t id = flux_untag_int(task);

    /* User has taken ownership of the handle — drop the root-task safety net. */
    flux_async_deregister_root_task(id);

    FluxTaskEntry *e = flux_task_lookup(id);

    if (!e) {
        fprintf(stderr,
            "flux: flux_task_blocking_join: unknown task id %" PRId64 "\n", id);
        abort();
    }

    EnterCriticalSection(&e->mutex);
    if (e->await_request != 0) {
        LeaveCriticalSection(&e->mutex);
        fprintf(stderr,
            "flux: flux_task_blocking_join: task %" PRId64 " already awaited\n", id);
        abort();
    }
    while (e->status != TASK_DONE && e->status != TASK_CANCELLED) {
        SleepConditionVariableCS(&e->finished, &e->mutex, INFINITE);
    }
    TaskStatus s = e->status;
    int64_t result = e->result;
    e->result = FLUX_NONE;
    LeaveCriticalSection(&e->mutex);

    WaitForSingleObject(e->thread, INFINITE);
    CloseHandle(e->thread);
    e->thread = NULL;

    flux_task_free_slot(e);

    if (s == TASK_CANCELLED) {
        flux_panic(flux_string_new("TaskCancelled", 13));
        return FLUX_NONE; /* unreachable */
    }
    return result;
}

int64_t flux_task_cancel(int64_t task) {
    task_table_init_once();
    int64_t id = flux_untag_int(task);

    /* User has taken ownership of the handle — drop the root-task safety net. */
    flux_async_deregister_root_task(id);

    FluxTaskEntry *e = flux_task_lookup(id);

    if (e) {
        atomic_store_explicit(&e->cancelled_flag, 1, memory_order_release);
        EnterCriticalSection(&e->mutex);
        if (e->status == TASK_DONE) {
            flux_drop(e->result);
            e->result = FLUX_NONE;
            e->status = TASK_CANCELLED;
            WakeAllConditionVariable(&e->finished);
        }
        LeaveCriticalSection(&e->mutex);
    }
    return FLUX_NONE;
}

int64_t flux_task_await(int64_t task) {
    task_table_init_once();
    int64_t id = flux_untag_int(task);

    /* User has taken ownership of the handle — drop the root-task safety net. */
    flux_async_deregister_root_task(id);

    FluxTaskEntry *e = flux_task_lookup(id);

    if (!e) {
        fprintf(stderr,
            "flux: flux_task_await: unknown task id %" PRId64 "\n", id);
        abort();
    }

    EnterCriticalSection(&e->mutex);
    if (e->await_request != 0) {
        LeaveCriticalSection(&e->mutex);
        fprintf(stderr,
            "flux: flux_task_await: task %" PRId64 " already awaited\n", id);
        abort();
    }
    if (e->status == TASK_DONE || e->status == TASK_CANCELLED) {
        TaskStatus s = e->status;
        int64_t result = e->result;
        e->result = FLUX_NONE;
        LeaveCriticalSection(&e->mutex);

        uint64_t request_id = flux_async_task_await_request();
        if (request_id == 0) {
            if (s == TASK_DONE) {
                flux_drop(result);
            }
            fprintf(stderr,
                "flux: flux_task_await: called outside Async.run_async\n");
            abort();
        }
        int64_t completion = s == TASK_DONE ? flux_wrap_some(result) : FLUX_NONE;
        WaitForSingleObject(e->thread, INFINITE);
        CloseHandle(e->thread);
        e->thread = NULL;
        flux_task_free_slot(e);
        flux_async_task_complete(request_id, completion);
        return flux_async_suspend_request(request_id);
    }

    uint64_t request_id = flux_async_task_await_request();
    if (request_id == 0) {
        LeaveCriticalSection(&e->mutex);
        fprintf(stderr,
            "flux: flux_task_await: called outside Async.run_async\n");
        abort();
    }

    e->await_request = request_id;
    CloseHandle(e->thread);
    e->thread = NULL;
    LeaveCriticalSection(&e->mutex);
    return flux_async_suspend_request(request_id);
}

#endif /* !_WIN32 */

/* ── Legacy fiber primop stubs (proposal 0174 Phase 1b) ─────────────────── *
 * These older unstructured entry points are still unsupported. The public    *
 * Flow.Async surface below uses the scheduler-backed structured shims.       */

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
    flux_panic(error_value);
    return FLUX_NONE;
}

/* ── Entry-point / scheduling shims (proposal 0174 Phase 1b) ───────────── */

static void flux_async_register_callbacks(void) {
    FluxAsyncCallbacks callbacks = {
        flux_async_call0,
        flux_async_resume1,
        flux_async_try_call0,
        flux_async_try_resume1,
        flux_async_retain,
        flux_async_release,
        flux_async_make_tuple2,
        flux_async_wrap_some,
        flux_async_make_adt0,
        flux_async_make_adt1,
        flux_async_suspend,
        flux_async_is_suspended,
        flux_async_current_request,
        flux_async_clear_suspend,
        flux_compose_conts,
        flux_effect_context_capture,
        flux_effect_context_restore,
        flux_effect_context_reset,
        flux_effect_context_release,
        flux_async_promote,
        flux_async_enter_worker_thread,
        flux_async_make_string,
        flux_task_spawn,
        flux_task_cancel,
        flux_async_register_root_task,
        flux_async_deregister_root_task,
    };
    if (flux_async_set_callbacks(&callbacks) != 0) {
        fprintf(stderr, "flux_async_set_callbacks failed\n");
        abort();
    }
}

int64_t flux_fiber_run_async(int64_t closure) {
    flux_async_register_callbacks();
    int64_t result = flux_async_run_root(closure);
    if (flux_async_last_run_failed() != 0) {
        flux_panic(result);
    }
    return result;
}

int64_t flux_fiber_yield_now(void) {
    /* Cooperative reschedule (proposal 0174 slice 2-vi): mint a synthetic
     * scheduler request and suspend on it. The Rust scheduler self-wakes the
     * fiber immediately, re-enqueueing it at the tail of its home worker's
     * run-queue so other ready fibers make progress first. Outside an active
     * run_async scheduler there is nothing to reschedule against, so
     * flux_async_yield_request returns 0 and this is a no-op. */
    uint64_t request_id = flux_async_yield_request();
    if (request_id == 0) {
        return FLUX_NONE;
    }
    return flux_async_suspend_request(request_id);
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

int32_t flux_async_try_call0(int64_t closure, int64_t *out) {
    return flux_try_call0_raw(closure, out);
}

int32_t flux_async_try_resume1(int64_t continuation, int64_t value, int64_t *out) {
    return flux_try_resume1_raw(continuation, value, out);
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

int64_t flux_async_make_adt0(int32_t ctor_tag) {
    void *mem = flux_gc_alloc_header(8, 0, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = ctor_tag;
    hdr[1] = 0;
    return flux_tag_ptr(mem);
}

int64_t flux_async_make_adt1(int32_t ctor_tag, int64_t value) {
    void *mem = flux_gc_alloc_header(8 + 8, 1, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = ctor_tag;
    hdr[1] = 1;
    int64_t *fields = (int64_t *)((char *)mem + 8);
    fields[0] = value;
    return flux_tag_ptr(mem);
}

void flux_async_promote(int64_t value) {
    flux_rc_promote(value);
}

void flux_async_enter_worker_thread(void) {
    flux_worker_thread = 1;
}

int64_t flux_async_make_string(const uint8_t *data, uintptr_t len) {
    return flux_string_new((const char *)data, (uint32_t)len);
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

int64_t flux_fiber_first_of(int64_t children) {
    size_t len = 0;
    size_t cap = 8;
    int64_t *closures = (int64_t *)malloc(cap * sizeof(int64_t));
    if (!closures) {
        fprintf(stderr, "flux_fiber_first_of: out of memory\n");
        abort();
    }

    int64_t cur = children;
    while (cur != FLUX_EMPTY_LIST && cur != FLUX_NONE) {
        if (!flux_is_ptr(cur)) {
            fprintf(stderr, "flux_fiber_first_of: expected List of closures\n");
            free(closures);
            abort();
        }
        void *ptr = flux_untag_ptr(cur);
        if (flux_obj_tag(ptr) != FLUX_OBJ_ADT) {
            fprintf(stderr, "flux_fiber_first_of: expected List of closures\n");
            free(closures);
            abort();
        }
        int32_t ctor_tag = *(int32_t *)ptr;
        int32_t field_count = *((int32_t *)ptr + 1);
        if (ctor_tag != 4 || field_count != 2) {
            fprintf(stderr, "flux_fiber_first_of: expected List of closures\n");
            free(closures);
            abort();
        }
        int64_t *fields = (int64_t *)((char *)ptr + 8);
        if (len == cap) {
            cap *= 2;
            int64_t *next = (int64_t *)realloc(closures, cap * sizeof(int64_t));
            if (!next) {
                fprintf(stderr, "flux_fiber_first_of: out of memory\n");
                free(closures);
                abort();
            }
            closures = next;
        }
        closures[len++] = fields[0];
        cur = fields[1];
    }

    if (len == 0) {
        fprintf(stderr, "flux_fiber_first_of: empty list\n");
        free(closures);
        abort();
    }

    uint64_t request_id = flux_async_fiber_first_of(closures, (uintptr_t)len);
    free(closures);
    return flux_async_suspend_request(request_id);
}

int64_t flux_fiber_try(int32_t ok_ctor_tag, int32_t err_ctor_tag, int32_t panicked_ctor_tag, int64_t body) {
    uint64_t request_id = flux_async_fiber_try(ok_ctor_tag, err_ctor_tag, panicked_ctor_tag, body);
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
    int64_t id = (int64_t)flux_async_scope_new();
    void *mem = flux_gc_alloc_header(8 + 8, 1, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = scope_ctor_tag;
    hdr[1] = 1; /* field_count */
    int64_t *fields = (int64_t *)((char *)mem + 8);
    fields[0] = flux_tag_int(id);
    return flux_tag_ptr(mem);
}

int64_t flux_fiber_fork_scoped(int64_t s, int64_t f) {
    void *scope_ptr = flux_untag_ptr(s);
    int64_t *fields = (int64_t *)((char *)scope_ptr + 8);
    uint64_t scope_id = (uint64_t)flux_untag_int(fields[0]);
    if (flux_async_fork_scoped(scope_id, f) != 0) {
        fprintf(stderr, "flux_fiber_fork_scoped: async scheduler is not active\n");
        abort();
    }
    return FLUX_NONE;
}

int64_t flux_fiber_cancel_scope(int64_t s) {
    void *scope_ptr = flux_untag_ptr(s);
    int64_t *fields = (int64_t *)((char *)scope_ptr + 8);
    uint64_t scope_id = (uint64_t)flux_untag_int(fields[0]);
    if (flux_async_cancel_scope(scope_id) != 0) {
        fprintf(stderr, "flux_fiber_cancel_scope: async scheduler is not active\n");
        abort();
    }
    return FLUX_NONE;
}

/* Phase 2 slice 2-iv: poll the current fiber's scope cancel flag. */
int64_t flux_fiber_check_cancelled(void) {
    return flux_make_bool(flux_async_check_cancelled() != 0);
}

/* Slice 2-vii follow-up: report the active scheduler's worker count.
 * Returns 0 outside `run_async`. */
int64_t flux_fiber_current_worker_count(void) {
    return flux_tag_int((int64_t)flux_async_current_worker_count());
}

/* Phase 2 slice 2-vii: run an async action with explicit RuntimeConfig.
 * The Flux ints are NaN-boxed; untag before forwarding to Rust. */
int64_t flux_fiber_run_async_with(int64_t worker_count, int64_t fs_pool_size, int64_t dns_pool_size, int64_t closure) {
    flux_async_register_callbacks();
    int64_t workers = flux_untag_int(worker_count);
    int64_t fs = flux_untag_int(fs_pool_size);
    int64_t dns = flux_untag_int(dns_pool_size);
    return flux_async_run_root_with(workers, fs, dns, closure);
}
