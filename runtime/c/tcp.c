/*
 * tcp.c — Fiber-suspending TCP primitives for proposal 0174 Phase 1b-vii.
 *
 * LLVM/native code calls these symbols for Flow.Tcp primops. The actual
 * socket state machines live in Rust's MioBackend; these wrappers decode Flux
 * values, submit an async request, and suspend the current fiber until the
 * native async scheduler resumes it.
 */

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>

static int64_t flux_tcp_suspend_or_abort(uint64_t request_id, const char *which) {
    if (request_id == 0) {
        fprintf(stderr, "%s: async TCP request registration failed\n", which);
        abort();
    }
    return flux_async_suspend_request(request_id);
}

int64_t flux_tcp_connect(int64_t host_val, int64_t port_val) {
    const uint8_t *host = (const uint8_t *)flux_string_data(host_val);
    uintptr_t host_len = (uintptr_t)flux_string_len(host_val);
    int64_t port = flux_untag_int(port_val);
    uint64_t request_id = flux_async_tcp_connect(host, host_len, port);
    return flux_tcp_suspend_or_abort(request_id, "flux_tcp_connect");
}

int64_t flux_tcp_read(int64_t handle_val, int64_t max_val) {
    uint64_t handle = (uint64_t)flux_untag_int(handle_val);
    int64_t raw_max = flux_untag_int(max_val);
    uintptr_t max = raw_max > 0 ? (uintptr_t)raw_max : 0;
    uint64_t request_id = flux_async_tcp_read(handle, max);
    return flux_tcp_suspend_or_abort(request_id, "flux_tcp_read");
}

int64_t flux_tcp_write_all(int64_t handle_val, int64_t data_val) {
    uint64_t handle = (uint64_t)flux_untag_int(handle_val);
    const uint8_t *data = (const uint8_t *)flux_string_data(data_val);
    uintptr_t len = (uintptr_t)flux_string_len(data_val);
    uint64_t request_id = flux_async_tcp_write_all(handle, data, len);
    return flux_tcp_suspend_or_abort(request_id, "flux_tcp_write_all");
}

int64_t flux_tcp_close(int64_t handle_val) {
    uint64_t handle = (uint64_t)flux_untag_int(handle_val);
    (void)flux_async_tcp_close(handle);
    return FLUX_NONE;
}

int64_t flux_tcp_listen(int64_t host_val, int64_t port_val) {
    const uint8_t *host = (const uint8_t *)flux_string_data(host_val);
    uintptr_t host_len = (uintptr_t)flux_string_len(host_val);
    int64_t port = flux_untag_int(port_val);
    uint64_t request_id = flux_async_tcp_listen(host, host_len, port);
    return flux_tcp_suspend_or_abort(request_id, "flux_tcp_listen");
}

int64_t flux_tcp_accept(int64_t listener_val) {
    uint64_t listener = (uint64_t)flux_untag_int(listener_val);
    uint64_t request_id = flux_async_tcp_accept(listener);
    return flux_tcp_suspend_or_abort(request_id, "flux_tcp_accept");
}

int64_t flux_http_serve_config(
    int64_t addr_val,
    int64_t port_val,
    int64_t config_val,
    int64_t handler_val
) {
    (void)addr_val;
    (void)port_val;
    (void)config_val;
    (void)handler_val;
    return flux_tag_int(0);
}

int64_t flux_http_shutdown(int64_t handle_val) {
    (void)handle_val;
    return FLUX_NONE;
}

int64_t flux_http_shutdown_now(int64_t handle_val) {
    (void)handle_val;
    return FLUX_NONE;
}
