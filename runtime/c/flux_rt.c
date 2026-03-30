/*
 * flux_rt.c — Flux runtime core: init, shutdown, print, I/O.
 *
 * All values are NaN-boxed i64.  See flux_rt.h for the encoding.
 */

// Expose POSIX APIs (clock_gettime, etc.) on Linux/glibc.
#if !defined(_POSIX_C_SOURCE) && !defined(__APPLE__)
#define _POSIX_C_SOURCE 199309L
#endif

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <time.h>
#if defined(_MSC_VER) || defined(_WIN32)
#include <windows.h>
#endif

/* ── Forward declarations for string helpers (string.c) ─────────────── */

extern const char *flux_string_data(int64_t s);
extern uint32_t    flux_string_len(int64_t s);
extern int64_t     flux_string_new(const char *data, uint32_t len);

/* ── Runtime lifecycle ──────────────────────────────────────────────── */

void flux_rt_init(void) {
    flux_gc_init(0);
}

void flux_rt_shutdown(void) {
    /* Skip GC shutdown — the OS reclaims all memory at process exit.
     * Walking millions of malloc'd objects is slower than just exiting. */
    fflush(stdout);
}

/* ── Printing ───────────────────────────────────────────────────────── */

/*
 * Print a NaN-boxed value to stdout (no trailing newline).
 * Dispatches on the NaN-box tag to determine the type.
 */
/* Internal: print a value without trailing newline. */
static void flux_print_value(int64_t val) {
    uint64_t bits = (uint64_t)val;

    /* Float: top 14 bits are NOT the sentinel → raw IEEE double. */
    if ((bits & FLUX_SENTINEL_MASK) != FLUX_NANBOX_SENTINEL) {
        double d;
        memcpy(&d, &bits, sizeof(d));
        printf("%.15g", d);
        return;
    }

    int tag = flux_nanbox_tag(val);
    switch (tag) {
    case FLUX_TAG_INTEGER:
        printf("%lld", (long long)flux_untag_int(val));
        break;

    case FLUX_TAG_BOOLEAN:
        printf("%s", ((uint64_t)val & FLUX_PAYLOAD_MASK) ? "true" : "false");
        break;

    case FLUX_TAG_NONE:
        printf("None");
        break;

    case FLUX_TAG_EMPTY_LIST:
        printf("[]");
        break;

    case FLUX_TAG_THUNK:
        /* Thunks should never escape to user code; print for debugging. */
        printf("<thunk>");
        break;

    case FLUX_TAG_BOXED_VALUE: {
        void *ptr = flux_untag_ptr(val);
        if (!ptr) {
            printf("<null>");
            break;
        }

        uint8_t obj = flux_obj_tag(ptr);
        if (obj == FLUX_OBJ_BIGINT) {
            /* BigInt: { obj_tag, pad[7], int64_t value } */
            int64_t bigval = *(int64_t *)((char *)ptr + 8);
            printf("%lld", (long long)bigval);
        } else if (obj == FLUX_OBJ_STRING) {
            /* String: { obj_tag, _pad[3], len, data[] } */
            uint32_t len = *(uint32_t *)((char *)ptr + 4);
            const char *data = (const char *)ptr + 8;
            putchar('"');
            fwrite(data, 1, len, stdout);
            putchar('"');
        } else if (obj == FLUX_OBJ_ARRAY) {
            /* Array: { obj_tag, _pad[3], len, capacity, _pad2, elements[] } */
            uint32_t len = *(uint32_t *)((char *)ptr + 4);
            int64_t *elems = (int64_t *)((char *)ptr + 16);
            printf("[|");
            for (uint32_t i = 0; i < len; i++) {
                if (i > 0) printf(", ");
                flux_print_value(elems[i]);
            }
            printf("|]");
        } else if (obj == FLUX_OBJ_TUPLE) {
            /* Tuple: { obj_tag, _pad[3], i32 arity, i64 elements[] } */
            uint32_t arity = *(uint32_t *)((char *)ptr + 4);
            int64_t *elems = (int64_t *)((char *)ptr + 8);
            printf("(");
            for (uint32_t i = 0; i < arity; i++) {
                if (i > 0) printf(", ");
                flux_print_value(elems[i]);
            }
            if (arity == 1) printf(",");
            printf(")");
        } else if (obj == FLUX_OBJ_ADT) {
            /* ADT: { i32 tag, i32 field_count, i64 fields[] } */
            int32_t ctor_tag = *(int32_t *)ptr;
            int32_t field_count = *((int32_t *)ptr + 1);
            int64_t *fields = (int64_t *)((char *)ptr + 8);

            switch (ctor_tag) {
            case 0: /* None or Nil */
                if (field_count == 0) {
                    printf("None");
                } else {
                    printf("(");
                    for (int32_t i = 0; i < field_count; i++) {
                        if (i > 0) printf(", ");
                        flux_print_value(fields[i]);
                    }
                    printf(")");
                }
                break;
            case 1: /* Some */
                printf("Some(");
                if (field_count > 0) flux_print_value(fields[0]);
                printf(")");
                break;
            case 2: /* Left */
                printf("Left(");
                if (field_count > 0) flux_print_value(fields[0]);
                printf(")");
                break;
            case 3: /* Right */
                printf("Right(");
                if (field_count > 0) flux_print_value(fields[0]);
                printf(")");
                break;
            case 4: /* Cons */
                printf("[");
                flux_print_value(fields[0]);
                {
                    int64_t tail = fields[1];
                    while (flux_is_ptr(tail)) {
                        void *tp = flux_untag_ptr(tail);
                        int32_t tt = *(int32_t *)tp;
                        if (tt != 4) break;
                        int64_t *tf = (int64_t *)((char *)tp + 8);
                        printf(", ");
                        flux_print_value(tf[0]);
                        tail = tf[1];
                    }
                }
                printf("]");
                break;
            default:
                if (field_count == 0) {
                    printf("<adt tag=%d>", ctor_tag);
                } else {
                    printf("<adt tag=%d>(", ctor_tag);
                    for (int32_t i = 0; i < field_count; i++) {
                        if (i > 0) printf(", ");
                        flux_print_value(fields[i]);
                    }
                    printf(")");
                }
                break;
            }
        } else if (flux_is_hamt(ptr)) {
            /* HAMT (hash map) */
            int64_t s = flux_hamt_format(val);
            fwrite(flux_string_data(s), 1, flux_string_len(s), stdout);
        } else {
            printf("<unknown obj=0x%02x>", obj);
        }
        break;
    }

    default:
        printf("<unknown tag=%d>", tag);
        break;
    }
}

void flux_print(int64_t val) {
    flux_print_value(val);
    putchar('\n');
    fflush(stdout);
}

void flux_println(int64_t val) {
    flux_print(val);
}
/* Print value without newline, followed by a space — used for multi-arg print. */
void flux_print_space(int64_t val) {
    flux_print_value(val);
    putchar(' ');
}

/* ── I/O ────────────────────────────────────────────────────────────── */

int64_t flux_read_line(void) {
    char buf[4096];
    if (!fgets(buf, sizeof(buf), stdin)) {
        return flux_string_new("", 0);
    }
    /* Strip trailing newline. */
    size_t len = strlen(buf);
    if (len > 0 && buf[len - 1] == '\n') {
        buf[--len] = '\0';
    }
    return flux_string_new(buf, (uint32_t)len);
}

int64_t flux_read_file(int64_t path) {
    const char *path_str = flux_string_data(path);
    uint32_t    path_len = flux_string_len(path);

    /* Null-terminate the path (it may not be). */
    char *cpath = (char *)malloc(path_len + 1);
    if (!cpath) return flux_make_none();
    memcpy(cpath, path_str, path_len);
    cpath[path_len] = '\0';

    FILE *f = fopen(cpath, "rb");
    free(cpath);
    if (!f) return flux_make_none();

    fseek(f, 0, SEEK_END);
    long fsize = ftell(f);
    fseek(f, 0, SEEK_SET);

    if (fsize < 0) { fclose(f); return flux_make_none(); }

    char *contents = (char *)malloc((size_t)fsize);
    if (!contents) { fclose(f); return flux_make_none(); }

    size_t nread = fread(contents, 1, (size_t)fsize, f);
    fclose(f);

    int64_t result = flux_string_new(contents, (uint32_t)nread);
    free(contents);
    return result;
}

int64_t flux_write_file(int64_t path, int64_t content) {
    const char *path_str    = flux_string_data(path);
    uint32_t    path_len    = flux_string_len(path);
    const char *content_str = flux_string_data(content);
    uint32_t    content_len = flux_string_len(content);

    char *cpath = (char *)malloc(path_len + 1);
    if (!cpath) return flux_make_bool(0);
    memcpy(cpath, path_str, path_len);
    cpath[path_len] = '\0';

    FILE *f = fopen(cpath, "wb");
    free(cpath);
    if (!f) return flux_make_bool(0);

    size_t written = fwrite(content_str, 1, content_len, f);
    fclose(f);

    return flux_make_bool(written == content_len);
}

