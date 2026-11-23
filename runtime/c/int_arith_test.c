/*
 * int_arith_test.c — the C runtime's Int arithmetic wraps, as on the VM.
 *
 * tests/native_llvm/c_runtime_tests.rs runs it against the plain runtime and
 * against the one built with UndefinedBehaviorSanitizer, which stops the
 * program at a signed overflow. The expected values are computed in
 * unsigned arithmetic, which wraps by definition.
 */

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>

static int failures = 0;

static void expect_int(int64_t got, int64_t want_raw, const char *what) {
    int64_t want = flux_tag_int(want_raw);
    if (got != want) {
        fprintf(stderr, "FAIL: %s: got %lld, want %lld\n", what,
                (long long)flux_untag_int(got), (long long)flux_untag_int(want));
        failures++;
    }
}

static int64_t wrap_add(int64_t a, int64_t b) { return (int64_t)((uint64_t)a + (uint64_t)b); }
static int64_t wrap_sub(int64_t a, int64_t b) { return (int64_t)((uint64_t)a - (uint64_t)b); }
static int64_t wrap_mul(int64_t a, int64_t b) { return (int64_t)((uint64_t)a * (uint64_t)b); }

int64_t flux_call_closure_c(int64_t closure, int64_t *args, int32_t nargs) {
    (void)closure;
    (void)args;
    (void)nargs;
    fprintf(stderr, "FAIL: int_arith_test called a closure\n");
    exit(1);
}

int64_t flux_main(void) {
    const int64_t max = FLUX_MAX_INLINE_INT;
    const int64_t min = FLUX_MIN_INLINE_INT;
    const int64_t big = 3037000500; /* big * big leaves int64 */
    const int64_t pairs[][2] = {
        {big, big}, {-big, big}, {max, 3}, {min, -1}, {max, max}, {min, min}, {7, -2},
    };
    for (size_t i = 0; i < sizeof(pairs) / sizeof(pairs[0]); i++) {
        int64_t a = pairs[i][0];
        int64_t b = pairs[i][1];
        int64_t ta = flux_tag_int(a);
        int64_t tb = flux_tag_int(b);
        expect_int(flux_rt_add(ta, tb), wrap_add(a, b), "flux_rt_add");
        expect_int(flux_rt_sub(ta, tb), wrap_sub(a, b), "flux_rt_sub");
        expect_int(flux_rt_mul(ta, tb), wrap_mul(a, b), "flux_rt_mul");
        expect_int(flux_rt_neg(ta), wrap_sub(0, a), "flux_rt_neg");
        expect_int(flux_abs(ta), a < 0 ? wrap_sub(0, a) : a, "flux_abs");
        expect_int(flux_rt_div(ta, tb), b == -1 ? wrap_sub(0, a) : a / b, "flux_rt_div");
        expect_int(flux_rt_mod(ta, tb), b == -1 ? 0 : a % b, "flux_rt_mod");
    }
    if (failures) exit(1);
    printf("int arithmetic wraps\n");
    return FLUX_NONE;
}
