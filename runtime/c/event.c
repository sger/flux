#include "flux_rt.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <pthread.h>
#endif

#if defined(_WIN32) || defined(_MSC_VER)
typedef CRITICAL_SECTION flux_event_mutex_t;
#define FLUX_EVENT_MUTEX_INIT(m) InitializeCriticalSection(m)
#define FLUX_EVENT_MUTEX_LOCK(m) EnterCriticalSection(m)
#define FLUX_EVENT_MUTEX_UNLOCK(m) LeaveCriticalSection(m)
static INIT_ONCE wait_mutex_once = INIT_ONCE_STATIC_INIT;
#else
typedef pthread_mutex_t flux_event_mutex_t;
#define FLUX_EVENT_MUTEX_INIT(m) pthread_mutex_init((m), NULL)
#define FLUX_EVENT_MUTEX_LOCK(m) pthread_mutex_lock(m)
#define FLUX_EVENT_MUTEX_UNLOCK(m) pthread_mutex_unlock(m)
static pthread_once_t wait_mutex_once = PTHREAD_ONCE_INIT;
#endif

typedef enum {
    EV_FREE,
    EV_RECV,
    EV_SEND,
    EV_SEND_MOVE,
    EV_AFTER,
    EV_ALWAYS,
    EV_NEVER,
    EV_CHOOSE,
    EV_WRAP,
} FluxEventKind;

typedef struct {
    FluxEventKind kind;
    int64_t a;
    int64_t b;
    int64_t *ids;
    size_t len;
    int64_t next_free;
} FluxEvent;

enum { FLUX_LIST_CONS_TAG = 4 };

static FluxEvent *events = NULL;
static size_t event_capacity = 0;
static int64_t next_event_id = 1;
static int64_t free_event_id = 0;
static flux_event_mutex_t wait_mutex;
static uint64_t *active_waits = NULL;
static size_t active_wait_len = 0;
static size_t active_wait_cap = 0;

static int64_t retain_value(int64_t value) {
    flux_dup(value);
    return value;
}

static uint64_t now_ms(void) {
#ifdef _WIN32
    return (uint64_t)GetTickCount64();
#else
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000ULL + (uint64_t)ts.tv_nsec / 1000000ULL;
#endif
}

static FluxEvent *lookup_event(int64_t id) {
    if (id < 1 || id >= next_event_id || (size_t)id > event_capacity) return NULL;
    if (events[id - 1].kind == EV_FREE) return NULL;
    return &events[id - 1];
}

static void ensure_event_capacity(int64_t id) {
    if (id < 1) abort();
    if ((size_t)id <= event_capacity) return;

    size_t next = event_capacity == 0 ? 1024 : event_capacity;
    while ((size_t)id > next) next *= 2;

    FluxEvent *grown = (FluxEvent *)realloc(events, next * sizeof(FluxEvent));
    if (!grown) {
        fprintf(stderr, "flux_event: out of memory growing event table\n");
        abort();
    }
    memset(grown + event_capacity, 0, (next - event_capacity) * sizeof(FluxEvent));
    events = grown;
    event_capacity = next;
}

#if defined(_WIN32) || defined(_MSC_VER)
static BOOL CALLBACK wait_mutex_do_init(PINIT_ONCE once, PVOID parameter, PVOID *context) {
    (void)once;
    (void)parameter;
    (void)context;
    FLUX_EVENT_MUTEX_INIT(&wait_mutex);
    return TRUE;
}
#else
static void wait_mutex_do_init(void) {
    FLUX_EVENT_MUTEX_INIT(&wait_mutex);
}
#endif

static void wait_mutex_init_once(void) {
#if defined(_WIN32) || defined(_MSC_VER)
    InitOnceExecuteOnce(&wait_mutex_once, wait_mutex_do_init, NULL, NULL);
#else
    pthread_once(&wait_mutex_once, wait_mutex_do_init);
#endif
}

static int64_t insert_event(FluxEvent ev) {
    int64_t id = 0;
    if (free_event_id != 0) {
        id = free_event_id;
        FluxEvent *slot = &events[id - 1];
        free_event_id = slot->next_free;
    } else {
        id = next_event_id++;
        ensure_event_capacity(id);
    }
    ev.next_free = 0;
    events[id - 1] = ev;
    return flux_tag_int(id);
}

