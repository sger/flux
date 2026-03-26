/*
 * smoke_test.c — Minimal tests for the Flux C runtime.
 *
 * Exercises GC, NaN-boxing, strings, HAMT, and basic I/O.
 * Build: see Makefile `make test`.
 */

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

static int tests_run    = 0;
static int tests_passed = 0;

#define ASSERT(cond, msg) do { \
    tests_run++; \
    if (!(cond)) { \
        fprintf(stderr, "FAIL: %s (line %d)\n", msg, __LINE__); \
    } else { \
        tests_passed++; \
    } \
} while (0)

/* ── NaN-box round-trip tests ───────────────────────────────────────── */

static void test_nanbox_int(void) {
    int64_t v0 = flux_tag_int(0);
    ASSERT(flux_untag_int(v0) == 0, "tag/untag int 0");

    int64_t v42 = flux_tag_int(42);
    ASSERT(flux_untag_int(v42) == 42, "tag/untag int 42");

    int64_t vneg = flux_tag_int(-1);
    ASSERT(flux_untag_int(vneg) == -1, "tag/untag int -1");

    int64_t vbig = flux_tag_int(1000000);
    ASSERT(flux_untag_int(vbig) == 1000000, "tag/untag int 1000000");

    int64_t vneg_big = flux_tag_int(-999999);
    ASSERT(flux_untag_int(vneg_big) == -999999, "tag/untag int -999999");
}

static void test_nanbox_bool(void) {
    int64_t t = flux_make_bool(1);
    int64_t f = flux_make_bool(0);
    ASSERT(flux_is_nanbox(t), "bool true is nanbox");
    ASSERT(flux_is_nanbox(f), "bool false is nanbox");
    ASSERT(flux_nanbox_tag(t) == FLUX_TAG_BOOLEAN, "bool true tag");
    ASSERT(flux_nanbox_tag(f) == FLUX_TAG_BOOLEAN, "bool false tag");
    ASSERT(((uint64_t)t & FLUX_PAYLOAD_MASK) == 1, "bool true payload");
    ASSERT(((uint64_t)f & FLUX_PAYLOAD_MASK) == 0, "bool false payload");
}

static void test_nanbox_none(void) {
    int64_t n = flux_make_none();
    ASSERT(flux_is_nanbox(n), "none is nanbox");
    ASSERT(flux_nanbox_tag(n) == FLUX_TAG_NONE, "none tag");
}

static void test_nanbox_ptr(void) {
    /* Allocate something and round-trip through tag/untag. */
    int64_t *obj = (int64_t *)flux_gc_alloc(16);
    obj[0] = 0xDEADBEEF;
    obj[1] = 0xCAFEBABE;

    int64_t tagged = flux_tag_ptr(obj);
    ASSERT(flux_is_ptr(tagged), "tagged ptr is_ptr");
    int64_t *recovered = (int64_t *)flux_untag_ptr(tagged);
    ASSERT(recovered == obj, "ptr round-trip");
    ASSERT(recovered[0] == 0xDEADBEEF, "ptr value preserved [0]");
    ASSERT(recovered[1] == (int64_t)0xCAFEBABE, "ptr value preserved [1]");
}

/* ── GC tests ───────────────────────────────────────────────────────── */

static void test_gc_basic(void) {
    size_t before = flux_gc_num_allocs();
    void *p = flux_gc_alloc(64);
    ASSERT(p != NULL, "gc_alloc returns non-null");
    ASSERT(flux_gc_num_allocs() == before + 1, "gc alloc count incremented");
}

/* ── String tests ───────────────────────────────────────────────────── */

static void test_string_basic(void) {
    int64_t s = flux_string_new("hello", 5);
    ASSERT(flux_is_ptr(s), "string is boxed ptr");
    ASSERT(flux_string_len(s) == 5, "string len");
    ASSERT(memcmp(flux_string_data(s), "hello", 5) == 0, "string data");
}

