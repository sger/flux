/*
 * smoke_test.c — tagging, allocation, strings, the HAMT and stackless drop.
 */

#include "flux_test.h"

/* ── Tagging ────────────────────────────────────────────────────────── */

TEST(tag_int) {
    ASSERT_EQ_I64(flux_untag_int(flux_tag_int(0)), 0);
    ASSERT_EQ_I64(flux_untag_int(flux_tag_int(42)), 42);
    ASSERT_EQ_I64(flux_untag_int(flux_tag_int(-1)), -1);
    ASSERT_EQ_I64(flux_untag_int(flux_tag_int(1000000)), 1000000);
    ASSERT_EQ_I64(flux_untag_int(flux_tag_int(-999999)), -999999);
}

TEST(tag_bool) {
    int64_t t = flux_make_bool(1);
    int64_t f = flux_make_bool(0);
    ASSERT_EQ_WORD(t, FLUX_TRUE);
    ASSERT_EQ_WORD(f, FLUX_FALSE);
    ASSERT_TRUE(!flux_is_ptr(t) && !flux_is_int(t));
    ASSERT_TRUE(!flux_is_ptr(f) && !flux_is_int(f));
}

TEST(tag_none) {
    int64_t n = flux_make_none();
    ASSERT_EQ_WORD(n, FLUX_NONE);
    ASSERT_TRUE(!flux_is_ptr(n) && !flux_is_int(n));
}

TEST(tag_ptr) {
    int64_t *obj = (int64_t *)flux_gc_alloc(16);
    obj[0] = 0xDEADBEEF;
    obj[1] = 0xCAFEBABE;

    int64_t tagged = flux_tag_ptr(obj);
    ASSERT_TRUE(flux_is_ptr(tagged));
    int64_t *recovered = (int64_t *)flux_untag_ptr(tagged);
    ASSERT_TRUE(recovered == obj);
    ASSERT_EQ_I64(recovered[0], 0xDEADBEEF);
    ASSERT_EQ_I64(recovered[1], (int64_t)0xCAFEBABE);
}

TEST(gc_alloc) {
    ASSERT_TRUE(flux_gc_alloc(64) != NULL);
}

/* ── Strings ────────────────────────────────────────────────────────── */

TEST(string_new) {
    int64_t s = flux_string_new("hello", 5);
    ASSERT_TRUE(flux_is_ptr(s));
    ASSERT_STR(s, "hello");
}

TEST(string_concat) {
    int64_t c = flux_string_concat(flux_string_new("foo", 3), flux_string_new("bar", 3));
    ASSERT_STR(c, "foobar");
}

TEST(string_slice) {
    int64_t s = flux_string_new("hello world", 11);
    ASSERT_STR(flux_string_slice(s, flux_tag_int(0), flux_tag_int(5)), "hello");
}

TEST(string_int_conversions) {
    int64_t s = flux_int_to_string(flux_tag_int(42));
    ASSERT_STR(s, "42");
    ASSERT_EQ_I64(flux_untag_int(flux_string_to_int(s)), 42);
}

TEST(string_eq) {
    int64_t a = flux_string_new("abc", 3);
    ASSERT_EQ_I64(flux_string_eq(a, flux_string_new("abc", 3)), 1);
    ASSERT_EQ_I64(flux_string_eq(a, flux_string_new("xyz", 3)), 0);
}

/* ── HAMT ───────────────────────────────────────────────────────────── */

TEST(hamt_set_get_contains) {
    int64_t map = flux_hamt_empty();
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_size(map)), 0);

    int64_t key1 = flux_tag_int(1);
    int64_t map2 = flux_hamt_set(map, key1, flux_tag_int(100));
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_size(map2)), 1);
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_get(map2, key1)), 100);
    /* Persistence: the original map is unchanged. */
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_size(map)), 0);

    ASSERT_EQ_WORD(flux_hamt_contains(map2, key1), FLUX_TRUE);
    ASSERT_EQ_WORD(flux_hamt_contains(map2, flux_tag_int(2)), FLUX_FALSE);
}