static void active_wait_insert(uint64_t req) {
    wait_mutex_init_once();
    FLUX_EVENT_MUTEX_LOCK(&wait_mutex);
    for (size_t i = 0; i < active_wait_len; i++) {
        if (active_waits[i] == req) {
            FLUX_EVENT_MUTEX_UNLOCK(&wait_mutex);
            return;
        }
    }
    if (active_wait_len == active_wait_cap) {
        size_t next = active_wait_cap == 0 ? 64 : active_wait_cap * 2;
        uint64_t *grown = (uint64_t *)realloc(active_waits, next * sizeof(uint64_t));
        if (!grown) {
            fprintf(stderr, "flux_event_wait: out of memory growing wait table\n");
            abort();
        }
        active_waits = grown;
        active_wait_cap = next;
    }
    active_waits[active_wait_len++] = req;
    FLUX_EVENT_MUTEX_UNLOCK(&wait_mutex);
}

void flux_event_complete_wait(uint64_t request_id) {
    wait_mutex_init_once();
    FLUX_EVENT_MUTEX_LOCK(&wait_mutex);
    for (size_t i = 0; i < active_wait_len; i++) {
        if (active_waits[i] == request_id) {
            active_waits[i] = active_waits[active_wait_len - 1];
            active_wait_len--;
            FLUX_EVENT_MUTEX_UNLOCK(&wait_mutex);
            flux_async_task_complete(request_id, FLUX_NONE);
            return;
        }
    }
    FLUX_EVENT_MUTEX_UNLOCK(&wait_mutex);
}

int flux_event_wait_is_active(uint64_t request_id) {
    int active = 0;
    wait_mutex_init_once();
    FLUX_EVENT_MUTEX_LOCK(&wait_mutex);
    for (size_t i = 0; i < active_wait_len; i++) {
        if (active_waits[i] == request_id) {
            active = 1;
            break;
        }
    }
    FLUX_EVENT_MUTEX_UNLOCK(&wait_mutex);
    return active;
}

static int collect_ids(int64_t list, int64_t **out_ids, size_t *out_len) {
    size_t len = 0;
    size_t cap = 8;
    int64_t *ids = (int64_t *)malloc(cap * sizeof(int64_t));
    if (!ids) return 0;

    int64_t cur = list;
    while (cur != FLUX_EMPTY_LIST && cur != FLUX_NONE) {
        if (!flux_is_ptr(cur)) {
            free(ids);
            return 0;
        }
        void *ptr = flux_untag_ptr(cur);
        if (flux_obj_tag(ptr) != FLUX_OBJ_ADT) {
            free(ids);
            return 0;
        }
        int32_t ctor_tag = *(int32_t *)ptr;
        int32_t field_count = *((int32_t *)ptr + 1);
        // Flux lists are ADT cons cells: Cons(head, tail).
        if (ctor_tag != FLUX_LIST_CONS_TAG || field_count != 2) {
            free(ids);
            return 0;
        }
        int64_t *fields = (int64_t *)((char *)ptr + 8);
        if (len == cap) {
            cap *= 2;
            int64_t *next = (int64_t *)realloc(ids, cap * sizeof(int64_t));
            if (!next) {
                free(ids);
                return 0;
            }
            ids = next;
        }
        ids[len++] = flux_untag_int(fields[0]);
        cur = fields[1];
    }

    *out_ids = ids;
    *out_len = len;
    return 1;
}

static int poll_event(int64_t id, int64_t *out) {
    FluxEvent *ev = lookup_event(id);
    if (!ev) {
        fprintf(stderr, "flux_event_sync: unknown event %lld\n", (long long)id);
        abort();
    }

    switch (ev->kind) {
    case EV_RECV: {
        int64_t value = flux_chan_try_recv(flux_tag_int(ev->a));
        if (value != FLUX_NONE) {
            *out = value;
            return 1;
        }
        if (flux_chan_is_closed(flux_tag_int(ev->a)) == FLUX_TRUE) {
            *out = FLUX_NONE;
            return 1;
        }
        return 0;
    }
    case EV_SEND:
        if (flux_chan_try_send(flux_tag_int(ev->a), ev->b) == FLUX_TRUE ||
            flux_chan_is_closed(flux_tag_int(ev->a)) == FLUX_TRUE) {
            *out = FLUX_NONE;
            return 1;
        }
        return 0;
    case EV_SEND_MOVE:
        if (flux_chan_is_closed(flux_tag_int(ev->a)) == FLUX_TRUE) {
            flux_drop(ev->b);
            ev->b = FLUX_NONE;
            *out = FLUX_NONE;
            return 1;
        }
        if (flux_chan_event_can_send(ev->a) != 0 &&
            flux_chan_try_send_move(flux_tag_int(ev->a), ev->b) == FLUX_TRUE) {
            ev->b = FLUX_NONE;
            *out = FLUX_NONE;
            return 1;
        }
        return 0;
    case EV_AFTER:
        if (now_ms() >= (uint64_t)ev->a) {
            *out = FLUX_NONE;
            return 1;
        }
        return 0;
    case EV_ALWAYS:
        *out = retain_value(ev->a);
        return 1;
    case EV_NEVER:
        return 0;
    case EV_CHOOSE:
        for (size_t i = 0; i < ev->len; i++) {
            if (poll_event(ev->ids[i], out)) return 1;
        }
        return 0;
    case EV_WRAP: {
        int64_t inner = 0;
        if (!poll_event(ev->a, &inner)) return 0;
        int64_t args[1] = { inner };
        *out = flux_call_closure_c(ev->b, args, 1);
        flux_drop(inner);
        return 1;
    }
    }
    return 0;
}

