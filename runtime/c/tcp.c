/*
 * tcp.c — Blocking TCP primitives for proposal 0174 Phase 1b-vii.
 *
 * VM path (sequential-equivalent semantics):
 *   Each function blocks the calling OS thread until the operation completes.
 *   This matches the behaviour of `sleep` and `blocking_join` on the VM path.
 *
 * Native path:
 *   The LLVM backend will call these same symbols for now; a future slice will
 *   replace them with fiber-suspending variants that yield to the mio reactor
 *   instead of blocking the thread.
 *
 * Value encoding:
 *   All int64_t arguments/results use Flux NaN-box pointer-tag encoding:
 *     - Integers: flux_tag_int / flux_untag_int  (bit 0 = 1)
 *     - Strings:  heap pointer (bit 0 = 0) — use flux_string_data / flux_string_len
 *     - Errors:   return flux_tag_int(-1) as a sentinel
 *
 * Handle encoding:
 *   OS file descriptors are small non-negative integers; we store them as
 *   tagged Flux integers (flux_tag_int(fd)).
 *
 * Windows:
 *   A Winsock2 fallback is included under #if defined(_WIN32).  Full support
 *   (WSAStartup lifetime, SOCKET vs int) will land in a follow-up.
 */

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>

/* ── POSIX implementation (macOS + Linux) ──────────────────────────── */
#if !defined(_WIN32) && !defined(_MSC_VER)

#include <unistd.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <netdb.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <pthread.h>

/* getaddrinfo is not guaranteed to be thread-safe on all systems.
 * Protect its use with a mutex. */
static pthread_mutex_t g_getaddrinfo_mutex = PTHREAD_MUTEX_INITIALIZER;

/* Helper: resolve host string + port to a connected socket.
 * Returns the file descriptor on success, -1 on failure. */
static int tcp_connect_fd(const char *host, int port) {
    char port_str[16];
    snprintf(port_str, sizeof(port_str), "%d", port);

    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family   = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;

    struct addrinfo *res = NULL;
    pthread_mutex_lock(&g_getaddrinfo_mutex);
    if (getaddrinfo(host, port_str, &hints, &res) != 0 || res == NULL) {
        pthread_mutex_unlock(&g_getaddrinfo_mutex);
        return -1;
    }

    int fd = -1;
    for (struct addrinfo *rp = res; rp != NULL; rp = rp->ai_next) {
        fd = socket(rp->ai_family, rp->ai_socktype, rp->ai_protocol);
        if (fd < 0) continue;
        if (connect(fd, rp->ai_addr, rp->ai_addrlen) == 0) break;
        close(fd);
        fd = -1;
    }
    freeaddrinfo(res);
    pthread_mutex_unlock(&g_getaddrinfo_mutex);
    return fd;
}

/*
 * flux_tcp_connect(host_val, host_len_ignored, port_val) -> handle_val
 *
 * host_val     : NaN-boxed pointer to a FluxString (use flux_string_data)
 * host_len_ignored: unused (length already inside FluxString header)
 * port_val     : NaN-boxed integer port number
 * returns      : NaN-boxed integer fd, or flux_tag_int(-1) on error.
 *
 * Note: the VM core_dispatch passes (host_ptr, host_len, port) as raw int64_t.
 * We receive the actual Flux value in host_val, so we decode it via
 * flux_string_data / flux_string_len.  The second argument (host_len) from the
 * dispatch side is the NaN-boxed string value itself — we ignore it and use
 * flux_string_data(host_val) directly.
 */
int64_t flux_tcp_connect(int64_t host_val, int64_t host_len_ignored, int64_t port_val) {
    (void)host_len_ignored;
    /* host_val is a NaN-boxed FluxString pointer; decode via runtime helpers. */
    const char *host = flux_string_data(host_val);
    int port = (int)flux_untag_int(port_val);
    int fd = tcp_connect_fd(host, port);
    return flux_tag_int((int64_t)fd);
}

/*
 * flux_tcp_read(handle_val, buf_ptr_ignored, max_val) -> result_val
 *
 * Reads up to max bytes from the connection and returns a new FluxString.
 * Returns flux_tag_int(-1) on error.
 *
 * Note: buf_ptr_ignored is the second argument from dispatch, which is the
 * NaN-boxed max Int (same as max_val in the 3-arg C declaration).
 * We use the third arg as max_val.
 */
int64_t flux_tcp_read(int64_t handle_val, int64_t buf_ptr_ignored, int64_t max_val) {
    (void)buf_ptr_ignored;
    int fd = (int)flux_untag_int(handle_val);
    int max = (int)flux_untag_int(max_val);
    if (max <= 0 || max > (1 << 24)) max = 4096;

    char *buf = (char *)malloc((size_t)max);
    if (!buf) return flux_tag_int(-1);

    ssize_t n = recv(fd, buf, (size_t)max, 0);
    if (n < 0) {
        free(buf);
        return flux_tag_int(-1);
    }
    int64_t result = flux_string_new(buf, (uint32_t)n);
    free(buf);
    return result;
}