/* ── Numeric helpers ────────────────────────────────────────────────── */

static inline int flux_val_is_float(int64_t val) {
    return !flux_is_nanbox(val);
}

static inline double flux_as_double(int64_t val) {
    double d;
    memcpy(&d, &val, sizeof(d));
    return d;
}

static inline int64_t flux_from_double(double d) {
    int64_t v;
    memcpy(&v, &d, sizeof(v));
    return v;
}

int64_t flux_abs(int64_t n) {
    if (flux_val_is_float(n)) {
        double d = flux_as_double(n);
        return flux_from_double(d < 0 ? -d : d);
    }
    int64_t raw = flux_untag_int(n);
    return flux_tag_int(raw < 0 ? -raw : raw);
}

int64_t flux_min(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        double da = flux_as_double(a);
        double db = flux_as_double(b);
        return flux_from_double(da < db ? da : db);
    }
    int64_t ra = flux_untag_int(a);
    int64_t rb = flux_untag_int(b);
    return flux_tag_int(ra < rb ? ra : rb);
}

int64_t flux_max(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        double da = flux_as_double(a);
        double db = flux_as_double(b);
        return flux_from_double(da > db ? da : db);
    }
    int64_t ra = flux_untag_int(a);
    int64_t rb = flux_untag_int(b);
    return flux_tag_int(ra > rb ? ra : rb);
}

/* ── Runtime-dispatching arithmetic ─────────────────────────────────── */
/* These check the value type at runtime and dispatch to the correct op. */

static inline int flux_val_is_string(int64_t val) {
    if (!flux_is_ptr(val)) return 0;
    void *ptr = flux_untag_ptr(val);
    return flux_obj_tag(ptr) == FLUX_OBJ_STRING;
}

int64_t flux_rt_add(int64_t a, int64_t b) {
    /* Check string first (boxed ptr with string tag), then float (!nanbox). */
    if (flux_val_is_string(a)) {
        return flux_string_concat(a, b);
    }
    if (flux_val_is_float(a)) {
        return flux_from_double(flux_as_double(a) + flux_as_double(b));
    }
    return flux_tag_int(flux_untag_int(a) + flux_untag_int(b));
}

int64_t flux_rt_sub(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        return flux_from_double(flux_as_double(a) - flux_as_double(b));
    }
    return flux_tag_int(flux_untag_int(a) - flux_untag_int(b));
}

int64_t flux_rt_mul(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        return flux_from_double(flux_as_double(a) * flux_as_double(b));
    }
    return flux_tag_int(flux_untag_int(a) * flux_untag_int(b));
}

int64_t flux_rt_div(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        return flux_from_double(flux_as_double(a) / flux_as_double(b));
    }
    int64_t rb = flux_untag_int(b);
    if (rb == 0) {
        flux_panic(flux_string_new("Division by zero", 16));
        return flux_tag_int(0); /* unreachable */
    }
    return flux_tag_int(flux_untag_int(a) / rb);
}

int64_t flux_rt_mod(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        return flux_from_double(fmod(flux_as_double(a), flux_as_double(b)));
    }
    int64_t rb = flux_untag_int(b);
    if (rb == 0) {
        flux_panic(flux_string_new("Division by zero", 16));
        return flux_tag_int(0); /* unreachable */
    }
    return flux_tag_int(flux_untag_int(a) % rb);
}

int64_t flux_rt_neg(int64_t a) {
    if (flux_val_is_float(a)) {
        return flux_from_double(-flux_as_double(a));
    }
    return flux_tag_int(-flux_untag_int(a));
}

/* ── Some-wrapping helper ───────────────────────────────────────────── */

