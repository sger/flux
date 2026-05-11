/*
 * channel.c — Native Flow.Channel<a> runtime.
 *
 * Bounded FIFO channels backed by a process-local table. Suspending send/recv
 * use the same request-id ABI as Task.await: register a request, park the
 * current fiber, and publish completion with flux_async_task_complete.
 *
 * Cross-platform: POSIX uses pthreads + stdatomic; Windows uses
 * CRITICAL_SECTION / CONDITION_VARIABLE / Interlocked*. Mirror the layout of
 * tasks.c so the two runtimes stay parallel.
 */

#include "flux_rt.h"
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ── Platform sync primitives ─────────────────────────────────────────── */

#if defined(_WIN32) || defined(_MSC_VER)

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

typedef CRITICAL_SECTION   flux_mutex_t;
typedef CONDITION_VARIABLE flux_cond_t;

#define FLUX_MUTEX_INIT(m)       InitializeCriticalSection(m)
#define FLUX_MUTEX_LOCK(m)       EnterCriticalSection(m)
#define FLUX_MUTEX_UNLOCK(m)     LeaveCriticalSection(m)
#define FLUX_COND_INIT(c)        InitializeConditionVariable(c)
#define FLUX_COND_WAIT(c, m)     SleepConditionVariableCS((c), (m), INFINITE)
#define FLUX_COND_BROADCAST(c)   WakeAllConditionVariable(c)

/* Atomic int64 — Interlocked* on volatile LONG64. */
typedef volatile LONG64 flux_atomic_i64;
static inline int64_t flux_atomic_fetch_add_i64(flux_atomic_i64 *p, int64_t v) {
    return (int64_t)InterlockedExchangeAdd64(p, (LONG64)v);
}

#else

#include <pthread.h>
#include <stdatomic.h>

typedef pthread_mutex_t flux_mutex_t;
typedef pthread_cond_t  flux_cond_t;

#define FLUX_MUTEX_INIT(m)       pthread_mutex_init((m), NULL)
#define FLUX_MUTEX_LOCK(m)       pthread_mutex_lock(m)
#define FLUX_MUTEX_UNLOCK(m)     pthread_mutex_unlock(m)
#define FLUX_COND_INIT(c)        pthread_cond_init((c), NULL)
#define FLUX_COND_WAIT(c, m)     pthread_cond_wait((c), (m))
#define FLUX_COND_BROADCAST(c)   pthread_cond_broadcast(c)

typedef _Atomic(int64_t) flux_atomic_i64;
static inline int64_t flux_atomic_fetch_add_i64(flux_atomic_i64 *p, int64_t v) {
    return atomic_fetch_add_explicit(p, v, memory_order_relaxed);
}

#endif

/* ── Channel runtime (single implementation) ──────────────────────────── */

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

    flux_mutex_t mutex;
    flux_cond_t  ready;
} FluxChannel;

static FluxChannel channels[FLUX_CHANNEL_TABLE_MAX];
static flux_atomic_i64 next_channel_id = 1;

/* One-shot table init. POSIX used pthread_once; portable replacement is an
 * Interlocked compare-exchange flag with a ready flag. */
#if defined(_WIN32) || defined(_MSC_VER)
static volatile LONG64 channels_init_state = 0; /* 0 = uninit, 1 = initing, 2 = ready */
#endif

static void channels_do_init(void) {
    for (int i = 0; i < FLUX_CHANNEL_TABLE_MAX; i++) {
        channels[i].id = 0;
        FLUX_MUTEX_INIT(&channels[i].mutex);
        FLUX_COND_INIT(&channels[i].ready);
    }
}

static void channels_init_once(void) {
#if defined(_WIN32) || defined(_MSC_VER)
    LONG64 prev = InterlockedCompareExchange64(&channels_init_state, 1, 0);
    if (prev == 0) {
        channels_do_init();
        InterlockedExchange64(&channels_init_state, 2);
        return;
    }
    while (InterlockedCompareExchange64(&channels_init_state, 2, 2) != 2) {
        Sleep(0);
    }
#else
    static pthread_once_t once = PTHREAD_ONCE_INIT;
    pthread_once(&once, channels_do_init);
#endif
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

    int64_t id = flux_atomic_fetch_add_i64(&next_channel_id, 1);
    if (id < 1 || id > FLUX_CHANNEL_TABLE_MAX) {
        fprintf(stderr, "flux_chan_make: channel table full\n");
        abort();
    }
    FluxChannel *ch = &channels[id - 1];
    if (ch->id != 0) {
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
        fprintf(stderr, "flux_chan_make: out of memory\n");
        abort();
    }
    return flux_tag_int(id);
}

int64_t flux_chan_try_send(int64_t id_val, int64_t value) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_try_send", id);

    FLUX_MUTEX_LOCK(&ch->mutex);
    if (ch->closed) {
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        return FLUX_FALSE;
    }
    uint64_t recv_req = 0;
    if (recv_pop(ch, &recv_req)) {
        publish_value(recv_req, flux_wrap_some(retain_value(value)));
        FLUX_COND_BROADCAST(&ch->ready);
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        return FLUX_TRUE;
    }
    if (ch->capacity > 0 && ch->len < ch->capacity) {
        buf_push(ch, retain_value(value));
        FLUX_COND_BROADCAST(&ch->ready);
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        return FLUX_TRUE;
    }
    FLUX_MUTEX_UNLOCK(&ch->mutex);
    return FLUX_FALSE;
}

