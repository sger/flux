/*
 * int_arith_test.c — the C runtime's Int arithmetic wraps, as on the VM
 * (KI-219).
 *
 * c_runtime_tests.rs runs these against the plain runtime and against the one
 * built with UndefinedBehaviorSanitizer, which stops the program at a signed
 * overflow. The expected values are computed in unsigned arithmetic, which
 * wraps by definition, and tagged as the runtime tags its results.
 */

#include "flux_test.h"

static int64_t wrap_add(int64_t a, int64_t b) { return (int64_t)((uint64_t)a + (uint64_t)b); }
static int64_t wrap_sub(int64_t a, int64_t b) { return (int64_t)((uint64_t)a - (uint64_t)b); }
static int64_t wrap_mul(int64_t a, int64_t b) { return (int64_t)((uint64_t)a * (uint64_t)b); }

/* 3037000500 squared leaves int64. */
#define BIG 3037000500LL

static const int64_t pairs[][2] = {
    {BIG, BIG},
    {-BIG, BIG},
    {FLUX_MAX_INLINE_INT, 3},
    {FLUX_MIN_INLINE_INT, -1},
    {FLUX_MAX_INLINE_INT, FLUX_MAX_INLINE_INT},
    {FLUX_MIN_INLINE_INT, FLUX_MIN_INLINE_INT},
    {7, -2},
};
#define PAIRS (sizeof(pairs) / sizeof(pairs[0]))

TEST(add_wraps) {
    for (size_t i = 0; i < PAIRS; i++) {
        int64_t a = pairs[i][0], b = pairs[i][1];
        ASSERT_EQ_WORD(flux_rt_add(flux_tag_int(a), flux_tag_int(b)), flux_tag_int(wrap_add(a, b)));
    }
}

TEST(sub_wraps) {
    for (size_t i = 0; i < PAIRS; i++) {
        int64_t a = pairs[i][0], b = pairs[i][1];
        ASSERT_EQ_WORD(flux_rt_sub(flux_tag_int(a), flux_tag_int(b)), flux_tag_int(wrap_sub(a, b)));
    }
}

TEST(mul_wraps) {
    for (size_t i = 0; i < PAIRS; i++) {
        int64_t a = pairs[i][0], b = pairs[i][1];
        ASSERT_EQ_WORD(flux_rt_mul(flux_tag_int(a), flux_tag_int(b)), flux_tag_int(wrap_mul(a, b)));
    }
}

TEST(neg_and_abs_wrap) {
    for (size_t i = 0; i < PAIRS; i++) {
        int64_t a = pairs[i][0];
        ASSERT_EQ_WORD(flux_rt_neg(flux_tag_int(a)), flux_tag_int(wrap_sub(0, a)));
        ASSERT_EQ_WORD(flux_abs(flux_tag_int(a)), flux_tag_int(a < 0 ? wrap_sub(0, a) : a));
    }
}

TEST(div_by_minus_one_negates) {
    for (size_t i = 0; i < PAIRS; i++) {
        int64_t a = pairs[i][0], b = pairs[i][1];
        int64_t want = b == -1 ? wrap_sub(0, a) : a / b;
        ASSERT_EQ_WORD(flux_rt_div(flux_tag_int(a), flux_tag_int(b)), flux_tag_int(want));
    }
}

TEST(mod_by_minus_one_is_zero) {
    for (size_t i = 0; i < PAIRS; i++) {
        int64_t a = pairs[i][0], b = pairs[i][1];
        int64_t want = b == -1 ? 0 : a % b;
        ASSERT_EQ_WORD(flux_rt_mod(flux_tag_int(a), flux_tag_int(b)), flux_tag_int(want));
    }
}

/* A BIGINT box can hold the operands a 63-bit Int can't (KI-039). Results
 * are still tagged inline until the flip, so they are compared that way. */
TEST(boxed_min_by_minus_one) {
    int64_t min = flux_make_int(INT64_MIN);
    int64_t minus_one = flux_tag_int(-1);
    ASSERT_EQ_WORD(flux_rt_div(min, minus_one), flux_tag_int(INT64_MIN));
    ASSERT_EQ_WORD(flux_rt_mod(min, minus_one), flux_tag_int(0));
    ASSERT_EQ_WORD(flux_rt_neg(min), flux_tag_int(INT64_MIN));
    ASSERT_EQ_WORD(flux_abs(min), flux_tag_int(INT64_MIN));
    flux_drop(min);
}

TEST(boxed_max_plus_one) {
    int64_t max = flux_make_int(INT64_MAX);
    ASSERT_EQ_WORD(flux_rt_add(max, flux_tag_int(1)), flux_tag_int(INT64_MIN));
    ASSERT_EQ_WORD(flux_rt_sub(flux_tag_int(-2), max), flux_tag_int(wrap_sub(-2, INT64_MAX)));
    ASSERT_EQ_WORD(flux_rt_mul(max, max), flux_tag_int(1));
    flux_drop(max);
}

FLUX_TESTS_BEGIN
    CASE(add_wraps)
    CASE(sub_wraps)
    CASE(mul_wraps)
    CASE(neg_and_abs_wrap)
    CASE(div_by_minus_one_negates)
    CASE(mod_by_minus_one_is_zero)
    CASE(boxed_min_by_minus_one)
    CASE(boxed_max_plus_one)
FLUX_TESTS_END