int64_t flux_wrap_some(int64_t val) {
    void *mem = flux_gc_alloc_header(8 + 8, 1, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = 1; /* ctor_tag = Some */
    hdr[1] = 1; /* field_count = 1 */
    int64_t *fields = (int64_t *)((char *)mem + 8);
    fields[0] = val;
    return flux_tag_ptr(mem);
}

int64_t flux_make_left(int64_t val) {
    void *mem = flux_gc_alloc_header(8 + 8, 1, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = 2; /* ctor_tag = Left */
    hdr[1] = 1; /* field_count = 1 */
    int64_t *fields = (int64_t *)((char *)mem + 8);
    fields[0] = val;
    return flux_tag_ptr(mem);
}

int64_t flux_make_right(int64_t val) {
    void *mem = flux_gc_alloc_header(8 + 8, 1, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = 3; /* ctor_tag = Right */
    hdr[1] = 1; /* field_count = 1 */
    int64_t *fields = (int64_t *)((char *)mem + 8);
    fields[0] = val;
    return flux_tag_ptr(mem);
}

/* HAMT get returning Some(value) or None — matches VM semantics. */
int64_t flux_hamt_get_option(int64_t map, int64_t key) {
    int64_t result = flux_hamt_get(map, key);
    if (flux_is_nanbox(result) && flux_nanbox_tag(result) == FLUX_TAG_NONE) {
        return flux_make_none();
    }
    return flux_wrap_some(result);
}

/* ── Runtime-dispatching comparisons ────────────────────────────────── */

int64_t flux_rt_eq(int64_t a, int64_t b) {
    /* Fast path: bitwise equal (same int, bool, None, or same pointer). */
    if (a == b) return flux_make_bool(1);
    /* String structural equality. */
    if (flux_is_ptr(a) && flux_is_ptr(b)) {
        void *pa = flux_untag_ptr(a);
        void *pb = flux_untag_ptr(b);
        if (pa && pb) {
            uint8_t tag_a = flux_obj_tag(pa);
            uint8_t tag_b = flux_obj_tag(pb);
            if (tag_a != tag_b) return flux_make_bool(0);
            /* BigInt structural equality. */
            if (tag_a == FLUX_OBJ_BIGINT) {
                int64_t va = *(int64_t *)((char *)pa + 8);
                int64_t vb = *(int64_t *)((char *)pb + 8);
                return flux_make_bool(va == vb);
            }
            /* String structural equality. */
            if (tag_a == FLUX_OBJ_STRING) {
                return flux_make_bool(flux_string_eq(a, b));
            }
            /* Tuple structural equality. */
            if (tag_a == FLUX_OBJ_TUPLE) {
                uint32_t arity_a = *(uint32_t *)((char *)pa + 4);
                uint32_t arity_b = *(uint32_t *)((char *)pb + 4);
                if (arity_a != arity_b) return flux_make_bool(0);
                int64_t *fa = (int64_t *)((char *)pa + 8);
                int64_t *fb = (int64_t *)((char *)pb + 8);
                for (uint32_t i = 0; i < arity_a; i++) {
                    int64_t eq = flux_rt_eq(fa[i], fb[i]);
                    if (eq == flux_make_bool(0)) return flux_make_bool(0);
                }
                return flux_make_bool(1);
            }
            /* ADT structural equality (Option/Either/List/user ctors). */
            if (tag_a == FLUX_OBJ_ADT) {
                int32_t ctor_a = *(int32_t *)pa;
                int32_t ctor_b = *(int32_t *)pb;
                int32_t field_count_a = *((int32_t *)pa + 1);
                int32_t field_count_b = *((int32_t *)pb + 1);
                if (ctor_a != ctor_b || field_count_a != field_count_b) {
                    return flux_make_bool(0);
                }
                int64_t *fa = (int64_t *)((char *)pa + 8);
                int64_t *fb = (int64_t *)((char *)pb + 8);
                for (int32_t i = 0; i < field_count_a; i++) {
                    int64_t eq = flux_rt_eq(fa[i], fb[i]);
                    if (eq == flux_make_bool(0)) return flux_make_bool(0);
                }
                return flux_make_bool(1);
            }
        }
    }
    /* Float equality. */
    if (flux_val_is_float(a) && flux_val_is_float(b)) {
        return flux_make_bool(flux_as_double(a) == flux_as_double(b));
    }
    return flux_make_bool(0);
}

int64_t flux_rt_neq(int64_t a, int64_t b) {
    int64_t eq = flux_rt_eq(a, b);
    return (eq == flux_make_bool(1)) ? flux_make_bool(0) : flux_make_bool(1);
}

int64_t flux_rt_lt(int64_t a, int64_t b) {
    if (flux_val_is_float(a))
        return flux_make_bool(flux_as_double(a) < flux_as_double(b));
    return flux_make_bool(flux_untag_int(a) < flux_untag_int(b));
}

int64_t flux_rt_le(int64_t a, int64_t b) {
    if (flux_val_is_float(a))
        return flux_make_bool(flux_as_double(a) <= flux_as_double(b));
    return flux_make_bool(flux_untag_int(a) <= flux_untag_int(b));
}

int64_t flux_rt_gt(int64_t a, int64_t b) {
    if (flux_val_is_float(a))
        return flux_make_bool(flux_as_double(a) > flux_as_double(b));
    return flux_make_bool(flux_untag_int(a) > flux_untag_int(b));
}

int64_t flux_rt_ge(int64_t a, int64_t b) {
    if (flux_val_is_float(a))
        return flux_make_bool(flux_as_double(a) >= flux_as_double(b));
    return flux_make_bool(flux_untag_int(a) >= flux_untag_int(b));
}

/* ── Runtime-dispatching index ──────────────────────────────────────── */

int64_t flux_rt_index(int64_t collection, int64_t key) {
    if (!flux_is_ptr(collection)) {
        return flux_make_none();
    }
    void *ptr = flux_untag_ptr(collection);
    uint8_t tag = flux_obj_tag(ptr);
    switch (tag) {
    case FLUX_OBJ_ARRAY: {
        int64_t result = flux_array_get(collection, key);
        if (flux_is_nanbox(result) && flux_nanbox_tag(result) == FLUX_TAG_NONE) {
            return flux_make_none();
        }
        return flux_wrap_some(result);
    }
    case FLUX_OBJ_TUPLE: {
        uint32_t arity = *(uint32_t *)((char *)ptr + 4);
        int64_t idx = flux_untag_int(key);
        if (idx < 0 || (uint32_t)idx >= arity) return flux_make_none();
        int64_t *elems = (int64_t *)((char *)ptr + 8);
        return flux_wrap_some(elems[idx]);
    }
    default: {
        /* Assume HAMT for any other boxed value. */
        return flux_hamt_get_option(collection, key);
    }
    }
}

/* ── Type inspection ────────────────────────────────────────────────── */

int64_t flux_type_of(int64_t val) {
    uint64_t bits = (uint64_t)val;
    if ((bits & FLUX_SENTINEL_MASK) != FLUX_NANBOX_SENTINEL) {
        return flux_string_new("Float", 5);
    }
    int tag = flux_nanbox_tag(val);
    switch (tag) {
    case FLUX_TAG_INTEGER:       return flux_string_new("Int", 3);
    case FLUX_TAG_BOOLEAN:       return flux_string_new("Bool", 4);
    case FLUX_TAG_NONE:          return flux_string_new("None", 4);
    case FLUX_TAG_EMPTY_LIST:    return flux_string_new("List", 4);
    case FLUX_TAG_BASE_FUNCTION: return flux_string_new("Function", 8);
    case FLUX_TAG_BOXED_VALUE: {
        void *ptr = flux_untag_ptr(val);
        if (ptr) {
            uint8_t obj = flux_obj_tag(ptr);
            switch (obj) {
            case FLUX_OBJ_BIGINT:  return flux_string_new("Int", 3);
            case FLUX_OBJ_STRING:  return flux_string_new("String", 6);
            case FLUX_OBJ_ARRAY:   return flux_string_new("Array", 5);
            case FLUX_OBJ_TUPLE:   return flux_string_new("Tuple", 5);
            case FLUX_OBJ_CLOSURE: return flux_string_new("Function", 8);
            default: {
                /* Non-FLUX_OBJ_* tag: either ADT or HAMT.
                 * Check HAMT first — HAMT kind values (0-3) collide with
                 * ADT ctor_tags (0=None, 1=Some, etc.). */
                if (flux_is_hamt(ptr)) {
                    return flux_string_new("Map", 3);
                }
                int32_t first_i32 = *(int32_t *)ptr;
                int32_t second_i32 = *((int32_t *)ptr + 1);
                /* ADT: ctor_tag 0-255, field_count 0-100 (reasonable) */
                if (first_i32 >= 0 && first_i32 <= 255 && second_i32 >= 0 && second_i32 <= 100) {
                    switch (first_i32) {
                    case 0: return (second_i32 == 0) ? flux_string_new("None", 4) : flux_string_new("Adt", 3);
                    case 1: return flux_string_new("Some", 4);
                    case 2: return flux_string_new("Left", 4);
                    case 3: return flux_string_new("Right", 5);
                    case 4: return flux_string_new("List", 4);
                    default: return flux_string_new("Adt", 3);
                    }
                }
                /* Fall through to Map for HAMT-like structures. */
                return flux_string_new("Map", 3);
            }
            }
        }
        return flux_string_new("Object", 6);
    }
    default:                     return flux_string_new("Unknown", 7);
    }
}

int64_t flux_is_int(int64_t val) {
    if (!flux_is_nanbox(val)) return flux_make_bool(0);
    if (flux_nanbox_tag(val) == FLUX_TAG_INTEGER) return flux_make_bool(1);
    /* Heap-boxed BigInt is also an integer. */
    if (flux_nanbox_tag(val) == FLUX_TAG_BOXED_VALUE) {
        void *ptr = flux_untag_ptr(val);
        if (ptr && flux_obj_tag(ptr) == FLUX_OBJ_BIGINT) return flux_make_bool(1);
    }
    return flux_make_bool(0);
}

int64_t flux_is_float(int64_t val) {
    return flux_make_bool(!flux_is_nanbox(val));
}

int64_t flux_is_string(int64_t val) {
    /* Strings are boxed values; we can't distinguish from other boxed without a type tag. */
    return flux_make_bool(flux_is_ptr(val));
}

int64_t flux_is_bool(int64_t val) {
    if (!flux_is_nanbox(val)) return flux_make_bool(0);
    return flux_make_bool(flux_nanbox_tag(val) == FLUX_TAG_BOOLEAN);
}

int64_t flux_is_none(int64_t val) {
    if (!flux_is_nanbox(val)) return flux_make_bool(0);
    return flux_make_bool(flux_nanbox_tag(val) == FLUX_TAG_NONE);
}

/* ── Shadow stack for native stack traces ───────────────────────────── */

#define FLUX_TRACE_MAX 256

typedef struct {
    const char *name;
    const char *file;
    int32_t     line;
} FluxTraceFrame;

static FluxTraceFrame flux_trace_stack[FLUX_TRACE_MAX];
static int32_t        flux_trace_depth = 0;

void flux_trace_push(const char *name, const char *file, int32_t line) {
    if (flux_trace_depth < FLUX_TRACE_MAX) {
        flux_trace_stack[flux_trace_depth].name = name;
        flux_trace_stack[flux_trace_depth].file = file;
        flux_trace_stack[flux_trace_depth].line = line;
    }
    flux_trace_depth++;
}

void flux_trace_pop(void) {
    if (flux_trace_depth > 0) flux_trace_depth--;
}

static void flux_trace_print(void) {
    int32_t depth = flux_trace_depth < FLUX_TRACE_MAX
                  ? flux_trace_depth : FLUX_TRACE_MAX;
    if (depth == 0) return;
    fprintf(stderr, "\nStack trace:\n");
    for (int32_t i = depth - 1; i >= 0; i--) {
        FluxTraceFrame *f = &flux_trace_stack[i];
        if (f->file && f->file[0] != '\0') {
            fprintf(stderr, "  at %s (%s:%d)\n", f->name, f->file, f->line);
        } else {
            fprintf(stderr, "  at %s\n", f->name);
        }
    }
}

/* ── Control ────────────────────────────────────────────────────────── */

void flux_panic(int64_t msg) {
    if (flux_is_ptr(msg)) {
        uint32_t len = flux_string_len(msg);
        const char *data = flux_string_data(msg);
        fprintf(stderr, "panic: %.*s\n", (int)len, data);
    } else {
        fprintf(stderr, "panic: ");
        flux_print(msg);
        fprintf(stderr, "\n");
    }
    flux_trace_print();
    abort();
}

int64_t flux_clock_now(void) {
#if defined(_MSC_VER) || defined(_WIN32)
    /* Windows: use QueryPerformanceCounter for monotonic time. */
    LARGE_INTEGER freq, counter;
    QueryPerformanceFrequency(&freq);
    QueryPerformanceCounter(&counter);
    int64_t ms = (int64_t)(counter.QuadPart * 1000 / freq.QuadPart);
#else
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    int64_t ms = (int64_t)ts.tv_sec * 1000 + (int64_t)ts.tv_nsec / 1000000;
#endif
    return flux_tag_int(ms);
}

/* ── Extended string/I/O helpers ────────────────────────────────────── */

int64_t flux_trim(int64_t s) {
    const char *data = flux_string_data(s);
    uint32_t len = flux_string_len(s);
    uint32_t start = 0, end = len;
    while (start < end && (data[start] == ' ' || data[start] == '\t' ||
                            data[start] == '\n' || data[start] == '\r'))
        start++;
    while (end > start && (data[end - 1] == ' ' || data[end - 1] == '\t' ||
                            data[end - 1] == '\n' || data[end - 1] == '\r'))
        end--;
    return flux_string_new(data + start, end - start);
}

int64_t flux_upper(int64_t s) {
    const char *data = flux_string_data(s);
    uint32_t len = flux_string_len(s);
    char *buf = (char *)malloc(len + 1);
    for (uint32_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)data[i];
        buf[i] = (c >= 'a' && c <= 'z') ? (char)(c - 32) : (char)c;
    }
    buf[len] = '\0';
    int64_t result = flux_string_new(buf, len);
    free(buf);
    return result;
}

int64_t flux_lower(int64_t s) {
    const char *data = flux_string_data(s);
    uint32_t len = flux_string_len(s);
    char *buf = (char *)malloc(len + 1);
    for (uint32_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)data[i];
        buf[i] = (c >= 'A' && c <= 'Z') ? (char)(c + 32) : (char)c;
    }
    buf[len] = '\0';
    int64_t result = flux_string_new(buf, len);
    free(buf);
    return result;
}

int64_t flux_replace(int64_t s, int64_t from, int64_t to) {
    const char *src = flux_string_data(s);
    uint32_t src_len = flux_string_len(s);
    const char *from_str = flux_string_data(from);
    uint32_t from_len = flux_string_len(from);
    const char *to_str = flux_string_data(to);
    uint32_t to_len = flux_string_len(to);

    if (from_len == 0) return s;

    /* Count occurrences to pre-allocate. */
    uint32_t count = 0;
    const char *p = src;
    const char *end = src + src_len;
    while (p <= end - from_len) {
        if (memcmp(p, from_str, from_len) == 0) { count++; p += from_len; }
        else { p++; }
    }
    if (count == 0) return s;

    uint32_t new_len = src_len + count * (to_len - from_len);
    char *buf = (char *)malloc(new_len + 1);
    char *dst = buf;
    p = src;
    while (p <= end - from_len) {
        if (memcmp(p, from_str, from_len) == 0) {
            memcpy(dst, to_str, to_len); dst += to_len; p += from_len;
        } else { *dst++ = *p++; }
    }
    while (p < end) *dst++ = *p++;
    *dst = '\0';
    int64_t result = flux_string_new(buf, new_len);
    free(buf);
    return result;
}

int64_t flux_chars(int64_t s) {
    const char *data = flux_string_data(s);
    uint32_t len = flux_string_len(s);
    int64_t *elems = (int64_t *)malloc(len * sizeof(int64_t));
    for (uint32_t i = 0; i < len; i++) {
        elems[i] = flux_string_new(data + i, 1);
    }
    int64_t result = flux_array_new(elems, (int32_t)len);
    free(elems);
    return result;
}

int64_t flux_str_contains(int64_t haystack, int64_t needle) {
    const char *h = flux_string_data(haystack);
    uint32_t h_len = flux_string_len(haystack);
    const char *n = flux_string_data(needle);
    uint32_t n_len = flux_string_len(needle);
    if (n_len > h_len) return flux_make_bool(0);
    if (n_len == 0) return flux_make_bool(1);
    for (uint32_t i = 0; i <= h_len - n_len; i++) {
        if (memcmp(h + i, n, n_len) == 0) return flux_make_bool(1);
    }
    return flux_make_bool(0);
}

int64_t flux_substring(int64_t s, int64_t start_val, int64_t end_val) {
    return flux_string_slice(s, start_val, end_val);
}

int64_t flux_parse_int(int64_t s) {
    return flux_string_to_int(s);
}

/* parse_ints(arr) → parse each string element of arr as an int. */
int64_t flux_parse_ints(int64_t arr) {
    if (!flux_is_ptr(arr)) return flux_array_new(NULL, 0);
    void *ptr = flux_untag_ptr(arr);
    if (!ptr || flux_obj_tag(ptr) != FLUX_OBJ_ARRAY) return flux_array_new(NULL, 0);
    uint32_t len = *(uint32_t *)((char *)ptr + 4);
    int64_t *elems = (int64_t *)((char *)ptr + 16);
    int64_t *ints = (int64_t *)malloc(len * sizeof(int64_t));
    for (uint32_t i = 0; i < len; i++) {
        ints[i] = flux_parse_int(elems[i]);
    }
    int64_t result = flux_array_new(ints, (int32_t)len);
    free(ints);
    return result;
}

int64_t flux_to_string(int64_t val) {
    uint64_t bits = (uint64_t)val;
    /* Float. */
    if ((bits & FLUX_SENTINEL_MASK) != FLUX_NANBOX_SENTINEL) {
        return flux_float_to_string(val);
    }
    int tag = flux_nanbox_tag(val);
    switch (tag) {
    case FLUX_TAG_INTEGER:    return flux_int_to_string(val);
    case FLUX_TAG_BOOLEAN:
        return ((uint64_t)val & FLUX_PAYLOAD_MASK)
            ? flux_string_new("true", 4)
            : flux_string_new("false", 5);
    case FLUX_TAG_NONE:       return flux_string_new("None", 4);
    case FLUX_TAG_EMPTY_LIST: return flux_string_new("[]", 2);
    case FLUX_TAG_BOXED_VALUE: {
        void *ptr = flux_untag_ptr(val);
        if (ptr) {
            uint8_t obj = flux_obj_tag(ptr);
            if (obj == FLUX_OBJ_BIGINT) {
                int64_t bigval = *(int64_t *)((char *)ptr + 8);
                char buf[32];
                int len = snprintf(buf, sizeof(buf), "%lld", (long long)bigval);
                return flux_string_new(buf, (uint32_t)len);
            }
            if (obj == FLUX_OBJ_STRING) return val;
            if (obj == FLUX_OBJ_ARRAY) {
                /* Render array as "[|elem1, elem2, ...|]" */
                uint32_t len = *(uint32_t *)((char *)ptr + 4);
                int64_t *elems = (int64_t *)((char *)ptr + 16);
                /* Build string in buffer. */
                char buf[4096];
                int pos = 0;
                pos += snprintf(buf + pos, sizeof(buf) - pos, "[|");
                for (uint32_t i = 0; i < len && pos < (int)sizeof(buf) - 20; i++) {
                    if (i > 0) pos += snprintf(buf + pos, sizeof(buf) - pos, ", ");
                    int64_t s = flux_to_string(elems[i]);
                    const char *sd = flux_string_data(s);
                    uint32_t sl = flux_string_len(s);
                    if (pos + sl < sizeof(buf) - 10) {
                        memcpy(buf + pos, sd, sl);
                        pos += sl;
                    }
                }
                pos += snprintf(buf + pos, sizeof(buf) - pos, "|]");
                return flux_string_new(buf, (uint32_t)pos);
            }
            if (obj == FLUX_OBJ_TUPLE) {
                uint32_t arity = *(uint32_t *)((char *)ptr + 4);
                int64_t *elems = (int64_t *)((char *)ptr + 8);
                char buf[4096];
                int pos = 0;
                pos += snprintf(buf + pos, sizeof(buf) - pos, "(");
                for (uint32_t i = 0; i < arity && pos < (int)sizeof(buf) - 20; i++) {
                    if (i > 0) pos += snprintf(buf + pos, sizeof(buf) - pos, ", ");
                    int64_t s = flux_to_string(elems[i]);
                    const char *sd = flux_string_data(s);
                    uint32_t sl = flux_string_len(s);
                    if (pos + sl < sizeof(buf) - 10) {
                        memcpy(buf + pos, sd, sl);
                        pos += sl;
                    }
                }
                if (arity == 1) pos += snprintf(buf + pos, sizeof(buf) - pos, ",");
                pos += snprintf(buf + pos, sizeof(buf) - pos, ")");
                return flux_string_new(buf, (uint32_t)pos);
            }
            /* ADT or HAMT?  ADTs have obj_tag FLUX_OBJ_ADT; HAMTs don't. */
            if (obj == FLUX_OBJ_ADT) {
                int32_t ctor_tag = *(int32_t *)ptr;
                int32_t field_count = *((int32_t *)ptr + 1);
                /* Some/Left/Right/None */
                if (ctor_tag == 1 && field_count >= 1) {
                    int64_t *fields = (int64_t *)((char *)ptr + 8);
                    char buf[4096]; int pos = 0;
                    pos += snprintf(buf + pos, sizeof(buf) - pos, "Some(");
                    int64_t s = flux_to_string(fields[0]);
                    const char *sd = flux_string_data(s); uint32_t sl = flux_string_len(s);
                    if (pos + sl < sizeof(buf) - 10) { memcpy(buf + pos, sd, sl); pos += sl; }
                    pos += snprintf(buf + pos, sizeof(buf) - pos, ")");
                    return flux_string_new(buf, (uint32_t)pos);
                }
                if (ctor_tag == 2 && field_count >= 1) {
                    int64_t *fields = (int64_t *)((char *)ptr + 8);
                    char buf[4096]; int pos = 0;
                    pos += snprintf(buf + pos, sizeof(buf) - pos, "Left(");
                    int64_t s = flux_to_string(fields[0]);
                    const char *sd = flux_string_data(s); uint32_t sl = flux_string_len(s);
                    if (pos + sl < sizeof(buf) - 10) { memcpy(buf + pos, sd, sl); pos += sl; }
                    pos += snprintf(buf + pos, sizeof(buf) - pos, ")");
                    return flux_string_new(buf, (uint32_t)pos);
                }
                if (ctor_tag == 3 && field_count >= 1) {
                    int64_t *fields = (int64_t *)((char *)ptr + 8);
                    char buf[4096]; int pos = 0;
                    pos += snprintf(buf + pos, sizeof(buf) - pos, "Right(");
                    int64_t s = flux_to_string(fields[0]);
                    const char *sd = flux_string_data(s); uint32_t sl = flux_string_len(s);
                    if (pos + sl < sizeof(buf) - 10) { memcpy(buf + pos, sd, sl); pos += sl; }
                    pos += snprintf(buf + pos, sizeof(buf) - pos, ")");
                    return flux_string_new(buf, (uint32_t)pos);
                }
                if (ctor_tag == 0 && field_count == 0) {
                    return flux_string_new("None", 4);
                }
                /* Cons list: ctor_tag=4, field_count=2 — fall through below */
            } else if (flux_is_hamt(ptr)) {
                return flux_hamt_format(val);
            }
            /* Cons list: ADT with ctor_tag=4, field_count=2 */
            {
                int32_t ctor_tag = *(int32_t *)ptr;
                int32_t field_count = *((int32_t *)ptr + 1);
                if (ctor_tag == 4 && field_count == 2) {
                    char buf[4096];
                    int pos = 0;
                    pos += snprintf(buf + pos, sizeof(buf) - pos, "[");
                    int64_t cur = val;
                    int first_elem = 1;
                    while (flux_is_ptr(cur)) {
                        void *cp = flux_untag_ptr(cur);
                        if (!cp) break;
                        int32_t ct = *(int32_t *)cp;
                        int32_t fc = *((int32_t *)cp + 1);
                        if (ct != 4 || fc != 2) break;
                        int64_t *fields = (int64_t *)((char *)cp + 8);
                        if (!first_elem) pos += snprintf(buf + pos, sizeof(buf) - pos, ", ");
                        first_elem = 0;
                        int64_t elem = fields[0];
                        int is_str = flux_is_ptr(elem) && flux_obj_tag(flux_untag_ptr(elem)) == FLUX_OBJ_STRING;
                        if (is_str) buf[pos++] = '"';
                        int64_t s = flux_to_string(elem);
                        const char *sd = flux_string_data(s);
                        uint32_t sl = flux_string_len(s);
                        if (pos + sl < sizeof(buf) - 10) {
                            memcpy(buf + pos, sd, sl);
                            pos += sl;
                        }
                        if (is_str) buf[pos++] = '"';
                        cur = fields[1];
                    }
                    pos += snprintf(buf + pos, sizeof(buf) - pos, "]");
                    return flux_string_new(buf, (uint32_t)pos);
                }
            }
        }
        return flux_string_new("<value>", 7);
    }
    default: return flux_string_new("<value>", 7);
    }
}

int64_t flux_read_lines(int64_t path) {
    /* Read file, split on newlines, return as Array (matching VM semantics). */
    int64_t content = flux_read_file(path);
    if (flux_is_nanbox(content) && flux_nanbox_tag(content) == FLUX_TAG_NONE) {
        return flux_array_new(NULL, 0);
    }
    const char *data = flux_string_data(content);
    uint32_t len = flux_string_len(content);

    /* Count lines. */
    uint32_t line_count = 0;
    for (uint32_t i = 0; i < len; i++) {
        if (data[i] == '\n') line_count++;
    }
    line_count++; /* last line (or single line without newline) */

    int64_t *lines = (int64_t *)malloc(line_count * sizeof(int64_t));
    uint32_t li = 0;
    uint32_t start = 0;
    for (uint32_t i = 0; i <= len; i++) {
        if (i == len || data[i] == '\n') {
            uint32_t line_len = i - start;
            if (line_len > 0 && data[start + line_len - 1] == '\r') line_len--;
            lines[li++] = flux_string_new(data + start, line_len);
            start = i + 1;
        }
    }

    /* Strip trailing empty line (file ends with \n). */
    if (li > 0) {
        uint32_t last_len = flux_string_len(lines[li - 1]);
        if (last_len == 0) li--;
    }

    int64_t result = flux_array_new(lines, (int32_t)li);
    free(lines);
    return result;
}

/* ── Collection helpers ─────────────────────────────────────────────── */

/* to_list(arr) → converts array to cons list. */
int64_t flux_to_list(int64_t arr_val) {
    if (!flux_is_ptr(arr_val)) return flux_make_empty_list();
    void *ptr = flux_untag_ptr(arr_val);
    if (!ptr) return flux_make_empty_list();
    uint8_t obj = flux_obj_tag(ptr);
    if (obj != FLUX_OBJ_ARRAY) return flux_make_empty_list();

    uint32_t len = *(uint32_t *)((char *)ptr + 4);
    int64_t *elems = (int64_t *)((char *)ptr + 16);

    /* Build cons list from back to front. */
    int64_t list = flux_make_empty_list();
    for (int32_t i = (int32_t)len - 1; i >= 0; i--) {
        void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_ADT);
        *(int32_t *)mem = 4; /* CONS tag */
        *((int32_t *)mem + 1) = 2;
        int64_t *fields = (int64_t *)((char *)mem + 8);
        fields[0] = elems[i];
        fields[1] = list;
        list = flux_tag_ptr(mem);
    }
    return list;
}

