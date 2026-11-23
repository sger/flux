/*
 * flux_test.h — a small unit-test framework for the C runtime.
 *
 * A test file defines its cases with TEST and lists them once:
 *
 *     #include "flux_test.h"
 *
 *     TEST(adds_two_ints) {
 *         ASSERT_EQ_I64(flux_untag_int(flux_rt_add(flux_tag_int(2),
 *                                                  flux_tag_int(3))), 5);
 *     }
 *
 *     FLUX_TESTS_BEGIN
 *         CASE(adds_two_ints)
 *     FLUX_TESTS_END
 *
 * tests/native_llvm/c_runtime_tests.rs builds the file against the runtime
 * as a native program is built, so the runtime's main() calls the flux_main
 * defined here. It runs one case per cargo test, named by FLUX_TEST_CASE.
 * Without it every case runs; FLUX_TEST_LIST=1 prints the names instead.
 *
 * A failed assertion prints where it is and what it saw, and the case goes
 * on, so one run reports every failure in it. Any failure exits with 1.
 */

#ifndef FLUX_TEST_H
#define FLUX_TEST_H

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int flux_test_failures = 0;

#define TEST(name) static void test_##name(void)

#define FLUX_TEST_FAIL(...)                                          \
    do {                                                             \
        fprintf(stderr, "%s:%d: ", __FILE__, __LINE__);              \
        fprintf(stderr, __VA_ARGS__);                                \
        fputc('\n', stderr);                                         \
        flux_test_failures++;                                        \
    } while (0)

#define ASSERT_TRUE(cond)                                            \
    do {                                                             \
        if (!(cond)) FLUX_TEST_FAIL("expected %s", #cond);           \
    } while (0)

/* Two C integers. */
#define ASSERT_EQ_I64(got, want)                                     \
    do {                                                             \
        int64_t got_ = (int64_t)(got);                               \
        int64_t want_ = (int64_t)(want);                             \
        if (got_ != want_)                                           \
            FLUX_TEST_FAIL("%s is %lld, want %lld", #got,            \
                           (long long)got_, (long long)want_);       \
    } while (0)

/* Two tagged words, shown in hex. */
#define ASSERT_EQ_WORD(got, want)                                    \
    do {                                                             \
        uint64_t got_ = (uint64_t)(got);                             \
        uint64_t want_ = (uint64_t)(want);                           \
        if (got_ != want_)                                           \
            FLUX_TEST_FAIL("%s is 0x%llx, want 0x%llx", #got,        \
                           (unsigned long long)got_,                 \
                           (unsigned long long)want_);               \
    } while (0)

/* A Flux String against C text. */
#define ASSERT_STR(value, text)                                      \
    do {                                                             \
        int64_t value_ = (value);                                    \
        const char *text_ = (text);                                  \
        uint32_t len_ = flux_string_len(value_);                     \
        if (len_ != (uint32_t)strlen(text_)                          \
            || memcmp(flux_string_data(value_), text_, len_) != 0)   \
            FLUX_TEST_FAIL("%s is \"%.*s\", want \"%s\"", #value,    \
                           (int)len_, flux_string_data(value_),      \
                           text_);                                   \
    } while (0)

typedef struct {
    const char *name;
    void (*run)(void);
} FluxTestCase;

/* Codegen defines the closure trampoline. No case calls a closure. */
int64_t flux_call_closure_c(int64_t closure, int64_t *args, int32_t nargs) {
    (void)closure;
    (void)args;
    (void)nargs;
    fprintf(stderr, "a C runtime test called a closure\n");
    exit(1);
}

static int64_t flux_test_main(const FluxTestCase *cases, size_t count) {
    const char *list = getenv("FLUX_TEST_LIST");
    if (list && *list) {
        for (size_t i = 0; i < count; i++) printf("%s\n", cases[i].name);
        return FLUX_NONE;
    }
    const char *only = getenv("FLUX_TEST_CASE");
    size_t ran = 0;
    for (size_t i = 0; i < count; i++) {
        if (only && *only && strcmp(only, cases[i].name) != 0) continue;
        int before = flux_test_failures;
        cases[i].run();
        if (flux_test_failures != before) fprintf(stderr, "FAIL %s\n", cases[i].name);
        ran++;
    }
    if (only && *only && ran == 0) {
        fprintf(stderr, "no test case named %s\n", only);
        exit(1);
    }
    if (flux_test_failures) exit(1);
    printf("%zu case(s) passed\n", ran);
    return FLUX_NONE;
}

#define FLUX_TESTS_BEGIN static const FluxTestCase flux_test_cases[] = {
#define CASE(name) {#name, test_##name},
#define FLUX_TESTS_END                                               \
    };                                                               \
    int64_t flux_main(void) {                                        \
        return flux_test_main(flux_test_cases,                       \
                              sizeof(flux_test_cases)                \
                                  / sizeof(flux_test_cases[0]));     \
    }

#endif /* FLUX_TEST_H */
