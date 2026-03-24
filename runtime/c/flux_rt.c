/*
 * flux_rt.c — Flux runtime core: init, shutdown, print, I/O.
 *
 * All values are NaN-boxed i64.  See flux_rt.h for the encoding.
 */

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <time.h>

/* ── Forward declarations for string helpers (string.c) ─────────────── */

extern const char *flux_string_data(int64_t s);
extern uint32_t    flux_string_len(int64_t s);
extern int64_t     flux_string_new(const char *data, uint32_t len);

/* ── Runtime lifecycle ──────────────────────────────────────────────── */

void flux_rt_init(void) {
    flux_gc_init(0); /* 0 → default 4 MB */
}

void flux_rt_shutdown(void) {
    flux_gc_shutdown();
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
        printf("%g", d);
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

    case FLUX_TAG_BOXED_VALUE: {
        void *ptr = flux_untag_ptr(val);
        if (!ptr) {
            printf("<null>");
            break;
        }

        uint8_t obj = flux_obj_tag(ptr);
        if (obj == FLUX_OBJ_STRING) {
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
        } else {
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
    if (rb == 0) return flux_tag_int(0);
    return flux_tag_int(flux_untag_int(a) / rb);
}

int64_t flux_rt_mod(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        return flux_from_double(fmod(flux_as_double(a), flux_as_double(b)));
    }
    int64_t rb = flux_untag_int(b);
    if (rb == 0) return flux_tag_int(0);
    return flux_tag_int(flux_untag_int(a) % rb);
}

int64_t flux_rt_neg(int64_t a) {
    if (flux_val_is_float(a)) {
        return flux_from_double(-flux_as_double(a));
    }
    return flux_tag_int(-flux_untag_int(a));
}

/* ── Runtime-dispatching comparisons ────────────────────────────────── */

int64_t flux_rt_eq(int64_t a, int64_t b) {
    /* Fast path: bitwise equal (same int, bool, None, or same pointer). */
    if (a == b) return flux_make_bool(1);
    /* String structural equality. */
    if (flux_is_ptr(a) && flux_is_ptr(b)) {
        void *pa = flux_untag_ptr(a);
        void *pb = flux_untag_ptr(b);
        if (pa && pb && flux_obj_tag(pa) == FLUX_OBJ_STRING
            && flux_obj_tag(pb) == FLUX_OBJ_STRING) {
            return flux_make_bool(flux_string_eq(a, b));
        }
        /* Tuple structural equality. */
        if (pa && pb && flux_obj_tag(pa) == FLUX_OBJ_TUPLE
            && flux_obj_tag(pb) == FLUX_OBJ_TUPLE) {
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
        /* flux_array_get returns raw value or None for out-of-bounds.
         * VM's Index wraps in Some() for safe indexing. */
        if (flux_is_nanbox(result) && flux_nanbox_tag(result) == FLUX_TAG_NONE) {
            return flux_make_none();
        }
        void *mem = flux_gc_alloc(8 + 8);
        int32_t *hdr = (int32_t *)mem;
        hdr[0] = 1; /* ctor_tag = Some */
        hdr[1] = 1; /* field_count = 1 */
        int64_t *fields = (int64_t *)((char *)mem + 8);
        fields[0] = result;
        return flux_tag_ptr(mem);
    }
    case FLUX_OBJ_TUPLE: {
        /* Tuple indexing: return Some(field) or None. */
        uint32_t arity = *(uint32_t *)((char *)ptr + 4);
        int64_t idx = flux_untag_int(key);
        if (idx < 0 || (uint32_t)idx >= arity) return flux_make_none();
        int64_t *elems = (int64_t *)((char *)ptr + 8);
        /* Wrap in Some — matches VM semantics for safe indexing. */
        /* Allocate a 1-field ADT with tag=1 (Some). */
        void *mem = flux_gc_alloc(8 + 8);
        int32_t *hdr = (int32_t *)mem;
        hdr[0] = 1; /* ctor_tag = Some */
        hdr[1] = 1; /* field_count = 1 */
        int64_t *fields = (int64_t *)((char *)mem + 8);
        fields[0] = elems[idx];
        return flux_tag_ptr(mem);
    }
    default: {
        /* Assume HAMT for any other boxed value. */
        int64_t result = flux_hamt_get(collection, key);
        /* flux_hamt_get returns raw value or None.
         * VM's Index wraps in Some() for safe indexing. */
        if (flux_is_nanbox(result) && flux_nanbox_tag(result) == FLUX_TAG_NONE) {
            return flux_make_none();
        }
        /* Wrap in Some(value) — allocate 1-field ADT with tag=1 */
        void *mem = flux_gc_alloc(8 + 8);
        int32_t *hdr = (int32_t *)mem;
        hdr[0] = 1; /* ctor_tag = Some */
        hdr[1] = 1; /* field_count = 1 */
        int64_t *fields = (int64_t *)((char *)mem + 8);
        fields[0] = result;
        return flux_tag_ptr(mem);
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
            case FLUX_OBJ_STRING:  return flux_string_new("String", 6);
            case FLUX_OBJ_ARRAY:   return flux_string_new("Array", 5);
            case FLUX_OBJ_TUPLE:   return flux_string_new("Tuple", 5);
            case FLUX_OBJ_CLOSURE: return flux_string_new("Function", 8);
            default: break;
            }
        }
        return flux_string_new("Object", 6);
    }
    default:                     return flux_string_new("Unknown", 7);
    }
}