/* is_array(val) → bool (true if val is a boxed array). */
int64_t flux_is_array(int64_t val) {
    if (!flux_is_ptr(val)) return flux_make_bool(0);
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return flux_make_bool(0);
    return flux_make_bool(flux_obj_tag(ptr) == FLUX_OBJ_ARRAY);
}

/* is_map(val) → bool (true if val is a HAMT map).
 * HAMTs don't have a FLUX_OBJ_* tag, so we check if it's a boxed pointer
 * that is NOT any of the known object types. */
int64_t flux_is_map(int64_t val) {
    if (!flux_is_ptr(val)) return flux_make_bool(0);
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return flux_make_bool(0);
    /* HAMT nodes have no FluxHeader — identify by structural checks only. */
    return flux_make_bool(flux_is_hamt(ptr));
}

/* len/length — returns length of array, string, list, tuple, or map. */
int64_t flux_rt_len(int64_t collection) {
    /* Check boxed pointer first — boxed values are also NaN-boxed. */
    if (flux_is_ptr(collection)) {
        void *ptr = flux_untag_ptr(collection);
        if (!ptr) return flux_tag_int(0);
        uint8_t obj = flux_obj_tag(ptr);
        if (obj == FLUX_OBJ_ARRAY) {
            uint32_t len = *(uint32_t *)((char *)ptr + 4);
            return flux_tag_int((int64_t)len);
        }
        if (obj == FLUX_OBJ_STRING) {
            return flux_string_length(collection);
        }
        if (obj == FLUX_OBJ_TUPLE) {
            uint32_t arity = *(uint32_t *)((char *)ptr + 4);
            return flux_tag_int((int64_t)arity);
        }
        /* Cons list: count nodes. */
        int64_t count = 0;
        int64_t cur = collection;
        while (flux_is_ptr(cur)) {
            void *cp = flux_untag_ptr(cur);
            int32_t ct = *(int32_t *)cp;
            if (ct != 4) break;
            count++;
            cur = ((int64_t *)((char *)cp + 8))[1];
        }
        return flux_tag_int(count);
    }
    /* NaN-boxed non-pointer (empty list, integer, etc.) */
    if (flux_is_nanbox(collection) && flux_nanbox_tag(collection) == FLUX_TAG_EMPTY_LIST)
        return flux_tag_int(0);
    return flux_tag_int(0);
}

