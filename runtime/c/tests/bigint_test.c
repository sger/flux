/*
 * bigint_test.c — the runtime reads a BIGINT box as the Int it holds
 * (KI-039).
 *
 * Nothing in the runtime makes a box yet, so these cases make them with
 * flux_make_int, which every producer will call after the flip.
 */

#include "flux_test.h"

#include <limits.h>

#define INLINE_MAX FLUX_MAX_INLINE_INT
#define INLINE_MIN FLUX_MIN_INLINE_INT

static int is_box(int64_t v) {
    return flux_is_ptr(v) && flux_obj_tag(flux_untag_ptr(v)) == FLUX_OBJ_BIGINT;
}

TEST(canonical_form) {
    const int64_t inline_values[] = {0, 1, -1, INLINE_MAX, INLINE_MIN};
    for (size_t i = 0; i < sizeof(inline_values) / sizeof(inline_values[0]); i++) {
        int64_t v = flux_make_int(inline_values[i]);
        ASSERT_EQ_WORD(v, flux_tag_int(inline_values[i]));
        ASSERT_EQ_I64(flux_untag_int(v), inline_values[i]);
    }
    const int64_t boxed_values[] = {INLINE_MAX + 1, INLINE_MIN - 1, INT64_MAX, INT64_MIN};
    for (size_t i = 0; i < sizeof(boxed_values) / sizeof(boxed_values[0]); i++) {
        int64_t v = flux_make_int(boxed_values[i]);
        ASSERT_TRUE(is_box(v));
        ASSERT_EQ_I64(flux_untag_int(v), boxed_values[i]);
        flux_drop(v);
    }
}

TEST(a_box_is_an_int) {
    int64_t v = flux_make_int(INT64_MAX);
    ASSERT_TRUE(flux_is_int(v));
    ASSERT_TRUE(flux_is_bigint(v));
    ASSERT_TRUE(flux_is_ptr(v));
    ASSERT_TRUE(!flux_val_is_float(v));
    ASSERT_TRUE(!flux_is_bigint(flux_tag_int(5)));
    ASSERT_TRUE(!flux_is_bigint(FLUX_TRUE));
    flux_drop(v);
}

TEST(equal_by_value) {
    int64_t a = flux_make_int(INT64_MAX);
    int64_t b = flux_make_int(INT64_MAX);
    int64_t c = flux_make_int(INT64_MIN);
    int64_t s = flux_string_new("9223372036854775807", 19);
    ASSERT_TRUE(a != b);
    ASSERT_EQ_WORD(flux_rt_eq(a, b), FLUX_TRUE);
    ASSERT_EQ_WORD(flux_rt_neq(a, b), FLUX_FALSE);
    ASSERT_EQ_WORD(flux_rt_eq(a, c), FLUX_FALSE);
    ASSERT_EQ_WORD(flux_rt_eq(a, flux_tag_int(INLINE_MAX)), FLUX_FALSE);
    ASSERT_EQ_WORD(flux_rt_eq(flux_tag_int(5), a), FLUX_FALSE);
    ASSERT_EQ_WORD(flux_rt_eq(a, s), FLUX_FALSE);
    flux_drop(a);
    flux_drop(b);
    flux_drop(c);
    flux_drop(s);
}

TEST(ordered_by_value) {
    int64_t max = flux_make_int(INT64_MAX);
    int64_t max2 = flux_make_int(INT64_MAX);
    int64_t min = flux_make_int(INT64_MIN);
    int64_t zero = flux_tag_int(0);
    ASSERT_EQ_WORD(flux_rt_lt(min, zero), FLUX_TRUE);
    ASSERT_EQ_WORD(flux_rt_lt(zero, min), FLUX_FALSE);
    ASSERT_EQ_WORD(flux_rt_gt(max, flux_tag_int(INLINE_MAX)), FLUX_TRUE);
    ASSERT_EQ_WORD(flux_rt_le(max, max2), FLUX_TRUE);
    ASSERT_EQ_WORD(flux_rt_ge(max, max2), FLUX_TRUE);
    ASSERT_EQ_WORD(flux_rt_lt(max, max2), FLUX_FALSE);
    ASSERT_EQ_WORD(flux_rt_gt(min, max), FLUX_FALSE);
    flux_drop(max);
    flux_drop(max2);
    flux_drop(min);
}

