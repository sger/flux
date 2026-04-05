/*
 * test_stackless_drop.c — Tests for stackless flux_drop.
 *
 * Verifies that flux_drop handles deep and wide structures without
 * stack overflow by using the stackless parent-chain traversal.
 *
 * Build:
 *   clang -std=c11 -O2 -DFLUX_RT_NO_MAIN -o test_stackless_drop \
 *     test_stackless_drop.c rc.c flux_rt.c string.c hamt.c effects.c array.c
 */

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

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

/* ── Helpers ───────────────────────────────────────────────────────── */

/*
 * Build a Cons cell: ADT { ctor_tag=4, field_count=2, fields=[head, tail] }
 * scan_fsize=2, obj_tag=FLUX_OBJ_ADT
 */
static int64_t make_cons(int64_t head, int64_t tail) {
    void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, 0xF2 /* FLUX_OBJ_ADT */);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = 4;  /* ctor_tag = Cons */
    hdr[1] = 2;  /* field_count */
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

/*
 * Build a binary tree of depth d.
 * Node: ADT { ctor_tag=5, field_count=2, fields=[left, right] }
 */
static int64_t make_tree(int depth) {
    if (depth <= 0) return FLUX_NONE;
    int64_t left = make_tree(depth - 1);
    int64_t right = make_tree(depth - 1);
    void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, 0xF2);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = 5;
    hdr[1] = 2;
    int64_t *fields = (int64_t *)((char *)mem + 8);
    fields[0] = left;
    fields[1] = right;
    return flux_tag_ptr(mem);
}

/*
 * Build a chain of Some(Some(Some(...Some(42)...))).
 * Some: ADT { ctor_tag=1, field_count=1, fields=[value] }
 * scan_fsize=1
 */
static int64_t make_nested_some(int depth) {
    int64_t val = flux_tag_int(42);
    for (int i = 0; i < depth; i++) {
        void *mem = flux_gc_alloc_header(8 + 1 * 8, 1, 0xF2);
        int32_t *hdr = (int32_t *)mem;
        hdr[0] = 1;  /* Some */
        hdr[1] = 1;
        int64_t *fields = (int64_t *)((char *)mem + 8);
        fields[0] = val;
        val = flux_tag_ptr(mem);
    }
    return val;
}

/*
 * Build a wide tuple: (v0, v1, ..., vN-1)
 * Tuple: { obj_tag=0xF3, pad[3], arity:i32, fields:i64[] }
 * scan_fsize = arity
 */
static int64_t make_wide_tuple(int arity) {
    uint32_t payload = 8 + (uint32_t)arity * 8;
    uint8_t scan = (arity <= 255) ? (uint8_t)arity : 255;
    void *mem = flux_gc_alloc_header(payload, scan, 0xF3 /* FLUX_OBJ_TUPLE */);
    uint8_t *p = (uint8_t *)mem;
    p[0] = 0xF3;  /* obj_tag */
    int32_t *arity_ptr = (int32_t *)((char *)mem + 4);
    *arity_ptr = arity;
    int64_t *fields = (int64_t *)((char *)mem + 8);
    for (int i = 0; i < arity; i++) {
        fields[i] = flux_tag_int(i);
    }
    return flux_tag_ptr(mem);
}

/* ── Tests ─────────────────────────────────────────────────────────── */

static void test_deep_list(void) {
    /* 200K Cons cells — would overflow the stack with recursive drop. */
    int64_t list = make_list(200000);
    ASSERT(flux_is_ptr(list), "deep list is ptr");
    flux_drop(list);
    ASSERT(1, "stackless drop of 200K-element list");
}

static void test_shared_list(void) {
    /* Build a list, dup it, drop once — should not free. */
    int64_t list = make_list(100);
    flux_dup(list);
    flux_drop(list);
    /* List should still be alive (rc=1). */
    void *ptr = flux_untag_ptr(list);
    int64_t *fields = (int64_t *)((char *)ptr + 8);
    ASSERT(flux_untag_int(fields[0]) == 0, "shared list head intact after drop");
    flux_drop(list);
    ASSERT(1, "shared list final drop OK");
}

static void test_tree(void) {
    /* Depth 18 = 262143 nodes. */
    int64_t tree = make_tree(18);
    ASSERT(flux_is_ptr(tree), "tree is ptr");
    flux_drop(tree);
    ASSERT(1, "stackless drop of depth-18 tree (262K nodes)");
}

static void test_nested_some(void) {
    /* 200K nested Some — single-field chain, tests tail-call optimization. */
    int64_t val = make_nested_some(200000);
    flux_drop(val);
    ASSERT(1, "stackless drop of 200K nested Some");
}

static void test_wide_tuple(void) {
    /* Tuple with 100 fields — tests multi-field scanning. */
    int64_t t = make_wide_tuple(100);
    ASSERT(flux_is_ptr(t), "wide tuple is ptr");
    flux_drop(t);
    ASSERT(1, "stackless drop of 100-field tuple");
}

static void test_list_of_lists(void) {
    /* List of 1000 lists, each 1000 elements — tests mixed deep+wide. */
    int64_t outer = FLUX_EMPTY_LIST;
    for (int i = 0; i < 1000; i++) {
        int64_t inner = make_list(1000);
        outer = make_cons(inner, outer);
    }
    flux_drop(outer);
    ASSERT(1, "stackless drop of 1000 lists x 1000 elements");
}

static void test_leaf_adt(void) {
    /* ADT with scan_fsize=0 (no children). */
    void *mem = flux_gc_alloc_header(8, 0, 0xF2);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = 0;  /* None-like */
    hdr[1] = 0;
    int64_t val = flux_tag_ptr(mem);
    flux_drop(val);
    ASSERT(1, "drop leaf ADT (scan_fsize=0)");
}

/* ── Main ──────────────────────────────────────────────────────────── */

int main(void) {
    flux_rt_init();

    test_deep_list();
    test_shared_list();
    test_tree();
    test_nested_some();
    test_wide_tuple();
    test_list_of_lists();
    test_leaf_adt();

    flux_rt_shutdown();

    printf("%d/%d stackless drop tests passed\n", tests_passed, tests_run);
    return (tests_passed == tests_run) ? 0 : 1;
}