static void test_string_concat(void) {
    int64_t a = flux_string_new("foo", 3);
    int64_t b = flux_string_new("bar", 3);
    int64_t c = flux_string_concat(a, b);
    ASSERT(flux_string_len(c) == 6, "concat len");
    ASSERT(memcmp(flux_string_data(c), "foobar", 6) == 0, "concat data");
}

static void test_string_slice(void) {
    int64_t s = flux_string_new("hello world", 11);
    int64_t slice = flux_string_slice(s, flux_tag_int(0), flux_tag_int(5));
    ASSERT(flux_string_len(slice) == 5, "slice len");
    ASSERT(memcmp(flux_string_data(slice), "hello", 5) == 0, "slice data");
}

static void test_string_conversions(void) {
    int64_t s = flux_int_to_string(flux_tag_int(42));
    ASSERT(flux_string_len(s) == 2, "int_to_string len");
    ASSERT(memcmp(flux_string_data(s), "42", 2) == 0, "int_to_string data");

    int64_t n = flux_string_to_int(s);
    ASSERT(flux_untag_int(n) == 42, "string_to_int round-trip");
}

static void test_string_eq(void) {
    int64_t a = flux_string_new("abc", 3);
    int64_t b = flux_string_new("abc", 3);
    int64_t c = flux_string_new("xyz", 3);
    ASSERT(flux_string_eq(a, b) == 1, "equal strings");
    ASSERT(flux_string_eq(a, c) == 0, "unequal strings");
}

/* ── HAMT tests ─────────────────────────────────────────────────────── */

static void test_hamt_basic(void) {
    int64_t map = flux_hamt_empty();
    ASSERT(flux_untag_int(flux_hamt_size(map)) == 0, "empty map size");

    /* Insert key=1, value=100. */
    int64_t key1 = flux_tag_int(1);
    int64_t val1 = flux_tag_int(100);
    int64_t map2 = flux_hamt_set(map, key1, val1);

    ASSERT(flux_untag_int(flux_hamt_size(map2)) == 1, "map size after insert");

    /* Lookup. */
    int64_t got = flux_hamt_get(map2, key1);
    ASSERT(flux_untag_int(got) == 100, "hamt get existing key");

    /* Original map unchanged (persistence). */
    ASSERT(flux_untag_int(flux_hamt_size(map)) == 0, "original map unchanged");

    /* Contains. */
    ASSERT(((uint64_t)flux_hamt_contains(map2, key1) & FLUX_PAYLOAD_MASK) == 1, "contains existing");
    int64_t key2 = flux_tag_int(2);
    ASSERT(((uint64_t)flux_hamt_contains(map2, key2) & FLUX_PAYLOAD_MASK) == 0, "not contains missing");
}

static void test_hamt_multiple(void) {
    int64_t map = flux_hamt_empty();
    /* Insert 20 key-value pairs. */
    for (int i = 0; i < 20; i++) {
        map = flux_hamt_set(map, flux_tag_int(i), flux_tag_int(i * 10));
    }
    ASSERT(flux_untag_int(flux_hamt_size(map)) == 20, "map size after 20 inserts");

    /* Verify all. */
    for (int i = 0; i < 20; i++) {
        int64_t got = flux_hamt_get(map, flux_tag_int(i));
        ASSERT(flux_untag_int(got) == i * 10, "hamt get multi");
    }

    /* Delete one. */
    int64_t map2 = flux_hamt_delete(map, flux_tag_int(5));
    ASSERT(flux_untag_int(flux_hamt_size(map2)) == 19, "map size after delete");
    ASSERT(flux_untag_int(flux_hamt_size(map)) == 20, "original unchanged after delete");
}

/* ── Main ───────────────────────────────────────────────────────────── */

int main(void) {
    flux_rt_init();

    test_nanbox_int();
    test_nanbox_bool();
    test_nanbox_none();
    test_nanbox_ptr();
    test_gc_basic();
    test_string_basic();
    test_string_concat();
    test_string_slice();
    test_string_conversions();
    test_string_eq();
    test_hamt_basic();
    test_hamt_multiple();

    flux_rt_shutdown();

    printf("%d/%d tests passed\n", tests_passed, tests_run);
    return (tests_passed == tests_run) ? 0 : 1;
}