TEST(hamt_many_keys_and_delete) {
    int64_t map = flux_hamt_empty();
    for (int i = 0; i < 20; i++) {
        map = flux_hamt_set(map, flux_tag_int(i), flux_tag_int(i * 10));
    }
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_size(map)), 20);
    for (int i = 0; i < 20; i++) {
        ASSERT_EQ_I64(flux_untag_int(flux_hamt_get(map, flux_tag_int(i))), i * 10);
    }

    int64_t map2 = flux_hamt_delete(map, flux_tag_int(5));
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_size(map2)), 19);
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_size(map)), 20);
}

/* ── Stackless drop ─────────────────────────────────────────────────── */

/* A Cons cell: ADT { ctor_tag=4, field_count=2, fields=[head, tail] }. */
static int64_t make_cons(int64_t head, int64_t tail) {
    void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = 4;
    hdr[1] = 2;
    int64_t *fields = (int64_t *)((char *)mem + 8);
    fields[0] = head;
    fields[1] = tail;
    return flux_tag_ptr(mem);
}

static int64_t make_list(int n) {
    int64_t list = FLUX_EMPTY_LIST;
    for (int i = n - 1; i >= 0; i--) {
        list = make_cons(flux_tag_int(i), list);
    }
    return list;
}

/* A binary tree node: ADT { ctor_tag=5, field_count=2, fields=[left, right] }. */
static int64_t make_tree(int depth) {
    if (depth <= 0) return FLUX_NONE;
    int64_t left = make_tree(depth - 1);
    int64_t right = make_tree(depth - 1);
    void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = 5;
    hdr[1] = 2;
    int64_t *fields = (int64_t *)((char *)mem + 8);
    fields[0] = left;
    fields[1] = right;
    return flux_tag_ptr(mem);
}

/* A recursive drop of 200K cells would overflow the stack. */
TEST(drop_deep_list) {
    int64_t list = make_list(200000);
    ASSERT_TRUE(flux_is_ptr(list));
    flux_drop(list);
}

TEST(drop_shared_list_keeps_it) {
    int64_t list = make_list(100);
    flux_dup(list);
    flux_drop(list);
    int64_t *fields = (int64_t *)((char *)flux_untag_ptr(list) + 8);
    ASSERT_EQ_I64(flux_untag_int(fields[0]), 0);
    flux_drop(list);
}

/* Depth 18 is 262143 nodes, each with two children to walk. */
TEST(drop_deep_tree) {
    int64_t tree = make_tree(18);
    ASSERT_TRUE(flux_is_ptr(tree));
    flux_drop(tree);
}

/* 200K nested single-field ADTs, like Some(Some(…)). */
TEST(drop_deep_single_field_chain) {
    int64_t val = flux_tag_int(42);
    for (int i = 0; i < 200000; i++) {
        void *mem = flux_gc_alloc_header(8 + 1 * 8, 1, FLUX_OBJ_ADT);
        int32_t *hdr = (int32_t *)mem;
        hdr[0] = 1;
        hdr[1] = 1;
        int64_t *fields = (int64_t *)((char *)mem + 8);
        fields[0] = val;
        val = flux_tag_ptr(mem);
    }
    flux_drop(val);
}

FLUX_TESTS_BEGIN
    CASE(tag_int)
    CASE(tag_bool)
    CASE(tag_none)
    CASE(tag_ptr)
    CASE(gc_alloc)
    CASE(string_new)
    CASE(string_concat)
    CASE(string_slice)
    CASE(string_int_conversions)
    CASE(string_eq)
    CASE(hamt_set_get_contains)
    CASE(hamt_many_keys_and_delete)
    CASE(drop_deep_list)
    CASE(drop_shared_list_keeps_it)
    CASE(drop_deep_tree)
    CASE(drop_deep_single_field_chain)
FLUX_TESTS_END
