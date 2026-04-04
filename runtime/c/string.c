/*
 * string.c — Flux string helpers.
 *
 * FluxString layout (heap-allocated via flux_gc_alloc):
 *   struct { uint32_t len; char data[]; }
 *
 * Strings are immutable.  All operations return new string values.
 * The pointer-tagged i64 representation uses the heap pointer directly
 * (LSB=0, value >= 12) to the FluxString struct payload.
 */

#include "flux_rt.h"
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

/* ── FluxString layout ──────────────────────────────────────────────── */

typedef struct {
    uint8_t  obj_tag;    /* FLUX_OBJ_STRING */
    uint8_t  _pad[3];
    uint32_t len;
    char     data[];
} FluxString;

/* ── Helpers ────────────────────────────────────────────────────────── */

static FluxString *string_ptr(int64_t val) {
    void *ptr = flux_untag_ptr(val);
    return (FluxString *)ptr;
}

/* ── Public API ─────────────────────────────────────────────────────── */

int64_t flux_string_new(const char *data, uint32_t len) {
    uint32_t alloc_size = (uint32_t)(sizeof(FluxString) + len);
    void *mem = flux_gc_alloc_header(alloc_size, 0, FLUX_OBJ_STRING);
    FluxString *s = (FluxString *)mem;
    s->obj_tag = FLUX_OBJ_STRING;
    s->len = len;
    if (len > 0 && data) {
        memcpy(s->data, data, len);
    }
    return flux_tag_ptr(mem);
}

const char *flux_string_data(int64_t val) {
    FluxString *s = string_ptr(val);
    if (!s) return "";
    return s->data;
}

uint32_t flux_string_len(int64_t val) {
    FluxString *s = string_ptr(val);
    if (!s) return 0;
    return s->len;
}

int64_t flux_string_length(int64_t val) {
    return flux_tag_int((int64_t)flux_string_len(val));
}

int64_t flux_string_concat(int64_t a, int64_t b) {
    FluxString *sa = string_ptr(a);
    FluxString *sb = string_ptr(b);
    if (!sa && !sb) return flux_string_new("", 0);
    if (!sa) return b;
    if (!sb) return a;

    uint32_t new_len = sa->len + sb->len;
    uint32_t alloc_size = (uint32_t)(sizeof(FluxString) + new_len);
    void *mem = flux_gc_alloc_header(alloc_size, 0, FLUX_OBJ_STRING);
    FluxString *result = (FluxString *)mem;
    result->obj_tag = FLUX_OBJ_STRING;
    result->len = new_len;
    memcpy(result->data, sa->data, sa->len);
    memcpy(result->data + sa->len, sb->data, sb->len);
    return flux_tag_ptr(mem);
}

int64_t flux_string_slice(int64_t s, int64_t start_val, int64_t end_val) {
    FluxString *str = string_ptr(s);
    if (!str) return flux_string_new("", 0);

    int64_t start = flux_untag_int(start_val);
    int64_t end   = flux_untag_int(end_val);

    /* Clamp bounds. */
    if (start < 0) start = 0;
    if (end < 0) end = 0;
    if ((uint32_t)start > str->len) start = str->len;
    if ((uint32_t)end > str->len) end = str->len;
    if (start >= end) return flux_string_new("", 0);

    uint32_t slice_len = (uint32_t)(end - start);
    return flux_string_new(str->data + start, slice_len);
}

int64_t flux_int_to_string(int64_t n) {
    int64_t raw = flux_untag_int(n);
    char buf[32];
    int len = snprintf(buf, sizeof(buf), "%lld", (long long)raw);
    if (len < 0) len = 0;
    return flux_string_new(buf, (uint32_t)len);
}

int64_t flux_float_to_string(int64_t f) {
    double d = flux_unbox_float(f);
    char buf[64];
    int len = snprintf(buf, sizeof(buf), "%.15g", d);
    if (len < 0) len = 0;
    return flux_string_new(buf, (uint32_t)len);
}

int64_t flux_string_to_int(int64_t s) {
    FluxString *str = string_ptr(s);
    if (!str || str->len == 0) return flux_tag_int(0);

    /* Null-terminate for strtoll. */
    char buf[64];
    uint32_t copy_len = str->len < 63 ? str->len : 63;
    memcpy(buf, str->data, copy_len);
    buf[copy_len] = '\0';

    char *endptr;
    long long val = strtoll(buf, &endptr, 10);
    if (endptr == buf) return flux_make_none(); /* parse failure */
    return flux_tag_int((int64_t)val);
}

int flux_string_eq(int64_t a, int64_t b) {
    FluxString *sa = string_ptr(a);
    FluxString *sb = string_ptr(b);
    if (sa == sb) return 1;
    if (!sa || !sb) return 0;
    if (sa->len != sb->len) return 0;
    return memcmp(sa->data, sb->data, sa->len) == 0;
}