/* to_array(list) → array. Converts cons list to array. */
int64_t flux_to_array(int64_t list) {
    /* Count elements. */
    int64_t count = 0;
    int64_t cur = list;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        int32_t ct = *(int32_t *)cp;
        if (ct != 4) break;
        count++;
        cur = ((int64_t *)((char *)cp + 8))[1];
    }
    if (count == 0) return flux_array_new(NULL, 0);
    int64_t *elems = (int64_t *)malloc(count * sizeof(int64_t));
    cur = list;
    for (int64_t i = 0; i < count; i++) {
        void *cp = flux_untag_ptr(cur);
        int64_t *fields = (int64_t *)((char *)cp + 8);
        elems[i] = fields[0];
        cur = fields[1];
    }
    int64_t result = flux_array_new(elems, (int32_t)count);
    free(elems);
    return result;
}

/* is_list(val) → bool. */
int64_t flux_is_list(int64_t val) {
    if (flux_is_nanbox(val) && flux_nanbox_tag(val) == FLUX_TAG_EMPTY_LIST)
        return flux_make_bool(1);
    if (!flux_is_ptr(val)) return flux_make_bool(0);
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return flux_make_bool(0);
    /* Cons list has ctor_tag=4 */
    int32_t ct = *(int32_t *)ptr;
    return flux_make_bool(ct == 4);
}