static int event_ready(int64_t id) {
    FluxEvent *ev = lookup_event(id);
    if (!ev) {
        fprintf(stderr, "flux_event_wait: unknown event %lld\n", (long long)id);
        abort();
    }

    switch (ev->kind) {
    case EV_RECV:
        return flux_chan_event_can_recv(ev->a) != 0;
    case EV_SEND:
    case EV_SEND_MOVE:
        return flux_chan_event_can_send(ev->a) != 0;
    case EV_AFTER:
        return now_ms() >= (uint64_t)ev->a;
    case EV_ALWAYS:
        return 1;
    case EV_NEVER:
        return 0;
    case EV_CHOOSE:
        for (size_t i = 0; i < ev->len; i++) {
            if (event_ready(ev->ids[i])) return 1;
        }
        return 0;
    case EV_WRAP:
        return event_ready(ev->a);
    case EV_FREE:
        return 0;
    }
    return 0;
}

typedef struct {
    uint64_t request_id;
    uint64_t deadline_ms;
} EventTimerWait;

static void sleep_until_ms(uint64_t deadline_ms) {
    for (;;) {
        uint64_t now = now_ms();
        if (now >= deadline_ms) return;
        uint64_t remaining = deadline_ms - now;
#ifdef _WIN32
        Sleep((DWORD)remaining);
#else
        struct timespec ts;
        ts.tv_sec = (time_t)(remaining / 1000ULL);
        ts.tv_nsec = (long)((remaining % 1000ULL) * 1000000ULL);
        nanosleep(&ts, NULL);
#endif
    }
}

#ifdef _WIN32
static DWORD WINAPI event_timer_thread(LPVOID arg) {
    EventTimerWait *wait = (EventTimerWait *)arg;
    sleep_until_ms(wait->deadline_ms);
    flux_event_complete_wait(wait->request_id);
    free(wait);
    return 0;
}
#else
static void *event_timer_thread(void *arg) {
    EventTimerWait *wait = (EventTimerWait *)arg;
    sleep_until_ms(wait->deadline_ms);
    flux_event_complete_wait(wait->request_id);
    free(wait);
    return NULL;
}
#endif

static void spawn_event_timer(uint64_t request_id, uint64_t deadline_ms) {
    // TODO: replace one native thread per timer event with the async runtime's
    // timer queue once event waits share that scheduler path.
    EventTimerWait *wait = (EventTimerWait *)malloc(sizeof(EventTimerWait));
    if (!wait) {
        fprintf(stderr, "flux_event_wait: out of memory allocating timer wait\n");
        abort();
    }
    wait->request_id = request_id;
    wait->deadline_ms = deadline_ms;
#ifdef _WIN32
    HANDLE thread = CreateThread(NULL, 0, event_timer_thread, wait, 0, NULL);
    if (!thread) {
        free(wait);
        fprintf(stderr, "flux_event_wait: failed to create timer thread\n");
        abort();
    }
    CloseHandle(thread);
#else
    pthread_t thread;
    if (pthread_create(&thread, NULL, event_timer_thread, wait) != 0) {
        free(wait);
        fprintf(stderr, "flux_event_wait: failed to create timer thread\n");
        abort();
    }
    pthread_detach(thread);
#endif
}

