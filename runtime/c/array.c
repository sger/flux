/*
 * array.c — Flux array primitives.
 *
 * FluxArray layout (heap-allocated via flux_gc_alloc):
 *   struct { uint32_t len; uint32_t capacity; int64_t elements[]; }
 *
 * Arrays are persistent (immutable from the Flux perspective).
 * Mutation operations return new arrays (copy-on-write).
 * All values are NaN-boxed i64.
 */

#include "flux_rt.h"
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

/* ── FluxArray layout ───────────────────────────────────────────────── */

typedef struct {
    uint8_t  obj_tag;    /* FLUX_OBJ_ARRAY */
    uint8_t  _pad[3];
    uint32_t len;
    uint32_t capacity;
    uint32_t _pad2;
    int64_t  elements[];
} FluxArray;

/* ── Helpers ────────────────────────────────────────────────────────── */

static FluxArray *array_ptr(int64_t val) {
    return (FluxArray *)flux_untag_ptr(val);
}

static int64_t array_tag(FluxArray *arr) {
    return flux_tag_ptr(arr);
}

static FluxArray *alloc_array(uint32_t capacity) {
    uint32_t size = (uint32_t)(sizeof(FluxArray) + capacity * sizeof(int64_t));
    /* scan_fsize tracks elements for recursive drop (capped at 255). */
    uint8_t scan = (capacity <= 255) ? (uint8_t)capacity : 255;
    FluxArray *arr = (FluxArray *)flux_gc_alloc_header(size, scan, FLUX_OBJ_ARRAY);
    arr->obj_tag = FLUX_OBJ_ARRAY;
    arr->len = 0;
    arr->capacity = capacity;
    return arr;
}

/* ── Public API ─────────────────────────────────────────────────────── */

int64_t flux_array_new(int64_t *elements, int32_t len) {
    FluxArray *arr = alloc_array((uint32_t)len);
    arr->len = (uint32_t)len;
    if (len > 0 && elements) {
        memcpy(arr->elements, elements, (size_t)len * sizeof(int64_t));
    }
    return array_tag(arr);
}

int64_t flux_array_len(int64_t arr_val) {
    FluxArray *arr = array_ptr(arr_val);
    if (!arr) return flux_tag_int(0);
    return flux_tag_int((int64_t)arr->len);
}

int64_t flux_array_get(int64_t arr_val, int64_t index_val) {
    FluxArray *arr = array_ptr(arr_val);
    if (!arr) return flux_make_none();
    int64_t idx = flux_untag_int(index_val);
    if (idx < 0 || (uint32_t)idx >= arr->len) return flux_make_none();
    /* Wrap in Some(value). */
    return arr->elements[idx];
}

int64_t flux_array_set(int64_t arr_val, int64_t index_val, int64_t value) {
    FluxArray *arr = array_ptr(arr_val);
    if (!arr) return arr_val;
    int64_t idx = flux_untag_int(index_val);
    if (idx < 0 || (uint32_t)idx >= arr->len) return arr_val;
    /* Copy-on-write: create new array with modified element. */
    FluxArray *new_arr = alloc_array(arr->len);
    new_arr->len = arr->len;
    memcpy(new_arr->elements, arr->elements, arr->len * sizeof(int64_t));
    new_arr->elements[idx] = value;
    return array_tag(new_arr);
}

int64_t flux_array_push(int64_t arr_val, int64_t value) {
    FluxArray *arr = array_ptr(arr_val);
    uint32_t old_len = arr ? arr->len : 0;
    FluxArray *new_arr = alloc_array(old_len + 1);
    new_arr->len = old_len + 1;
    if (arr && old_len > 0) {
        memcpy(new_arr->elements, arr->elements, old_len * sizeof(int64_t));
    }
    new_arr->elements[old_len] = value;
    return array_tag(new_arr);
}

int64_t flux_array_concat(int64_t a_val, int64_t b_val) {
    FluxArray *a = array_ptr(a_val);
    FluxArray *b = array_ptr(b_val);
    uint32_t a_len = a ? a->len : 0;
    uint32_t b_len = b ? b->len : 0;
    FluxArray *new_arr = alloc_array(a_len + b_len);
    new_arr->len = a_len + b_len;
    if (a && a_len > 0) {
        memcpy(new_arr->elements, a->elements, a_len * sizeof(int64_t));
    }
    if (b && b_len > 0) {
        memcpy(new_arr->elements + a_len, b->elements, b_len * sizeof(int64_t));
    }
    return array_tag(new_arr);
}

int64_t flux_array_slice(int64_t arr_val, int64_t start_val, int64_t end_val) {
    FluxArray *arr = array_ptr(arr_val);
    if (!arr) return flux_array_new(NULL, 0);
    int64_t start = flux_untag_int(start_val);
    int64_t end = flux_untag_int(end_val);
    if (start < 0) start = 0;
    if (end < 0) end = 0;
    if ((uint32_t)start > arr->len) start = arr->len;
    if ((uint32_t)end > arr->len) end = arr->len;
    if (start >= end) return flux_array_new(NULL, 0);
    uint32_t slice_len = (uint32_t)(end - start);
    return flux_array_new(arr->elements + start, (int32_t)slice_len);
}

int64_t flux_array_reverse(int64_t arr_val) {
    FluxArray *arr = array_ptr(arr_val);
    if (!arr || arr->len == 0) return arr_val;
    FluxArray *new_arr = alloc_array(arr->len);
    new_arr->len = arr->len;
    for (uint32_t i = 0; i < arr->len; i++) {
        new_arr->elements[i] = arr->elements[arr->len - 1 - i];
    }
    return array_tag(new_arr);
}

int64_t flux_array_contains(int64_t arr_val, int64_t value) {
    FluxArray *arr = array_ptr(arr_val);
    if (!arr) return flux_make_bool(0);
    for (uint32_t i = 0; i < arr->len; i++) {
        if (arr->elements[i] == value) return flux_make_bool(1);
        /* String equality check for boxed values. */
        if (flux_is_ptr(arr->elements[i]) && flux_is_ptr(value)) {
            if (flux_string_eq(arr->elements[i], value)) return flux_make_bool(1);
        }
    }
    return flux_make_bool(0);
}