/* is_some(val) → bool. */
int64_t flux_is_some(int64_t val) {
    if (!flux_is_ptr(val)) return flux_make_bool(0);
    void *ptr = flux_untag_ptr(val);
    if (!ptr) return flux_make_bool(0);
    int32_t ct = *(int32_t *)ptr;
    return flux_make_bool(ct == 1); /* ctor_tag 1 = Some */
}

/* unwrap(option) → value or panic. */
int64_t flux_unwrap(int64_t val) {
    if (flux_is_ptr(val)) {
        void *ptr = flux_untag_ptr(val);
        if (ptr) {
            int32_t ct = *(int32_t *)ptr;
            if (ct == 1) { /* Some */
                int64_t *fields = (int64_t *)((char *)ptr + 8);
                return fields[0];
            }
        }
    }
    flux_panic(flux_string_new("unwrap called on None", 21));
    return flux_make_none(); /* unreachable */
}

/* unwrap_or(option, default) → value or default. */
int64_t flux_unwrap_or(int64_t val, int64_t def) {
    if (flux_is_ptr(val)) {
        void *ptr = flux_untag_ptr(val);
        if (ptr) {
            int32_t ct = *(int32_t *)ptr;
            if (ct == 1) { /* Some */
                int64_t *fields = (int64_t *)((char *)ptr + 8);
                return fields[0];
            }
        }
    }
    return def;
}

/* split(s, delim) → array of strings. */
int64_t flux_split(int64_t s, int64_t delim) {
    const char *s_data = flux_string_data(s);
    uint32_t s_len = flux_string_len(s);
    const char *d_data = flux_string_data(delim);
    uint32_t d_len = flux_string_len(delim);

    /* Edge case: empty delimiter → return array of single-char strings. */
    if (d_len == 0) {
        int64_t *parts = (int64_t *)malloc(s_len * sizeof(int64_t));
        for (uint32_t i = 0; i < s_len; i++) {
            parts[i] = flux_string_new(s_data + i, 1);
        }
        int64_t result = flux_array_new(parts, (int32_t)s_len);
        free(parts);
        return result;
    }

    /* Count splits first. */
    uint32_t count = 1;
    for (uint32_t i = 0; i + d_len <= s_len; i++) {
        if (memcmp(s_data + i, d_data, d_len) == 0) {
            count++;
            i += d_len - 1;
        }
    }

    int64_t *parts = (int64_t *)malloc(count * sizeof(int64_t));
    uint32_t pi = 0;
    uint32_t start = 0;
    for (uint32_t i = 0; i + d_len <= s_len; i++) {
        if (memcmp(s_data + i, d_data, d_len) == 0) {
            parts[pi++] = flux_string_new(s_data + start, i - start);
            i += d_len - 1;
            start = i + 1;
        }
    }
    /* Last segment. */
    parts[pi++] = flux_string_new(s_data + start, s_len - start);

    int64_t result = flux_array_new(parts, (int32_t)pi);
    free(parts);
    return result;
}

/* ── Main entry point wrapper ───────────────────────────────────────── */

/* ── String join ───────────────────────────────────────────────────── */

int64_t flux_join(int64_t list, int64_t sep) {
    const char *sep_data = flux_string_data(sep);
    uint32_t sep_len = flux_string_len(sep);

    /* Collect strings from array or cons list. */
    char buf[8192];
    int pos = 0;
    int first = 1;

    int64_t *elems; uint32_t len;
    if (flux_is_ptr(list)) {
        void *ptr = flux_untag_ptr(list);
        if (ptr && flux_obj_tag(ptr) == FLUX_OBJ_ARRAY) {
            len = *(uint32_t *)((char *)ptr + 4);
            elems = (int64_t *)((char *)ptr + 16);
            for (uint32_t i = 0; i < len && pos < (int)sizeof(buf) - 200; i++) {
                if (!first && sep_len > 0) { memcpy(buf + pos, sep_data, sep_len); pos += sep_len; }
                first = 0;
                const char *sd = flux_string_data(elems[i]);
                uint32_t sl = flux_string_len(elems[i]);
                if (pos + (int)sl < (int)sizeof(buf) - 10) { memcpy(buf + pos, sd, sl); pos += sl; }
            }
            return flux_string_new(buf, (uint32_t)pos);
        }
    }
    /* Cons list. */
    int64_t cur = list;
    while (flux_is_ptr(cur) && pos < (int)sizeof(buf) - 200) {
        void *cp = flux_untag_ptr(cur);
        int32_t ct = *(int32_t *)cp;
        if (ct != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        if (!first && sep_len > 0) { memcpy(buf + pos, sep_data, sep_len); pos += sep_len; }
        first = 0;
        const char *sd = flux_string_data(fields[0]);
        uint32_t sl = flux_string_len(fields[0]);
        if (pos + (int)sl < (int)sizeof(buf) - 10) { memcpy(buf + pos, sd, sl); pos += sl; }
        cur = fields[1];
    }
    return flux_string_new(buf, (uint32_t)pos);
}

/* ── Simple numeric helpers ────────────────────────────────────────── */

int64_t flux_sum(int64_t collection) {
    int64_t *elems; uint32_t len;
    if (flux_is_ptr(collection)) {
        void *ptr = flux_untag_ptr(collection);
        if (ptr && flux_obj_tag(ptr) == FLUX_OBJ_ARRAY) {
            len = *(uint32_t *)((char *)ptr + 4);
            elems = (int64_t *)((char *)ptr + 16);
            int64_t acc = flux_tag_int(0);
            for (uint32_t i = 0; i < len; i++) acc = flux_rt_add(acc, elems[i]);
            return acc;
        }
    }
    /* Cons list. */
    int64_t acc = flux_tag_int(0);
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (*(int32_t *)cp != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        acc = flux_rt_add(acc, fields[0]);
        cur = fields[1];
    }
    return acc;
}

/* ── String helpers ────────────────────────────────────────────────── */

int64_t flux_starts_with(int64_t s, int64_t prefix) {
    const char *sd = flux_string_data(s);
    uint32_t sl = flux_string_len(s);
    const char *pd = flux_string_data(prefix);
    uint32_t pl = flux_string_len(prefix);
    if (pl > sl) return flux_make_bool(0);
    return flux_make_bool(memcmp(sd, pd, pl) == 0);
}

int64_t flux_ends_with(int64_t s, int64_t suffix) {
    const char *sd = flux_string_data(s);
    uint32_t sl = flux_string_len(s);
    const char *xd = flux_string_data(suffix);
    uint32_t xl = flux_string_len(suffix);
    if (xl > sl) return flux_make_bool(0);
    return flux_make_bool(memcmp(sd + sl - xl, xd, xl) == 0);
}

/* split_ints(s, delim) → array of ints parsed from split string. */
int64_t flux_split_ints(int64_t s, int64_t delim) {
    int64_t parts = flux_split(s, delim);
    int64_t *elems; uint32_t len;
    if (!flux_is_ptr(parts)) return flux_array_new(NULL, 0);
    void *ptr = flux_untag_ptr(parts);
    if (!ptr || flux_obj_tag(ptr) != FLUX_OBJ_ARRAY) return flux_array_new(NULL, 0);
    len = *(uint32_t *)((char *)ptr + 4);
    elems = (int64_t *)((char *)ptr + 16);
    int64_t *ints = (int64_t *)malloc(len * sizeof(int64_t));
    uint32_t count = 0;
    for (uint32_t i = 0; i < len; i++) {
        /* Skip empty strings. */
        if (flux_string_len(elems[i]) == 0) continue;
        ints[count++] = flux_parse_int(elems[i]);
    }
    int64_t result = flux_array_new(ints, (int32_t)count);
    free(ints);
    return result;
}

/* ── Higher-order functions ─────────────────────────────────────────── */
/* These call Flux closures via the ccc trampoline flux_call_closure_c. */

static int64_t call1(int64_t func, int64_t arg) {
    /* Dup the arg: the Aether-compiled closure may drop its parameter
     * (e.g. `\_ -> true`), which would free a heap object still owned
     * by the calling collection. */
    flux_dup(arg);
    int64_t args[1] = { arg };
    return flux_call_closure_c(func, args, 1);
}

static int64_t call2(int64_t func, int64_t a, int64_t b) {
    flux_dup(a);
    flux_dup(b);
    int64_t args[2] = { a, b };
    return flux_call_closure_c(func, args, 2);
}

/* Helper: get array elements pointer and length. */
static int flux_get_array_elems(int64_t arr, int64_t **out_elems, uint32_t *out_len) {
    if (!flux_is_ptr(arr)) return 0;
    void *ptr = flux_untag_ptr(arr);
    if (!ptr || flux_obj_tag(ptr) != FLUX_OBJ_ARRAY) return 0;
    *out_len = *(uint32_t *)((char *)ptr + 4);
    *out_elems = (int64_t *)((char *)ptr + 16);
    return 1;
}

int64_t flux_ho_map(int64_t collection, int64_t func) {
    int64_t *elems; uint32_t len;
    if (flux_get_array_elems(collection, &elems, &len)) {
        int64_t *results = (int64_t *)malloc(len * sizeof(int64_t));
        for (uint32_t i = 0; i < len; i++) {
            results[i] = call1(func, elems[i]);
        }
        int64_t result = flux_array_new(results, (int32_t)len);
        free(results);
        return result;
    }
    /* Cons list: map and build new list. */
    int64_t cur = collection;
    /* Collect into temp array, then build cons list. */
    uint32_t count = 0;
    int64_t temp_cur = cur;
    while (flux_is_ptr(temp_cur)) {
        void *cp = flux_untag_ptr(temp_cur);
        int32_t ct = *(int32_t *)cp;
        if (ct != 4) break;
        count++;
        temp_cur = ((int64_t *)((char *)cp + 8))[1];
    }
    if (count == 0) return flux_make_empty_list();
    int64_t *mapped = (int64_t *)malloc(count * sizeof(int64_t));
    for (uint32_t i = 0; i < count; i++) {
        void *cp = flux_untag_ptr(cur);
        int64_t *fields = (int64_t *)((char *)cp + 8);
        mapped[i] = call1(func, fields[0]);
        cur = fields[1];
    }
    /* Build cons list from back to front. */
    int64_t list = flux_make_empty_list();
    for (int32_t i = (int32_t)count - 1; i >= 0; i--) {
        void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_ADT);
        *(int32_t *)mem = 4;
        *((int32_t *)mem + 1) = 2;
        int64_t *f = (int64_t *)((char *)mem + 8);
        f[0] = mapped[i];
        f[1] = list;
        list = flux_tag_ptr(mem);
    }
    free(mapped);
    return list;
}