static void register_event_wait(int64_t id, uint64_t request_id) {
    FluxEvent *ev = lookup_event(id);
    if (!ev) {
        fprintf(stderr, "flux_event_wait: unknown event %lld\n", (long long)id);
        abort();
    }

    switch (ev->kind) {
    case EV_RECV:
        flux_chan_event_watch_recv(ev->a, request_id);
        break;
    case EV_SEND:
    case EV_SEND_MOVE:
        flux_chan_event_watch_send(ev->a, request_id);
        break;
    case EV_AFTER:
        if (now_ms() >= (uint64_t)ev->a) {
            flux_event_complete_wait(request_id);
        } else {
            spawn_event_timer(request_id, (uint64_t)ev->a);
        }
        break;
    case EV_ALWAYS:
        flux_event_complete_wait(request_id);
        break;
    case EV_NEVER:
        break;
    case EV_CHOOSE:
        for (size_t i = 0; i < ev->len; i++) {
            register_event_wait(ev->ids[i], request_id);
        }
        break;
    case EV_WRAP:
        register_event_wait(ev->a, request_id);
        break;
    case EV_FREE:
        break;
    }
}

static void free_event_tree(int64_t id) {
    FluxEvent *ev = lookup_event(id);
    if (!ev) return;

    FluxEvent old = *ev;
    memset(ev, 0, sizeof(*ev));
    ev->kind = EV_FREE;
    ev->next_free = free_event_id;
    free_event_id = id;

    switch (old.kind) {
    case EV_SEND:
    case EV_SEND_MOVE:
    case EV_WRAP:
        flux_drop(old.b);
        if (old.kind == EV_WRAP) free_event_tree(old.a);
        break;
    case EV_ALWAYS:
        flux_drop(old.a);
        break;
    case EV_CHOOSE:
        for (size_t i = 0; i < old.len; i++) {
            free_event_tree(old.ids[i]);
        }
        free(old.ids);
        break;
    default:
        free(old.ids);
        break;
    }
}

int64_t flux_event_recv(int64_t ch) {
    FluxEvent ev = { EV_RECV, flux_untag_int(ch), 0, NULL, 0 };
    return insert_event(ev);
}

int64_t flux_event_send(int64_t ch, int64_t value) {
    FluxEvent ev = { EV_SEND, flux_untag_int(ch), retain_value(value), NULL, 0 };
    return insert_event(ev);
}

int64_t flux_event_send_move(int64_t ch, int64_t value) {
    FluxEvent ev = {
        EV_SEND_MOVE,
        flux_untag_int(ch),
        flux_rc_is_unique(value) ? value : retain_value(value),
        NULL,
        0
    };
    return insert_event(ev);
}

int64_t flux_event_after(int64_t ms_val) {
    int64_t ms = flux_untag_int(ms_val);
    if (ms < 0) ms = 0;
    FluxEvent ev = { EV_AFTER, (int64_t)(now_ms() + (uint64_t)ms), 0, NULL, 0 };
    return insert_event(ev);
}

int64_t flux_event_always(int64_t value) {
    FluxEvent ev = { EV_ALWAYS, retain_value(value), 0, NULL, 0 };
    return insert_event(ev);
}

int64_t flux_event_never(void) {
    FluxEvent ev = { EV_NEVER, 0, 0, NULL, 0 };
    return insert_event(ev);
}

int64_t flux_event_choose(int64_t ids_list) {
    int64_t *ids = NULL;
    size_t len = 0;
    if (!collect_ids(ids_list, &ids, &len) || len == 0) {
        fprintf(stderr, "flux_event_choose: expected non-empty List<Int>\n");
        abort();
    }
    FluxEvent ev = { EV_CHOOSE, 0, 0, ids, len };
    return insert_event(ev);
}

int64_t flux_event_wrap(int64_t id_val, int64_t closure) {
    FluxEvent ev = { EV_WRAP, flux_untag_int(id_val), retain_value(closure), NULL, 0 };
    return insert_event(ev);
}

int64_t flux_event_sync(int64_t id_val) {
    (void)id_val;
    fprintf(stderr, "flux_event_sync is deprecated; Flow.Event.sync uses flux_event_poll\n");
    abort();
}

int64_t flux_event_poll(int32_t ready_tag, int32_t pending_tag, int64_t id_val) {
    int64_t id = flux_untag_int(id_val);
    int64_t out = 0;
    if (poll_event(id, &out)) {
        free_event_tree(id);
        return flux_async_make_adt1(ready_tag, out);
    }
    return flux_async_make_adt0(pending_tag);
}

int64_t flux_event_wait(int64_t id_val) {
    uint64_t request_id = flux_async_task_await_request();
    if (request_id == 0) {
        fprintf(stderr, "flux_event_wait: requires active async scheduler\n");
        abort();
    }

    int64_t id = flux_untag_int(id_val);
    active_wait_insert(request_id);
    if (event_ready(id)) {
        flux_event_complete_wait(request_id);
    } else {
        register_event_wait(id, request_id);
    }
    return flux_async_suspend_request(request_id);
}