/*
 * flux_tcp_write_all(handle_val, data_val, data_len_ignored) -> FLUX_NONE or error
 *
 * Writes all bytes of data_val (a FluxString) to the connection.
 * Returns FLUX_NONE on success, flux_tag_int(-1) on error.
 */
int64_t flux_tcp_write_all(int64_t handle_val, int64_t data_val, int64_t data_len_ignored) {
    (void)data_len_ignored;
    int fd = (int)flux_untag_int(handle_val);
    const char *data = flux_string_data(data_val);
    uint32_t len = flux_string_len(data_val);

    size_t sent = 0;
    while (sent < (size_t)len) {
        ssize_t n = send(fd, data + sent, (size_t)len - sent, 0);
        if (n < 0) return flux_tag_int(-1);
        sent += (size_t)n;
    }
    return FLUX_NONE;
}

/*
 * flux_tcp_close(handle_val) -> FLUX_NONE
 */
int64_t flux_tcp_close(int64_t handle_val) {
    int fd = (int)flux_untag_int(handle_val);
    if (fd >= 0) close(fd);
    return FLUX_NONE;
}

/*
 * flux_tcp_listen(host_val, host_len_ignored, port_val) -> listener_val
 *
 * Binds and listens on the given address.  Returns a NaN-boxed fd.
 */
int64_t flux_tcp_listen(int64_t host_val, int64_t host_len_ignored, int64_t port_val) {
    (void)host_len_ignored;
    const char *host = flux_string_data(host_val);
    int port = (int)flux_untag_int(port_val);

    char port_str[16];
    snprintf(port_str, sizeof(port_str), "%d", port);

    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family   = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags    = AI_PASSIVE;

    struct addrinfo *res = NULL;
    pthread_mutex_lock(&g_getaddrinfo_mutex);
    if (getaddrinfo(*host ? host : NULL, port_str, &hints, &res) != 0 || !res) {
        pthread_mutex_unlock(&g_getaddrinfo_mutex);
        return flux_tag_int(-1);
    }

    int fd = -1;
    for (struct addrinfo *rp = res; rp != NULL; rp = rp->ai_next) {
        fd = socket(rp->ai_family, rp->ai_socktype, rp->ai_protocol);
        if (fd < 0) continue;
        int yes = 1;
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
        if (bind(fd, rp->ai_addr, rp->ai_addrlen) == 0) break;
        close(fd);
        fd = -1;
    }
    freeaddrinfo(res);
    pthread_mutex_unlock(&g_getaddrinfo_mutex);

    if (fd < 0) return flux_tag_int(-1);
    if (listen(fd, SOMAXCONN) < 0) { close(fd); return flux_tag_int(-1); }
    return flux_tag_int((int64_t)fd);
}

/*
 * flux_tcp_accept(listener_val) -> handle_val
 *
 * Blocks until a new connection arrives; returns its fd.
 */
int64_t flux_tcp_accept(int64_t listener_val) {
    int listener = (int)flux_untag_int(listener_val);
    int fd = accept(listener, NULL, NULL);
    return flux_tag_int((int64_t)fd);
}

/* ── Windows stub ──────────────────────────────────────────────────── */
#else

static void flux_tcp_unimplemented(const char *which) {
    fprintf(stderr,
        "flux: %s is not yet implemented on Windows (proposal 0174 Phase 1b-vii follow-up).\n"
        "      Use the VM backend on Linux/macOS for TCP operations.\n",
        which);
    abort();
}

int64_t flux_tcp_connect(int64_t h, int64_t hl, int64_t p) {
    (void)h; (void)hl; (void)p;
    flux_tcp_unimplemented("flux_tcp_connect");
    return FLUX_NONE;
}
int64_t flux_tcp_read(int64_t h, int64_t b, int64_t m) {
    (void)h; (void)b; (void)m;
    flux_tcp_unimplemented("flux_tcp_read");
    return FLUX_NONE;
}
int64_t flux_tcp_write_all(int64_t h, int64_t d, int64_t l) {
    (void)h; (void)d; (void)l;
    flux_tcp_unimplemented("flux_tcp_write_all");
    return FLUX_NONE;
}
int64_t flux_tcp_close(int64_t h) {
    (void)h;
    flux_tcp_unimplemented("flux_tcp_close");
    return FLUX_NONE;
}
int64_t flux_tcp_listen(int64_t h, int64_t hl, int64_t p) {
    (void)h; (void)hl; (void)p;
    flux_tcp_unimplemented("flux_tcp_listen");
    return FLUX_NONE;
}
int64_t flux_tcp_accept(int64_t l) {
    (void)l;
    flux_tcp_unimplemented("flux_tcp_accept");
    return FLUX_NONE;
}

#endif /* !_WIN32 */