TEST(map_key_by_value) {
    int64_t key = flux_make_int(INT64_MAX);
    int64_t same = flux_make_int(INT64_MAX);
    int64_t other = flux_make_int(INT64_MIN);
    int64_t map = flux_hamt_empty();
    int64_t map1 = flux_hamt_set(map, key, flux_tag_int(1));
    int64_t map2 = flux_hamt_set(map1, flux_tag_int(7), flux_tag_int(2));
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_size(map2)), 2);
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_get(map2, same)), 1);
    ASSERT_EQ_WORD(flux_hamt_contains(map2, same), FLUX_TRUE);
    ASSERT_EQ_WORD(flux_hamt_contains(map2, other), FLUX_FALSE);
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_get(map2, flux_tag_int(7))), 2);
    /* Setting the same value through another box replaces the entry. */
    int64_t map3 = flux_hamt_set(map2, same, flux_tag_int(3));
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_size(map3)), 2);
    ASSERT_EQ_I64(flux_untag_int(flux_hamt_get(map3, key)), 3);
    flux_drop(map3);
    flux_drop(map2);
    flux_drop(map1);
    flux_drop(map);
    flux_drop(key);
    flux_drop(same);
    flux_drop(other);
}

TEST(prints_as_its_value) {
    int64_t max = flux_make_int(INT64_MAX);
    int64_t min = flux_make_int(INT64_MIN);
    int64_t s1 = flux_to_string(max);
    int64_t s2 = flux_to_string(min);
    int64_t s3 = flux_int_to_string(max);
    int64_t ty = flux_type_of(max);
    ASSERT_STR(s1, "9223372036854775807");
    ASSERT_STR(s2, "-9223372036854775808");
    ASSERT_STR(s3, "9223372036854775807");
    ASSERT_STR(ty, "Int");
    flux_drop(s1);
    flux_drop(s2);
    flux_drop(s3);
    flux_drop(ty);
    flux_drop(max);
    flux_drop(min);
}

/* An ADT value { ctor_tag, field_count, fields[] } that owns its fields. */
static int64_t make_adt(int32_t ctor_tag, int32_t count, const int64_t *fields) {
    void *mem = flux_gc_alloc_header((uint32_t)(8 + 8 * count), (uint8_t)count, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = ctor_tag;
    hdr[1] = count;
    memcpy((char *)mem + 8, fields, (size_t)count * sizeof(int64_t));
    return flux_tag_ptr(mem);
}

/* JsonNumber(JsonInt(box)), with the Json constructors numbered 0..7 in the
 * order flux_json_stringify takes their tags. */
TEST(json_writes_its_value) {
    int64_t box = flux_make_int(INT64_MIN);
    int64_t json_int = make_adt(3, 1, &box);
    int64_t json_number = make_adt(2, 1, &json_int);
    int64_t text = flux_json_stringify(0, 1, 2, 3, 4, 5, 6, 7, json_number);
    ASSERT_STR(text, "-9223372036854775808");
    flux_drop(text);
    flux_drop(json_number);
}

TEST(dup_and_drop) {
    int64_t v = flux_make_int(INT64_MAX);
    ASSERT_TRUE(flux_rc_is_unique(v));
    flux_dup(v);
    ASSERT_TRUE(!flux_rc_is_unique(v));
    flux_drop(v);
    ASSERT_TRUE(flux_rc_is_unique(v));
    ASSERT_EQ_I64(flux_untag_int(v), INT64_MAX);
    flux_drop(v);
}

FLUX_TESTS_BEGIN
    CASE(canonical_form)
    CASE(a_box_is_an_int)
    CASE(equal_by_value)
    CASE(ordered_by_value)
    CASE(map_key_by_value)
    CASE(prints_as_its_value)
    CASE(json_writes_its_value)
    CASE(dup_and_drop)
FLUX_TESTS_END
