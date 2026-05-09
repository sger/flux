/*
 * channel.c — Native Flow.Channel<a> runtime.
 *
 * Bounded FIFO channels backed by a process-local table. Suspending send/recv
 * use the same request-id ABI as Task.await: register a request, park the
 * current fiber, and publish completion with flux_async_task_complete.
 */

#include "flux_rt.h"
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if !defined(_WIN32) && !defined(_MSC_VER)

#include <pthread.h>
#include <stdatomic.h>

#define FLUX_CHANNEL_TABLE_MAX 1024

typedef struct {
    uint64_t req;
    int64_t value;
} WaitingSend;

typedef struct {
    int64_t id;
    int64_t capacity;
    int64_t len;
    int64_t head;
    int64_t *buf;
    int closed;

    uint64_t recv_reqs[FLUX_CHANNEL_TABLE_MAX];
    int recv_head;
    int recv_len;

    WaitingSend send_reqs[FLUX_CHANNEL_TABLE_MAX];
    int send_head;
    int send_len;

    pthread_mutex_t mutex;
    pthread_cond_t ready;
} FluxChannel;

static FluxChannel channels[FLUX_CHANNEL_TABLE_MAX];
static pthread_mutex_t channels_mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_once_t channels_once = PTHREAD_ONCE_INIT;
static _Atomic(int64_t) next_channel_id = 1;

static void channels_do_init(void) {
    for (int i = 0; i < FLUX_CHANNEL_TABLE_MAX; i++) {
        channels[i].id = 0;
        pthread_mutex_init(&channels[i].mutex, NULL);
        pthread_cond_init(&channels[i].ready, NULL);
    }
}

static void channels_init_once(void) {
    pthread_once(&channels_once, channels_do_init);
}

static int64_t retain_value(int64_t value) {
    flux_rc_promote(value);
    flux_dup(value);
    return value;
}

static void publish_value(uint64_t req, int64_t option_value) {
    if (req != 0) {
        flux_async_task_complete(req, option_value);
    }
}

static FluxChannel *lookup_channel(int64_t id) {
    channels_init_once();
    if (id >= 1 && id <= FLUX_CHANNEL_TABLE_MAX) {
        FluxChannel *ch = &channels[id - 1];
        if (ch->id == id) {
            return ch;
        }
    }
    return NULL;
}

static void abort_unknown(const char *which, int64_t id) {
    fprintf(stderr, "%s: unknown channel id %" PRId64 "\n", which, id);
    abort();
}

static int recv_push(FluxChannel *ch, uint64_t req) {
    if (ch->recv_len >= FLUX_CHANNEL_TABLE_MAX) {
        return 0;
    }
    int idx = (ch->recv_head + ch->recv_len) % FLUX_CHANNEL_TABLE_MAX;
    ch->recv_reqs[idx] = req;
    ch->recv_len++;
    return 1;
}

static int recv_pop(FluxChannel *ch, uint64_t *req) {
    if (ch->recv_len == 0) {
        return 0;
    }
    *req = ch->recv_reqs[ch->recv_head];
    ch->recv_head = (ch->recv_head + 1) % FLUX_CHANNEL_TABLE_MAX;
    ch->recv_len--;
    return 1;
}

static int send_push(FluxChannel *ch, uint64_t req, int64_t value) {
    if (ch->send_len >= FLUX_CHANNEL_TABLE_MAX) {
        return 0;
    }
    int idx = (ch->send_head + ch->send_len) % FLUX_CHANNEL_TABLE_MAX;
    ch->send_reqs[idx].req = req;
    ch->send_reqs[idx].value = value;
    ch->send_len++;
    return 1;
}

static int send_pop(FluxChannel *ch, WaitingSend *send) {
    if (ch->send_len == 0) {
        return 0;
    }
    *send = ch->send_reqs[ch->send_head];
    ch->send_head = (ch->send_head + 1) % FLUX_CHANNEL_TABLE_MAX;
    ch->send_len--;
    return 1;
}

static void buf_push(FluxChannel *ch, int64_t value) {
    int64_t idx = (ch->head + ch->len) % ch->capacity;
    ch->buf[idx] = value;
    ch->len++;
}

static int64_t buf_pop(FluxChannel *ch) {
    int64_t value = ch->buf[ch->head];
    ch->head = (ch->head + 1) % ch->capacity;
    ch->len--;
    return value;
}

