/*
 * tasks.c — Native FFI stubs for `Flow.Task` (proposal 0174 D5-a).
 *
 * The Rust task scheduler in `src/runtime/async/task_scheduler.rs` is the
 * intended implementation, but it is reachable from native code only once
 * the staticlib infrastructure (D5-b) lands. Until then these stubs abort
 * with a diagnostic so a `--native` binary that calls into `Flow.Task`
 * fails loudly instead of silently corrupting state.
 *
 * The VM backend implements `Task.spawn`/`blocking_join`/`cancel`
 * end-to-end via `src/vm/core_dispatch.rs` and exercises them through the
 * Flux test runner — every Phase 1a-vi follow-up test runs there.
 */

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>

static void flux_task_unimplemented(const char *which) {
    fprintf(stderr,
        "flux: %s called on native backend, but the native FFI bridge to\n"
        "      the Rust task scheduler is not yet wired (proposal 0174 D5-b).\n"
        "      Use the VM backend (drop --native) until D5-b lands.\n",
        which);
    abort();
}

int64_t flux_task_spawn(int64_t closure) {
    (void)closure;
    flux_task_unimplemented("flux_task_spawn");
}

int64_t flux_task_blocking_join(int64_t task) {
    (void)task;
    flux_task_unimplemented("flux_task_blocking_join");
}

int64_t flux_task_cancel(int64_t task) {
    (void)task;
    flux_task_unimplemented("flux_task_cancel");
}