int64_t flux_is_int(int64_t val) {
    if (!flux_is_nanbox(val)) return flux_make_bool(0);
    return flux_make_bool(flux_nanbox_tag(val) == FLUX_TAG_INTEGER);
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
    abort();
}

int64_t flux_clock_now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    int64_t ms = (int64_t)ts.tv_sec * 1000 + (int64_t)ts.tv_nsec / 1000000;
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

int64_t flux_substring(int64_t s, int64_t start_val, int64_t end_val) {
    return flux_string_slice(s, start_val, end_val);
}

int64_t flux_parse_int(int64_t s) {
    return flux_string_to_int(s);
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
        }
        return flux_string_new("<value>", 7);
    }
    default: return flux_string_new("<value>", 7);
    }
}

int64_t flux_read_lines(int64_t path) {
    /* Read file, then split on newlines into a cons list. */
    int64_t content = flux_read_file(path);
    if (flux_nanbox_tag(content) == FLUX_TAG_NONE) {
        return flux_make_empty_list();
    }
    const char *data = flux_string_data(content);
    uint32_t len = flux_string_len(content);

    /* Build list in reverse, then reverse. */
    int64_t list = flux_make_empty_list();
    uint32_t start = 0;
    for (uint32_t i = 0; i <= len; i++) {
        if (i == len || data[i] == '\n') {
            uint32_t line_len = i - start;
            /* Skip trailing \r. */
            if (line_len > 0 && data[start + line_len - 1] == '\r') line_len--;
            int64_t line = flux_string_new(data + start, line_len);
            /* Cons onto front. */
            int64_t fields[2];
            fields[0] = line;
            fields[1] = list;
            /* Build a 2-field ADT with CONS tag (4). */
            list = flux_tag_ptr(flux_gc_alloc(sizeof(int32_t) * 2 + sizeof(int64_t) * 2));
            void *ptr = flux_untag_ptr(list);
            *(int32_t *)ptr = 4; /* CONS_TAG */
            *((int32_t *)ptr + 1) = 2; /* field count */
            int64_t *fptr = (int64_t *)((char *)ptr + 8);
            fptr[0] = line;
            fptr[1] = flux_make_empty_list(); /* placeholder */
            start = i + 1;
        }
    }
    /* TODO: proper cons list building requires reverse. For now, rebuild forward. */
    /* Rebuild forward by collecting lines first. */
    uint32_t line_count = 0;
    for (uint32_t i = 0; i < len; i++) {
        if (data[i] == '\n') line_count++;
    }
    line_count++; /* last line */

    /* Allocate temporary array of lines. */
    int64_t *lines = (int64_t *)malloc(line_count * sizeof(int64_t));
    uint32_t li = 0;
    start = 0;
    for (uint32_t i = 0; i <= len; i++) {
        if (i == len || data[i] == '\n') {
            uint32_t line_len = i - start;
            if (line_len > 0 && data[start + line_len - 1] == '\r') line_len--;
            lines[li++] = flux_string_new(data + start, line_len);
            start = i + 1;
        }
    }

    /* Build cons list from back to front. */
    list = flux_make_empty_list();
    for (int32_t i = (int32_t)li - 1; i >= 0; i--) {
        /* Use flux_gc_alloc for ADT: tag(4) + field_count(4) + 2*i64 */
        void *mem = flux_gc_alloc(8 + 2 * 8);
        *(int32_t *)mem = 4; /* CONS tag */
        *((int32_t *)mem + 1) = 2;
        int64_t *fields_p = (int64_t *)((char *)mem + 8);
        fields_p[0] = lines[i];
        fields_p[1] = list;
        list = flux_tag_ptr(mem);
    }
    free(lines);
    return list;
}

/* ── Main entry point wrapper ───────────────────────────────────────── */

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