static void flush_waiters(FluxChannel *ch) {
    uint64_t recv_req = 0;
    while (recv_pop(ch, &recv_req)) {
        if (ch->len > 0) {
            publish_value(recv_req, flux_wrap_some(buf_pop(ch)));
            continue;
        }
        WaitingSend send;
        if (send_pop(ch, &send)) {
            publish_value(send.req, FLUX_NONE);
            publish_value(recv_req, flux_wrap_some(send.value));
            continue;
        }
        if (ch->closed) {
            publish_value(recv_req, FLUX_NONE);
            continue;
        }
        ch->recv_head = (ch->recv_head - 1 + FLUX_CHANNEL_TABLE_MAX) % FLUX_CHANNEL_TABLE_MAX;
        ch->recv_reqs[ch->recv_head] = recv_req;
        ch->recv_len++;
        break;
    }

    while (!ch->closed && ch->capacity > 0 && ch->len < ch->capacity) {
        WaitingSend send;
        if (!send_pop(ch, &send)) {
            break;
        }
        if (recv_pop(ch, &recv_req)) {
            publish_value(send.req, FLUX_NONE);
            publish_value(recv_req, flux_wrap_some(send.value));
        } else {
            buf_push(ch, send.value);
            publish_value(send.req, FLUX_NONE);
        }
    }

    if (ch->closed) {
        WaitingSend send;
        while (send_pop(ch, &send)) {
            flux_drop(send.value);
            publish_value(send.req, FLUX_NONE);
        }
    }
}

int64_t flux_chan_make(int64_t capacity_val) {
    channels_init_once();
    int64_t capacity = flux_untag_int(capacity_val);
    if (capacity < 0) {
        fprintf(stderr, "flux_chan_make: capacity must be non-negative\n");
        abort();
    }

    int64_t id = atomic_fetch_add_explicit(&next_channel_id, 1, memory_order_relaxed);
    if (id < 1 || id > FLUX_CHANNEL_TABLE_MAX) {
        pthread_mutex_unlock(&channels_mutex);
        fprintf(stderr, "flux_chan_make: channel table full\n");
        abort();
    }
    FluxChannel *ch = &channels[id - 1];
    if (ch->id != 0) {
        pthread_mutex_unlock(&channels_mutex);
        fprintf(stderr, "flux_chan_make: channel table full\n");
        abort();
    }

    ch->id = id;
    ch->capacity = capacity;
    ch->len = 0;
    ch->head = 0;
    ch->closed = 0;
    ch->recv_head = ch->recv_len = 0;
    ch->send_head = ch->send_len = 0;
    ch->buf = capacity > 0 ? (int64_t *)calloc((size_t)capacity, sizeof(int64_t)) : NULL;
    if (capacity > 0 && !ch->buf) {
        ch->id = 0;
        pthread_mutex_unlock(&channels_mutex);
        fprintf(stderr, "flux_chan_make: out of memory\n");
        abort();
    }
    pthread_mutex_unlock(&channels_mutex);
    return flux_tag_int(id);
}

int64_t flux_chan_try_send(int64_t id_val, int64_t value) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_try_send", id);

    pthread_mutex_lock(&ch->mutex);
    if (ch->closed) {
        pthread_mutex_unlock(&ch->mutex);
        return FLUX_FALSE;
    }
    uint64_t recv_req = 0;
    if (recv_pop(ch, &recv_req)) {
        publish_value(recv_req, flux_wrap_some(retain_value(value)));
        pthread_cond_broadcast(&ch->ready);
        pthread_mutex_unlock(&ch->mutex);
        return FLUX_TRUE;
    }
    if (ch->capacity > 0 && ch->len < ch->capacity) {
        buf_push(ch, retain_value(value));
        pthread_cond_broadcast(&ch->ready);
        pthread_mutex_unlock(&ch->mutex);
        return FLUX_TRUE;
    }
    pthread_mutex_unlock(&ch->mutex);
    return FLUX_FALSE;
}

int64_t flux_chan_try_recv(int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_try_recv", id);

    pthread_mutex_lock(&ch->mutex);
    if (ch->len > 0) {
        int64_t value = buf_pop(ch);
        flush_waiters(ch);
        pthread_cond_broadcast(&ch->ready);
        pthread_mutex_unlock(&ch->mutex);
        return flux_wrap_some(value);
    }
    WaitingSend send;
    if (send_pop(ch, &send)) {
        publish_value(send.req, FLUX_NONE);
        pthread_cond_broadcast(&ch->ready);
        pthread_mutex_unlock(&ch->mutex);
        return flux_wrap_some(send.value);
    }
    pthread_mutex_unlock(&ch->mutex);
    return FLUX_NONE;
}

int64_t flux_chan_send(int64_t id_val, int64_t value) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_send", id);

    /* LLVM lowers ChanSend as an always-yielding primop. Even immediate
     * completions publish to a fresh request and suspend so the generated
     * continuation receives Unit through the native async scheduler. */
    uint64_t request_id = flux_async_task_await_request();
    pthread_mutex_lock(&ch->mutex);
    if (ch->closed) {
        pthread_mutex_unlock(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, FLUX_NONE);
            return flux_async_suspend_request(request_id);
        }
        return FLUX_NONE;
    }
    uint64_t recv_req = 0;
    if (recv_pop(ch, &recv_req)) {
        publish_value(recv_req, flux_wrap_some(retain_value(value)));
        pthread_cond_broadcast(&ch->ready);
        pthread_mutex_unlock(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, FLUX_NONE);
            return flux_async_suspend_request(request_id);
        }
        return FLUX_NONE;
    }
    if (ch->capacity > 0 && ch->len < ch->capacity) {
        buf_push(ch, retain_value(value));
        pthread_cond_broadcast(&ch->ready);
        pthread_mutex_unlock(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, FLUX_NONE);
            return flux_async_suspend_request(request_id);
        }
        return FLUX_NONE;
    }
    if (request_id == 0) {
        while (!ch->closed) {
            pthread_cond_wait(&ch->ready, &ch->mutex);
            if (recv_pop(ch, &recv_req)) {
                publish_value(recv_req, flux_wrap_some(retain_value(value)));
                pthread_mutex_unlock(&ch->mutex);
                return FLUX_NONE;
            }
            if (ch->capacity > 0 && ch->len < ch->capacity) {
                buf_push(ch, retain_value(value));
                pthread_mutex_unlock(&ch->mutex);
                return FLUX_NONE;
            }
        }
        pthread_mutex_unlock(&ch->mutex);
        return FLUX_NONE;
    }
    if (!send_push(ch, request_id, retain_value(value))) {
        pthread_mutex_unlock(&ch->mutex);
        fprintf(stderr, "flux_chan_send: too many waiting senders\n");
        abort();
    }
    pthread_mutex_unlock(&ch->mutex);
    return flux_async_suspend_request(request_id);
}