int64_t flux_ho_filter(int64_t collection, int64_t func) {
    int64_t *elems; uint32_t len;
    if (flux_get_array_elems(collection, &elems, &len)) {
        int64_t *results = (int64_t *)malloc(len * sizeof(int64_t));
        uint32_t count = 0;
        for (uint32_t i = 0; i < len; i++) {
            int64_t keep = call1(func, elems[i]);
            if (keep == flux_make_bool(1)) {
                results[count++] = elems[i];
            }
        }
        int64_t result = flux_array_new(results, (int32_t)count);
        free(results);
        return result;
    }
    /* Cons list filter. */
    int64_t cur = collection;
    uint32_t cap = 64;
    int64_t *kept = (int64_t *)malloc(cap * sizeof(int64_t));
    uint32_t count = 0;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        int32_t ct = *(int32_t *)cp;
        if (ct != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        int64_t keep = call1(func, fields[0]);
        if (keep == flux_make_bool(1)) {
            if (count >= cap) { cap *= 2; kept = (int64_t *)realloc(kept, cap * sizeof(int64_t)); }
            kept[count++] = fields[0];
        }
        cur = fields[1];
    }
    /* Build cons list. */
    int64_t list = flux_make_empty_list();
    for (int32_t i = (int32_t)count - 1; i >= 0; i--) {
        void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_ADT);
        *(int32_t *)mem = 4;
        *((int32_t *)mem + 1) = 2;
        int64_t *f = (int64_t *)((char *)mem + 8);
        f[0] = kept[i];
        f[1] = list;
        list = flux_tag_ptr(mem);
    }
    free(kept);
    return list;
}

int64_t flux_ho_any(int64_t collection, int64_t func) {
    int64_t *elems; uint32_t len;
    if (flux_get_array_elems(collection, &elems, &len)) {
        for (uint32_t i = 0; i < len; i++) {
            int64_t r = call1(func, elems[i]);
            if (r == flux_make_bool(1)) return flux_make_bool(1);
        }
        return flux_make_bool(0);
    }
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (*(int32_t *)cp != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        int64_t r = call1(func, fields[0]);
        if (r == flux_make_bool(1)) return flux_make_bool(1);
        cur = fields[1];
    }
    return flux_make_bool(0);
}

int64_t flux_ho_all(int64_t collection, int64_t func) {
    int64_t *elems; uint32_t len;
    if (flux_get_array_elems(collection, &elems, &len)) {
        for (uint32_t i = 0; i < len; i++) {
            int64_t r = call1(func, elems[i]);
            if (r != flux_make_bool(1)) return flux_make_bool(0);
        }
        return flux_make_bool(1);
    }
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (*(int32_t *)cp != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        int64_t r = call1(func, fields[0]);
        if (r != flux_make_bool(1)) return flux_make_bool(0);
        cur = fields[1];
    }
    return flux_make_bool(1);
}

int64_t flux_ho_fold(int64_t collection, int64_t init, int64_t func) {
    int64_t acc = init;
    int64_t *elems; uint32_t len;
    if (flux_get_array_elems(collection, &elems, &len)) {
        for (uint32_t i = 0; i < len; i++) {
            acc = call2(func, acc, elems[i]);
        }
        return acc;
    }
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (*(int32_t *)cp != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        acc = call2(func, acc, fields[0]);
        cur = fields[1];
    }
    return acc;
}

int64_t flux_ho_each(int64_t collection, int64_t func) {
    int64_t *elems; uint32_t len;
    if (flux_get_array_elems(collection, &elems, &len)) {
        for (uint32_t i = 0; i < len; i++) call1(func, elems[i]);
        return flux_make_none();
    }
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (*(int32_t *)cp != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        call1(func, fields[0]);
        cur = fields[1];
    }
    return flux_make_none();
}

int64_t flux_ho_find(int64_t collection, int64_t func) {
    int64_t *elems; uint32_t len;
    if (flux_get_array_elems(collection, &elems, &len)) {
        for (uint32_t i = 0; i < len; i++) {
            int64_t r = call1(func, elems[i]);
            if (r == flux_make_bool(1)) return flux_wrap_some(elems[i]);
        }
        return flux_make_none();
    }
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (*(int32_t *)cp != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        int64_t r = call1(func, fields[0]);
        if (r == flux_make_bool(1)) return flux_wrap_some(fields[0]);
        cur = fields[1];
    }
    return flux_make_none();
}

/* sort_default(collection) — sort by natural int ordering. */
int64_t flux_ho_count(int64_t collection, int64_t func) {
    int64_t *elems; uint32_t len;
    if (flux_get_array_elems(collection, &elems, &len)) {
        int64_t c = 0;
        for (uint32_t i = 0; i < len; i++) {
            if (call1(func, elems[i]) == flux_make_bool(1)) c++;
        }
        return flux_tag_int(c);
    }
    int64_t c = 0;
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (*(int32_t *)cp != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        if (call1(func, fields[0]) == flux_make_bool(1)) c++;
        cur = fields[1];
    }
    return flux_tag_int(c);
}

/* sort_by(collection, key_fn) — sort by key function result. */
int64_t flux_ho_sort_by(int64_t collection, int64_t func) {
    int64_t *elems; uint32_t len;
    if (!flux_get_array_elems(collection, &elems, &len)) return collection;
    if (len <= 1) return collection;

    /* Compute keys, then sort by keys. */
    int64_t *sorted = (int64_t *)malloc(len * sizeof(int64_t));
    int64_t *keys_arr = (int64_t *)malloc(len * sizeof(int64_t));
    memcpy(sorted, elems, len * sizeof(int64_t));
    for (uint32_t i = 0; i < len; i++) {
        keys_arr[i] = call1(func, sorted[i]);
    }

    /* Insertion sort by key. */
    for (uint32_t i = 1; i < len; i++) {
        int64_t key = keys_arr[i];
        int64_t val = sorted[i];
        int32_t j = (int32_t)i - 1;
        while (j >= 0 && flux_rt_gt(keys_arr[j], key) == flux_make_bool(1)) {
            keys_arr[j + 1] = keys_arr[j];
            sorted[j + 1] = sorted[j];
            j--;
        }
        keys_arr[j + 1] = key;
        sorted[j + 1] = val;
    }
    int64_t result = flux_array_new(sorted, (int32_t)len);
    free(sorted);
    free(keys_arr);
    return result;
}