int64_t flux_chan_try_recv(int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_try_recv", id);

    FLUX_MUTEX_LOCK(&ch->mutex);
    if (ch->len > 0) {
        int64_t value = buf_pop(ch);
        flush_waiters(ch);
        FLUX_COND_BROADCAST(&ch->ready);
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        return flux_wrap_some(value);
    }
    WaitingSend send;
    if (send_pop(ch, &send)) {
        publish_value(send.req, FLUX_NONE);
        FLUX_COND_BROADCAST(&ch->ready);
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        return flux_wrap_some(send.value);
    }
    FLUX_MUTEX_UNLOCK(&ch->mutex);
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
    FLUX_MUTEX_LOCK(&ch->mutex);
    if (ch->closed) {
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, FLUX_NONE);
            return flux_async_suspend_request(request_id);
        }
        return FLUX_NONE;
    }
    uint64_t recv_req = 0;
    if (recv_pop(ch, &recv_req)) {
        publish_value(recv_req, flux_wrap_some(retain_value(value)));
        FLUX_COND_BROADCAST(&ch->ready);
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, FLUX_NONE);
            return flux_async_suspend_request(request_id);
        }
        return FLUX_NONE;
    }
    if (ch->capacity > 0 && ch->len < ch->capacity) {
        buf_push(ch, retain_value(value));
        FLUX_COND_BROADCAST(&ch->ready);
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, FLUX_NONE);
            return flux_async_suspend_request(request_id);
        }
        return FLUX_NONE;
    }
    if (request_id == 0) {
        while (!ch->closed) {
            FLUX_COND_WAIT(&ch->ready, &ch->mutex);
            if (recv_pop(ch, &recv_req)) {
                publish_value(recv_req, flux_wrap_some(retain_value(value)));
                FLUX_MUTEX_UNLOCK(&ch->mutex);
                return FLUX_NONE;
            }
            if (ch->capacity > 0 && ch->len < ch->capacity) {
                buf_push(ch, retain_value(value));
                FLUX_MUTEX_UNLOCK(&ch->mutex);
                return FLUX_NONE;
            }
        }
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        return FLUX_NONE;
    }
    if (!send_push(ch, request_id, retain_value(value))) {
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        fprintf(stderr, "flux_chan_send: too many waiting senders\n");
        abort();
    }
    FLUX_MUTEX_UNLOCK(&ch->mutex);
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
    FLUX_MUTEX_LOCK(&ch->mutex);
    if (ch->len > 0) {
        int64_t value = buf_pop(ch);
        flush_waiters(ch);
        FLUX_COND_BROADCAST(&ch->ready);
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, flux_wrap_some(value));
            return flux_async_suspend_request(request_id);
        }
        return flux_wrap_some(value);
    }
    WaitingSend send;
    if (send_pop(ch, &send)) {
        publish_value(send.req, FLUX_NONE);
        FLUX_COND_BROADCAST(&ch->ready);
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, flux_wrap_some(send.value));
            return flux_async_suspend_request(request_id);
        }
        return flux_wrap_some(send.value);
    }
    if (ch->closed) {
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        if (request_id != 0) {
            publish_value(request_id, FLUX_NONE);
            return flux_async_suspend_request(request_id);
        }
        return FLUX_NONE;
    }
    if (request_id == 0) {
        while (!ch->closed && ch->len == 0 && ch->send_len == 0) {
            FLUX_COND_WAIT(&ch->ready, &ch->mutex);
        }
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        return flux_chan_recv(id_val);
    }
    if (!recv_push(ch, request_id)) {
        FLUX_MUTEX_UNLOCK(&ch->mutex);
        fprintf(stderr, "flux_chan_recv: too many waiting receivers\n");
        abort();
    }
    FLUX_MUTEX_UNLOCK(&ch->mutex);
    return flux_async_suspend_request(request_id);
}

int64_t flux_chan_close(int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_close", id);

    FLUX_MUTEX_LOCK(&ch->mutex);
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
    FLUX_COND_BROADCAST(&ch->ready);
    FLUX_MUTEX_UNLOCK(&ch->mutex);
    return FLUX_NONE;
}

int64_t flux_chan_len(int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_len", id);
    FLUX_MUTEX_LOCK(&ch->mutex);
    int64_t len = ch->len;
    FLUX_MUTEX_UNLOCK(&ch->mutex);
    return flux_tag_int(len);
}

int64_t flux_chan_cap(int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_cap", id);
    return flux_tag_int(ch->capacity);
}

int64_t flux_chan_is_closed(int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    FluxChannel *ch = lookup_channel(id);
    if (!ch) abort_unknown("flux_chan_is_closed", id);
    FLUX_MUTEX_LOCK(&ch->mutex);
    int closed = ch->closed;
    FLUX_MUTEX_UNLOCK(&ch->mutex);
    return closed ? FLUX_TRUE : FLUX_FALSE;
}