int64_t flux_chan_recv(int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_recv", id);

    /* LLVM lowers ChanRecv as an always-yielding primop. Fast-path values are
     * published before suspension so the generated continuation receives the
     * Option<a> through the same path as genuinely parked receives. */
    uint64_t request_id = flux_async_task_await_request();
    pthread_mutex_lock(&ch->mutex);
    if (ch->len > 0) {
        int64_t value = buf_pop(ch);
        flush_waiters(ch);
        pthread_cond_broadcast(&ch->ready);
        pthread_mutex_unlock(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, flux_wrap_some(value));
            return flux_async_suspend_request(request_id);
        }
        return flux_wrap_some(value);
    }
    WaitingSend send;
    if (send_pop(ch, &send)) {
        publish_value(send.req, FLUX_NONE);
        pthread_cond_broadcast(&ch->ready);
        pthread_mutex_unlock(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, flux_wrap_some(send.value));
            return flux_async_suspend_request(request_id);
        }
        return flux_wrap_some(send.value);
    }
    if (ch->closed) {
        pthread_mutex_unlock(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, FLUX_NONE);
            return flux_async_suspend_request(request_id);
        }
        return FLUX_NONE;
    }
    if (request_id == 0) {
        while (!ch->closed && ch->len == 0 && ch->send_len == 0) {
            pthread_cond_wait(&ch->ready, &ch->mutex);
        }
        pthread_mutex_unlock(&ch->mutex);
        return flux_chan_recv(id_val);
    }
    if (!recv_push(ch, request_id)) {
        pthread_mutex_unlock(&ch->mutex);
        fprintf(stderr, "flux_chan_recv: too many waiting receivers\n");
        abort();
    }
    pthread_mutex_unlock(&ch->mutex);
    return flux_async_suspend_request(request_id);
}

int64_t flux_chan_close(int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_close", id);

    pthread_mutex_lock(&ch->mutex);
    if (!ch->closed) {
        ch->closed = 1;
        uint64_t recv_req = 0;
        while (recv_pop(ch, &recv_req)) {
            if (ch->len > 0) {
                publish_value(recv_req, flux_wrap_some(buf_pop(ch)));
            } else {
                publish_value(recv_req, FLUX_NONE);
            }
        }
        WaitingSend send;
        while (send_pop(ch, &send)) {
            flux_drop(send.value);
            publish_value(send.req, FLUX_NONE);
        }
    }
    pthread_cond_broadcast(&ch->ready);
    pthread_mutex_unlock(&ch->mutex);
    return FLUX_NONE;
}

int64_t flux_chan_len(int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_len", id);
    pthread_mutex_lock(&ch->mutex);
    int64_t len = ch->len;
    pthread_mutex_unlock(&ch->mutex);
    return flux_tag_int(len);
}

int64_t flux_chan_cap(int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_cap", id);
    return flux_tag_int(ch->capacity);
}

#else

static void channel_unimplemented(const char *which) {
    fprintf(stderr, "%s: Flow.Channel native runtime is not implemented on this platform\n", which);
    abort();
}

int64_t flux_chan_make(int64_t capacity) { (void)capacity; channel_unimplemented("flux_chan_make"); }
int64_t flux_chan_send(int64_t id, int64_t value) { (void)id; (void)value; channel_unimplemented("flux_chan_send"); }
int64_t flux_chan_recv(int64_t id) { (void)id; channel_unimplemented("flux_chan_recv"); }
int64_t flux_chan_try_send(int64_t id, int64_t value) { (void)id; (void)value; channel_unimplemented("flux_chan_try_send"); }
int64_t flux_chan_try_recv(int64_t id) { (void)id; channel_unimplemented("flux_chan_try_recv"); }
int64_t flux_chan_close(int64_t id) { (void)id; channel_unimplemented("flux_chan_close"); }
int64_t flux_chan_len(int64_t id) { (void)id; channel_unimplemented("flux_chan_len"); }
int64_t flux_chan_cap(int64_t id) { (void)id; channel_unimplemented("flux_chan_cap"); }

#endif
