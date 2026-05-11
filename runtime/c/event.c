#include "flux_rt.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#ifdef _WIN32
#include <windows.h>
#endif

typedef enum {
    EV_FREE,
    EV_RECV,
    EV_SEND,
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
} FluxEvent;

enum { FLUX_LIST_CONS_TAG = 4 };

static FluxEvent *events = NULL;
static size_t event_capacity = 0;
static int64_t next_event_id = 1;

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

static int64_t insert_event(FluxEvent ev) {
    int64_t id = next_event_id++;
    ensure_event_capacity(id);
    events[id - 1] = ev;
    return flux_tag_int(id);
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

static void free_event_tree(int64_t id) {
    FluxEvent *ev = lookup_event(id);
    if (!ev) return;

    FluxEvent old = *ev;
    memset(ev, 0, sizeof(*ev));
    ev->kind = EV_FREE;

    switch (old.kind) {
    case EV_SEND:
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