/* zip(a, b) → array of tuples [(a[0],b[0]), (a[1],b[1]), ...]. */
int64_t flux_zip(int64_t a, int64_t b) {
    int64_t *ea, *eb; uint32_t la, lb;
    if (!flux_get_array_elems(a, &ea, &la)) return flux_array_new(NULL, 0);
    if (!flux_get_array_elems(b, &eb, &lb)) return flux_array_new(NULL, 0);
    uint32_t len = la < lb ? la : lb;
    int64_t *tuples = (int64_t *)malloc(len * sizeof(int64_t));
    for (uint32_t i = 0; i < len; i++) {
        /* Build 2-tuple: { obj_tag=F3, pad[3], arity=2, elem0, elem1 } */
        void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_TUPLE);
        *(uint8_t *)mem = FLUX_OBJ_TUPLE;
        *(uint32_t *)((char *)mem + 4) = 2;
        int64_t *fields = (int64_t *)((char *)mem + 8);
        fields[0] = ea[i];
        fields[1] = eb[i];
        tuples[i] = flux_tag_ptr(mem);
    }
    int64_t result = flux_array_new(tuples, (int32_t)len);
    free(tuples);
    return result;
}

int64_t flux_sort_default(int64_t collection) {
    int64_t *elems; uint32_t len;
    if (!flux_get_array_elems(collection, &elems, &len)) return collection;
    if (len <= 1) return collection;

    int64_t *sorted = (int64_t *)malloc(len * sizeof(int64_t));
    memcpy(sorted, elems, len * sizeof(int64_t));

    /* Insertion sort by NaN-boxed int comparison. */
    for (uint32_t i = 1; i < len; i++) {
        int64_t key = sorted[i];
        int32_t j = (int32_t)i - 1;
        while (j >= 0) {
            int64_t cmp = flux_rt_gt(sorted[j], key);
            if (cmp != flux_make_bool(1)) break;
            sorted[j + 1] = sorted[j];
            j--;
        }
        sorted[j + 1] = key;
    }
    int64_t result = flux_array_new(sorted, (int32_t)len);
    free(sorted);
    return result;
}

/* sort(collection, comparator) — insertion sort using comparator. */
int64_t flux_ho_sort(int64_t collection, int64_t func) {
    int64_t *elems; uint32_t len;
    if (!flux_get_array_elems(collection, &elems, &len)) return collection;
    if (len <= 1) return collection;

    /* Copy elements. */
    int64_t *sorted = (int64_t *)malloc(len * sizeof(int64_t));
    memcpy(sorted, elems, len * sizeof(int64_t));

    /* Insertion sort using comparator. */
    for (uint32_t i = 1; i < len; i++) {
        int64_t key = sorted[i];
        int32_t j = (int32_t)i - 1;
        while (j >= 0) {
            int64_t cmp = call2(func, sorted[j], key);
            /* If cmp > 0, shift right. */
            int64_t cmp_result = flux_rt_gt(cmp, flux_tag_int(0));
            if (cmp_result != flux_make_bool(1)) break;
            sorted[j + 1] = sorted[j];
            j--;
        }
        sorted[j + 1] = key;
    }
    int64_t result = flux_array_new(sorted, (int32_t)len);
    free(sorted);
    return result;
}

/* flatten(collection) — flatten array of arrays into single array. */
int64_t flux_flatten(int64_t collection) {
    int64_t *elems; uint32_t len;
    if (!flux_get_array_elems(collection, &elems, &len))
        return flux_array_new(NULL, 0);

    /* First pass: count total elements. */
    uint32_t total = 0;
    for (uint32_t i = 0; i < len; i++) {
        int64_t *sub_elems; uint32_t sub_len;
        if (flux_get_array_elems(elems[i], &sub_elems, &sub_len)) {
            total += sub_len;
        } else {
            total++; /* non-array element kept as-is */
        }
    }

    int64_t *result = (int64_t *)malloc(total * sizeof(int64_t));
    uint32_t idx = 0;
    for (uint32_t i = 0; i < len; i++) {
        int64_t *sub_elems; uint32_t sub_len;
        if (flux_get_array_elems(elems[i], &sub_elems, &sub_len)) {
            memcpy(result + idx, sub_elems, sub_len * sizeof(int64_t));
            idx += sub_len;
        } else {
            result[idx++] = elems[i];
        }
    }
    int64_t out = flux_array_new(result, (int32_t)total);
    free(result);
    return out;
}

/* flat_map(collection, func) — map each element, concat results. */
int64_t flux_ho_flat_map(int64_t collection, int64_t func) {
    /* For arrays, map+flatten works. */
    int64_t *elems; uint32_t len;
    if (flux_get_array_elems(collection, &elems, &len)) {
        int64_t mapped = flux_ho_map(collection, func);
        return flux_flatten(mapped);
    }
    /* For cons lists: iterate, call func on each element, concat sub-lists. */
    /* Collect all sub-list elements into a buffer, then build result cons list. */
    uint32_t cap = 64;
    int64_t *buf = (int64_t *)malloc(cap * sizeof(int64_t));
    uint32_t count = 0;
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (!cp || *(int32_t *)cp != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        flux_dup(fields[0]);
        int64_t sub = call1(func, fields[0]);
        /* Walk the sub-list and collect elements. */
        int64_t sc = sub;
        while (flux_is_ptr(sc)) {
            void *sp = flux_untag_ptr(sc);
            if (!sp || *(int32_t *)sp != 4) break;
            int64_t *sf = (int64_t *)((char *)sp + 8);
            if (count >= cap) { cap *= 2; buf = (int64_t *)realloc(buf, cap * sizeof(int64_t)); }
            buf[count++] = sf[0];
            sc = sf[1];
        }
        cur = fields[1];
    }
    /* Build cons list from back to front. */
    int64_t result = flux_make_empty_list();
    for (int32_t i = (int32_t)count - 1; i >= 0; i--) {
        void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_ADT);
        *(int32_t *)mem = 4;
        *((int32_t *)mem + 1) = 2;
        int64_t *f = (int64_t *)((char *)mem + 8);
        f[0] = buf[i];
        f[1] = result;
        result = flux_tag_ptr(mem);
    }
    free(buf);
    return result;
}

/* reverse(collection) — polymorphic reverse for arrays and cons lists. */
int64_t flux_reverse(int64_t collection) {
    if (flux_is_ptr(collection)) {
        void *ptr = flux_untag_ptr(collection);
        if (ptr && flux_obj_tag(ptr) == FLUX_OBJ_ARRAY) {
            return flux_array_reverse(collection);
        }
    }
    /* Cons list: collect elements, build reversed list. */
    int64_t cur = collection;
    int64_t reversed = flux_make_empty_list();
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (!cp) break;
        int32_t ct = *(int32_t *)cp;
        if (ct != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        /* Cons(head, reversed) builds the reversed list. */
        void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_ADT);
        *(int32_t *)mem = 4;
        *((int32_t *)mem + 1) = 2;
        int64_t *f = (int64_t *)((char *)mem + 8);
        f[0] = fields[0];
        f[1] = reversed;
        reversed = flux_tag_ptr(mem);
        cur = fields[1];
    }
    return reversed;
}

/* contains(collection, value) — polymorphic contains. */
int64_t flux_contains(int64_t collection, int64_t value) {
    if (flux_is_ptr(collection)) {
        void *ptr = flux_untag_ptr(collection);
        if (ptr && flux_obj_tag(ptr) == FLUX_OBJ_ARRAY) {
            return flux_array_contains(collection, value);
        }
    }
    /* Cons list: walk and compare. */
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (!cp) break;
        int32_t ct = *(int32_t *)cp;
        if (ct != 4) break;
        int64_t *fields = (int64_t *)((char *)cp + 8);
        if (fields[0] == value) return flux_make_bool(1);
        if (flux_is_ptr(fields[0]) && flux_is_ptr(value)) {
            if (flux_string_eq(fields[0], value)) return flux_make_bool(1);
        }
        cur = fields[1];
    }
    return flux_make_bool(0);
}

/* Globals table for LIR native backend.
 * Populated by module init functions before flux_main runs.
 * TODO: replace with proper linker-based symbol resolution. */
static int64_t flux_globals[256];

int64_t flux_get_global(int64_t idx) {
    if (idx >= 0 && idx < 256) return flux_globals[idx];
    return flux_make_none();
}

void flux_set_global(int64_t idx, int64_t val) {
    if (idx >= 0 && idx < 256) flux_globals[idx] = val;
}

#ifndef FLUX_RT_NO_MAIN

/*
 * The LLVM codegen emits a `@flux_main() -> i64` function.
 * This C main() initializes the runtime, calls flux_main, and shuts down.
 */
extern int64_t flux_main(void);

int main(void) {
    flux_rt_init();
    int64_t result = flux_main();
    (void)result;
    flux_rt_shutdown();
    return 0;
}

#endif /* FLUX_RT_NO_MAIN */
