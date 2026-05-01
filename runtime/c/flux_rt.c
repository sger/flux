/*
 * flux_rt.c — Flux runtime core: init, shutdown, print, I/O.
 *
 * All values are pointer-tagged i64.  See flux_rt.h for the encoding.
 */

// Expose POSIX APIs (clock_gettime, etc.) on Linux/glibc.
#if !defined(_POSIX_C_SOURCE) && !defined(__APPLE__)
#define _POSIX_C_SOURCE 199309L
#endif

#include "flux_rt.h"
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <time.h>
#include <setjmp.h>
#if defined(_MSC_VER) || defined(_WIN32)
#include <windows.h>
#include <io.h>
#include <fcntl.h>
#else
#include <unistd.h>
#include <sched.h>
#include <pthread.h>
#include <arpa/inet.h>
#include <netdb.h>
#include <netinet/in.h>
#include <sys/socket.h>
#endif

/* ── Forward declarations for string helpers (string.c) ─────────────── */

extern const char *flux_string_data(int64_t s);
extern uint32_t    flux_string_len(int64_t s);
extern int64_t     flux_string_new(const char *data, uint32_t len);
extern int64_t     flux_string_concat(int64_t a, int64_t b);

int64_t flux_type_of(int64_t val);

/* Default constructor-name hook for native builds.
 * Programs with user ADTs provide a strong definition from generated LLVM.
 * Programs without user ADTs still need one linkable definition on Mach-O. */
const char *flux_user_ctor_name(int32_t ctor_tag) __attribute__((weak));
const char *flux_user_ctor_name(int32_t ctor_tag) {
    (void)ctor_tag;
    return NULL;
}

/* ── Runtime lifecycle ──────────────────────────────────────────────── */

void flux_rt_init(void) {
#if defined(_MSC_VER) || defined(_WIN32)
    _setmode(_fileno(stdout), _O_BINARY);
#endif
    flux_gc_init(0);
}

void flux_rt_shutdown(void) {
    /* Skip GC shutdown — the OS reclaims all memory at process exit.
     * Walking millions of malloc'd objects is slower than just exiting. */
    fflush(stdout);
}

/* ── Printing ───────────────────────────────────────────────────────── */

/*
 * Print a pointer-tagged value to stdout (no trailing newline).
 * Dispatches on the pointer tag / sentinel to determine the type.
 */
static const char *flux_lookup_user_ctor_name(int32_t ctor_tag) {
    return flux_user_ctor_name(ctor_tag);
}

/* Internal: print a value without trailing newline. */
static void flux_print_value(int64_t val) {
    /* Integer: LSB = 1 */
    if (flux_is_int(val)) {
        printf("%lld", (long long)flux_untag_int(val));
        return;
    }

    /* Sentinel values (even, < FLUX_MIN_PTR) */
    if (val == FLUX_NONE) {
        printf("None");
        return;
    }
    if (val == FLUX_FALSE) {
        printf("false");
        return;
    }
    if (val == FLUX_TRUE) {
        printf("true");
        return;
    }
    if (val == FLUX_EMPTY_LIST) {
        printf("[]");
        return;
    }
    if (val == FLUX_UNINIT) {
        printf("<uninit>");
        return;
    }
    if (val == FLUX_YIELD_SENTINEL) {
        printf("<yield>");
        return;
    }

    /* Heap pointer */
    if (flux_is_ptr(val)) {
        void *ptr = flux_untag_ptr(val);

        uint8_t obj = flux_obj_tag(ptr);
        if (obj == FLUX_OBJ_FLOAT) {
            double d = flux_unbox_float(val);
            printf("%.15g", d);
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
        } else if (obj == FLUX_OBJ_THUNK) {
            /* Thunks should never escape to user code; print for debugging. */
            printf("<thunk>");
        } else if (obj == FLUX_OBJ_TASK) {
            printf("<task>");
        } else if (obj == FLUX_OBJ_SCOPE) {
            printf("<scope>");
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
            default: {
                const char *ctor_name = flux_lookup_user_ctor_name(ctor_tag);
                if (field_count == 0) {
                    if (ctor_name) {
                        printf("%s", ctor_name);
                    } else {
                        printf("<adt tag=%d>", ctor_tag);
                    }
                } else {
                    if (ctor_name) {
                        printf("%s(", ctor_name);
                    } else {
                        printf("<adt tag=%d>(", ctor_tag);
                    }
                    for (int32_t i = 0; i < field_count; i++) {
                        if (i > 0) printf(", ");
                        flux_print_value(fields[i]);
                    }
                    printf(")");
                }
                break;
            }
            }
        } else if (flux_is_hamt(ptr)) {
            /* HAMT (hash map) */
            int64_t s = flux_hamt_format(val);
            fwrite(flux_string_data(s), 1, flux_string_len(s), stdout);
        } else {
            printf("<unknown obj=0x%02x>", obj);
        }
        return;
    }

    printf("<unknown val=%lld>", (long long)val);
}

void flux_print(int64_t val) {
    flux_print_value(val);
    putchar('\n');
    fflush(stdout);
}

void flux_println(int64_t val) {
    flux_print(val);
}

/* Debug-trace output. Routed to stderr so it does not corrupt stdout
 * pipelines — matches GHC Debug.Trace, Rust dbg!, and Python logging.
 *
 * The Flow.Debug wrappers always pass a pre-formatted String (produced by
 * `show()` / `to_string()` / a caller-supplied formatter), so the fast
 * path just writes the string's bytes directly to stderr. Non-string
 * fallback is provided for direct tests and primop-level callers. */
void flux_debug_trace(int64_t val) {
    if (flux_is_ptr(val) && flux_obj_tag(flux_untag_ptr(val)) == FLUX_OBJ_STRING) {
        fwrite(flux_string_data(val), 1, flux_string_len(val), stderr);
        fputc('\n', stderr);
    } else {
        /* Defensive fallback: temporarily redirect value-print to stderr
         * by flushing stdout, printing, and flushing stderr. Not used by
         * the Flow.Debug wrappers, but keeps primop-level calls honest. */
        fflush(stdout);
        fprintf(stderr, "<debug non-string value=%lld>\n", (long long)val);
    }
    fflush(stderr);
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
    char        err_buf[256];

    /* Build a VM-style error message and abort through the runtime panic path. */
    #define FLUX_READ_FILE_PANIC(fmt, ...)                                           \
        do {                                                                         \
            int written = snprintf(err_buf, sizeof(err_buf), fmt, __VA_ARGS__);      \
            if (written < 0) {                                                       \
                flux_panic(flux_string_new("read_file failed", 16));                 \
            }                                                                        \
            size_t msg_len = (size_t)written;                                        \
            if (msg_len >= sizeof(err_buf)) msg_len = sizeof(err_buf) - 1;           \
            flux_panic(flux_string_new(err_buf, (uint32_t)msg_len));                 \
        } while (0)

    /* Null-terminate the path (it may not be). */
    char *cpath = (char *)malloc(path_len + 1);
    if (!cpath) {
        FLUX_READ_FILE_PANIC("read_file failed for '%.*s': out of memory", (int)path_len, path_str);
    }
    memcpy(cpath, path_str, path_len);
    cpath[path_len] = '\0';

    FILE *f = fopen(cpath, "rb");
    if (!f) {
        int saved_errno = errno;
        FLUX_READ_FILE_PANIC(
            "read_file failed for '%s': %s (os error %d)",
            cpath,
            strerror(saved_errno),
            saved_errno
        );
    }

    fseek(f, 0, SEEK_END);
    long fsize = ftell(f);
    fseek(f, 0, SEEK_SET);

    if (fsize < 0) {
        int saved_errno = errno;
        fclose(f);
        FLUX_READ_FILE_PANIC(
            "read_file failed for '%s': %s (os error %d)",
            cpath,
            strerror(saved_errno),
            saved_errno
        );
    }

    char *contents = (char *)malloc((size_t)fsize);
    if (!contents) {
        fclose(f);
        FLUX_READ_FILE_PANIC("read_file failed for '%s': out of memory", cpath);
    }

    size_t nread = fread(contents, 1, (size_t)fsize, f);
    int read_error = ferror(f);
    int saved_errno = errno;
    fclose(f);
    if (nread != (size_t)fsize) {
        free(contents);
        FLUX_READ_FILE_PANIC(
            "read_file failed for '%s': %s (os error %d)",
            cpath,
            read_error ? strerror(saved_errno) : "short read",
            read_error ? saved_errno : 0
        );
    }

    int64_t result = flux_string_new(contents, (uint32_t)nread);
    free(cpath);
    free(contents);
    return result;

    #undef FLUX_READ_FILE_PANIC
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

int64_t flux_abs(int64_t n) {
    if (flux_val_is_float(n)) {
        double d = flux_unbox_float(n);
        return flux_box_float(d < 0 ? -d : d);
    }
    int64_t raw = flux_untag_int(n);
    return flux_tag_int(raw < 0 ? -raw : raw);
}

int64_t flux_min(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        double da = flux_unbox_float(a);
        double db = flux_unbox_float(b);
        return flux_box_float(da < db ? da : db);
    }
    int64_t ra = flux_untag_int(a);
    int64_t rb = flux_untag_int(b);
    return flux_tag_int(ra < rb ? ra : rb);
}

int64_t flux_max(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        double da = flux_unbox_float(a);
        double db = flux_unbox_float(b);
        return flux_box_float(da > db ? da : db);
    }
    int64_t ra = flux_untag_int(a);
    int64_t rb = flux_untag_int(b);
    return flux_tag_int(ra > rb ? ra : rb);
}

int64_t flux_sqrt(int64_t n) {
    return flux_box_float(sqrt(flux_unbox_float(n)));
}

int64_t flux_sin(int64_t n) {
    return flux_box_float(sin(flux_unbox_float(n)));
}

int64_t flux_cos(int64_t n) {
    return flux_box_float(cos(flux_unbox_float(n)));
}

int64_t flux_exp(int64_t n) {
    return flux_box_float(exp(flux_unbox_float(n)));
}

int64_t flux_log(int64_t n) {
    return flux_box_float(log(flux_unbox_float(n)));
}

int64_t flux_floor(int64_t n) {
    return flux_box_float(floor(flux_unbox_float(n)));
}

int64_t flux_ceil(int64_t n) {
    return flux_box_float(ceil(flux_unbox_float(n)));
}

int64_t flux_round(int64_t n) {
    return flux_box_float(round(flux_unbox_float(n)));
}

int64_t flux_tan(int64_t n) {
    return flux_box_float(tan(flux_unbox_float(n)));
}

int64_t flux_asin(int64_t n) {
    return flux_box_float(asin(flux_unbox_float(n)));
}

int64_t flux_acos(int64_t n) {
    return flux_box_float(acos(flux_unbox_float(n)));
}

int64_t flux_atan(int64_t n) {
    return flux_box_float(atan(flux_unbox_float(n)));
}

int64_t flux_sinh(int64_t n) {
    return flux_box_float(sinh(flux_unbox_float(n)));
}

int64_t flux_cosh(int64_t n) {
    return flux_box_float(cosh(flux_unbox_float(n)));
}

int64_t flux_tanh(int64_t n) {
    return flux_box_float(tanh(flux_unbox_float(n)));
}

int64_t flux_truncate(int64_t n) {
    return flux_box_float(trunc(flux_unbox_float(n)));
}

/* ── Runtime-dispatching arithmetic ─────────────────────────────────── */
/* These check the value type at runtime and dispatch to the correct op. */

static inline int flux_val_is_string(int64_t val) {
    if (!flux_is_ptr(val)) return 0;
    void *ptr = flux_untag_ptr(val);
    return flux_obj_tag(ptr) == FLUX_OBJ_STRING;
}

static int64_t flux_invalid_binary_op_error(const char *op, uint32_t op_len,
                                            int64_t a, int64_t b) {
    int64_t prefix = flux_string_new("Cannot ", 7);
    int64_t op_name = flux_string_new(op, op_len);
    int64_t middle = flux_string_new(" ", 1);
    int64_t and_sep = flux_string_new(" and ", 5);
    int64_t suffix = flux_string_new(" values.", 8);
    int64_t lhs = flux_type_of(a);
    int64_t rhs = flux_type_of(b);
    int64_t message =
        flux_string_concat(
            flux_string_concat(
                flux_string_concat(
                    flux_string_concat(
                        flux_string_concat(
                            flux_string_concat(prefix, op_name),
                            middle),
                        lhs),
                    and_sep),
                rhs),
            suffix);
    flux_panic(message);
    return FLUX_NONE;
}

static int64_t flux_invalid_unary_op_error(const char *op, uint32_t op_len,
                                           int64_t value) {
    int64_t prefix = flux_string_new("Cannot ", 7);
    int64_t op_name = flux_string_new(op, op_len);
    int64_t middle = flux_string_new(" ", 1);
    int64_t suffix = flux_string_new(" value.", 7);
    int64_t ty = flux_type_of(value);
    int64_t message =
        flux_string_concat(
            flux_string_concat(
                flux_string_concat(
                    flux_string_concat(prefix, op_name),
                    middle),
                ty),
            suffix);
    flux_panic(message);
    return FLUX_NONE;
}

int64_t flux_rt_add(int64_t a, int64_t b) {
    /* Check string first (boxed ptr with string tag), then float. */
    if (flux_val_is_string(a)) {
        if (!flux_val_is_string(b)) {
            return flux_invalid_binary_op_error("add", 3, a, b);
        }
        return flux_string_concat(a, b);
    }
    if (flux_val_is_float(a)) {
        if (!flux_val_is_float(b)) {
            return flux_invalid_binary_op_error("add", 3, a, b);
        }
        return flux_box_float(flux_unbox_float(a) + flux_unbox_float(b));
    }
    if (!flux_is_int(a) || !flux_is_int(b)) {
        return flux_invalid_binary_op_error("add", 3, a, b);
    }
    return flux_tag_int(flux_untag_int(a) + flux_untag_int(b));
}

int64_t flux_rt_sub(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        if (!flux_val_is_float(b)) {
            return flux_invalid_binary_op_error("subtract", 8, a, b);
        }
        return flux_box_float(flux_unbox_float(a) - flux_unbox_float(b));
    }
    if (!flux_is_int(a) || !flux_is_int(b)) {
        return flux_invalid_binary_op_error("subtract", 8, a, b);
    }
    return flux_tag_int(flux_untag_int(a) - flux_untag_int(b));
}

int64_t flux_rt_mul(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        if (!flux_val_is_float(b)) {
            return flux_invalid_binary_op_error("multiply", 8, a, b);
        }
        return flux_box_float(flux_unbox_float(a) * flux_unbox_float(b));
    }
    if (!flux_is_int(a) || !flux_is_int(b)) {
        return flux_invalid_binary_op_error("multiply", 8, a, b);
    }
    return flux_tag_int(flux_untag_int(a) * flux_untag_int(b));
}

int64_t flux_rt_div(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        if (!flux_val_is_float(b)) {
            return flux_invalid_binary_op_error("divide", 6, a, b);
        }
        return flux_box_float(flux_unbox_float(a) / flux_unbox_float(b));
    }
    if (!flux_is_int(a) || !flux_is_int(b)) {
        return flux_invalid_binary_op_error("divide", 6, a, b);
    }
    int64_t rb = flux_untag_int(b);
    if (rb == 0) {
        flux_panic(flux_string_new("Division by zero", 16));
        return flux_tag_int(0); /* unreachable */
    }
    return flux_tag_int(flux_untag_int(a) / rb);
}

static void flux_panic_division_by_zero_at(int64_t line, int64_t column) {
    char buf[64];
    int len = snprintf(buf, sizeof(buf), "Division by zero @%lld:%lld",
                       (long long)line, (long long)column);
    if (len <= 0) {
        flux_panic(flux_string_new("Division by zero", 16));
        return;
    }
    flux_panic(flux_string_new(buf, (uint32_t)len));
}

int64_t flux_rt_div_loc(int64_t a, int64_t b, int64_t line, int64_t column) {
    if (flux_val_is_float(a)) {
        if (!flux_val_is_float(b)) {
            return flux_invalid_binary_op_error("divide", 6, a, b);
        }
        return flux_box_float(flux_unbox_float(a) / flux_unbox_float(b));
    }
    if (!flux_is_int(a) || !flux_is_int(b)) {
        return flux_invalid_binary_op_error("divide", 6, a, b);
    }
    int64_t rb = flux_untag_int(b);
    if (rb == 0) {
        flux_panic_division_by_zero_at(line, column);
        return flux_tag_int(0); /* unreachable */
    }
    return flux_tag_int(flux_untag_int(a) / rb);
}

int64_t flux_rt_mod(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        if (!flux_val_is_float(b)) {
            return flux_invalid_binary_op_error("modulo", 6, a, b);
        }
        return flux_box_float(fmod(flux_unbox_float(a), flux_unbox_float(b)));
    }
    if (!flux_is_int(a) || !flux_is_int(b)) {
        return flux_invalid_binary_op_error("modulo", 6, a, b);
    }
    int64_t rb = flux_untag_int(b);
    if (rb == 0) {
        flux_panic(flux_string_new("Division by zero", 16));
        return flux_tag_int(0); /* unreachable */
    }
    return flux_tag_int(flux_untag_int(a) % rb);
}

int64_t flux_rt_mod_loc(int64_t a, int64_t b, int64_t line, int64_t column) {
    if (flux_val_is_float(a)) {
        if (!flux_val_is_float(b)) {
            return flux_invalid_binary_op_error("modulo", 6, a, b);
        }
        return flux_box_float(fmod(flux_unbox_float(a), flux_unbox_float(b)));
    }
    if (!flux_is_int(a) || !flux_is_int(b)) {
        return flux_invalid_binary_op_error("modulo", 6, a, b);
    }
    int64_t rb = flux_untag_int(b);
    if (rb == 0) {
        flux_panic_division_by_zero_at(line, column);
        return flux_tag_int(0); /* unreachable */
    }
    return flux_tag_int(flux_untag_int(a) % rb);
}

int64_t flux_rt_neg(int64_t a) {
    if (flux_val_is_float(a)) {
        return flux_box_float(-flux_unbox_float(a));
    }
    if (!flux_is_int(a)) {
        return flux_invalid_unary_op_error("negate", 6, a);
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

/* ── Safe arithmetic (Proposal 0135) ───────────────────────────────── */

int64_t flux_safe_div(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        double fb = flux_unbox_float(b);
        if (fb == 0.0) return flux_make_none();
        return flux_wrap_some(flux_box_float(flux_unbox_float(a) / fb));
    }
    int64_t rb = flux_untag_int(b);
    if (rb == 0) return flux_make_none();
    return flux_wrap_some(flux_tag_int(flux_untag_int(a) / rb));
}

int64_t flux_safe_mod(int64_t a, int64_t b) {
    if (flux_val_is_float(a)) {
        double fb = flux_unbox_float(b);
        if (fb == 0.0) return flux_make_none();
        return flux_wrap_some(flux_box_float(fmod(flux_unbox_float(a), fb)));
    }
    int64_t rb = flux_untag_int(b);
    if (rb == 0) return flux_make_none();
    return flux_wrap_some(flux_tag_int(flux_untag_int(a) % rb));
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
    if (result == FLUX_NONE) {
        return flux_make_none();
    }
    return flux_wrap_some(result);
}

/* ── Runtime-dispatching comparisons ────────────────────────────────── */

int64_t flux_rt_eq(int64_t a, int64_t b) {
    /* Fast path: bitwise equal (same int, bool, None, or same pointer). */
    if (a == b) return flux_make_bool(1);
    /* Heap pointer structural equality. */
    if (flux_is_ptr(a) && flux_is_ptr(b)) {
        void *pa = flux_untag_ptr(a);
        void *pb = flux_untag_ptr(b);
        uint8_t tag_a = flux_obj_tag(pa);
        uint8_t tag_b = flux_obj_tag(pb);
        if (tag_a != tag_b) return flux_make_bool(0);
        /* Float equality. */
        if (tag_a == FLUX_OBJ_FLOAT) {
            return flux_make_bool(flux_unbox_float(a) == flux_unbox_float(b));
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
                if (eq == FLUX_FALSE) return flux_make_bool(0);
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
                if (eq == FLUX_FALSE) return flux_make_bool(0);
            }
            return flux_make_bool(1);
        }
    }
    return flux_make_bool(0);
}

int64_t flux_rt_neq(int64_t a, int64_t b) {
    int64_t eq = flux_rt_eq(a, b);
    return (eq == flux_make_bool(1)) ? flux_make_bool(0) : flux_make_bool(1);
}

int64_t flux_rt_lt(int64_t a, int64_t b) {
    if (flux_val_is_float(a))
        return flux_make_bool(flux_unbox_float(a) < flux_unbox_float(b));
    return flux_make_bool(flux_untag_int(a) < flux_untag_int(b));
}

int64_t flux_rt_le(int64_t a, int64_t b) {
    if (flux_val_is_float(a))
        return flux_make_bool(flux_unbox_float(a) <= flux_unbox_float(b));
    return flux_make_bool(flux_untag_int(a) <= flux_untag_int(b));
}

int64_t flux_rt_gt(int64_t a, int64_t b) {
    if (flux_val_is_float(a))
        return flux_make_bool(flux_unbox_float(a) > flux_unbox_float(b));
    return flux_make_bool(flux_untag_int(a) > flux_untag_int(b));
}

int64_t flux_rt_ge(int64_t a, int64_t b) {
    if (flux_val_is_float(a))
        return flux_make_bool(flux_unbox_float(a) >= flux_unbox_float(b));
    return flux_make_bool(flux_untag_int(a) >= flux_untag_int(b));
}

/* ── Runtime-dispatching index ──────────────────────────────────────── */

static int64_t flux_index_unsupported(int64_t collection) {
    int64_t prefix = flux_string_new("index operator not supported: ", 30);
    int64_t ty = flux_type_of(collection);
    flux_panic(flux_string_concat(prefix, ty));
    return FLUX_NONE;
}

int64_t flux_rt_index(int64_t collection, int64_t key) {
    if (!flux_is_ptr(collection)) {
        return flux_index_unsupported(collection);
    }
    void *ptr = flux_untag_ptr(collection);
    uint8_t tag = flux_obj_tag(ptr);
    switch (tag) {
    case FLUX_OBJ_ARRAY: {
        int64_t result = flux_array_get(collection, key);
        if (result == FLUX_NONE) {
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
    case FLUX_OBJ_ADT: {
        if (flux_is_hamt(ptr)) {
            return flux_hamt_get_option(collection, key);
        }
        /* Cons list: ctor_tag=4, traverse to nth element. */
        int32_t ctor_tag = *(int32_t *)ptr;
        if (ctor_tag == 4) {
            int64_t idx = flux_untag_int(key);
            if (idx < 0) return flux_make_none();
            int64_t cur = collection;
            while (flux_is_ptr(cur)) {
                void *cp = flux_untag_ptr(cur);
                if (*(int32_t *)cp != 4) break;
                if (idx == 0) {
                    int64_t *fields = (int64_t *)((char *)cp + 8);
                    return flux_wrap_some(fields[0]);
                }
                idx--;
                int64_t *fields = (int64_t *)((char *)cp + 8);
                cur = fields[1];
            }
            return flux_make_none();
        }
        return flux_index_unsupported(collection);
    }
    default: {
        if (flux_is_hamt(ptr)) {
            return flux_hamt_get_option(collection, key);
        }
        return flux_index_unsupported(collection);
    }
    }
}

/* ── Type inspection ────────────────────────────────────────────────── */

int64_t flux_type_of(int64_t val) {
    if (flux_is_int(val))          return flux_string_new("Int", 3);
    if (val == FLUX_NONE)          return flux_string_new("None", 4);
    if (val == FLUX_FALSE || val == FLUX_TRUE) return flux_string_new("Bool", 4);
    if (val == FLUX_EMPTY_LIST)    return flux_string_new("List", 4);

    if (flux_is_ptr(val)) {
        void *ptr = flux_untag_ptr(val);
        uint8_t obj = flux_obj_tag(ptr);
        switch (obj) {
        case FLUX_OBJ_FLOAT:   return flux_string_new("Float", 5);
        case FLUX_OBJ_STRING:  return flux_string_new("String", 6);
        case FLUX_OBJ_ARRAY:   return flux_string_new("Array", 5);
        case FLUX_OBJ_TUPLE:   return flux_string_new("Tuple", 5);
        case FLUX_OBJ_CLOSURE: return flux_string_new("Function", 8);
        case FLUX_OBJ_ADT: {
            /* Check HAMT first — HAMT kind values collide with ADT ctor_tags. */
            if (flux_is_hamt(ptr)) {
                return flux_string_new("Map", 3);
            }
            int32_t first_i32 = *(int32_t *)ptr;
            int32_t second_i32 = *((int32_t *)ptr + 1);
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
            return flux_string_new("Adt", 3);
        }
        default: {
            if (flux_is_hamt(ptr)) {
                return flux_string_new("Map", 3);
            }
            return flux_string_new("Object", 6);
        }
        }
    }

    return flux_string_new("Unknown", 7);
}

/* Runtime-callable type predicates are defined at end of file
 * (after #undef of the static inline helpers from flux_rt.h). */

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

/* Try/catch infrastructure: flux_try uses setjmp to catch flux_panic. */
static __thread jmp_buf *flux_try_jmp = NULL;
static __thread int64_t  flux_try_error_msg = 0; /* FLUX_NONE */

void flux_panic(int64_t msg) {
    /* If a try handler is active, longjmp instead of aborting. */
    if (flux_try_jmp) {
        flux_try_error_msg = msg;
        longjmp(*flux_try_jmp, 1);
    }
    if (flux_is_ptr(msg) && flux_obj_tag(flux_untag_ptr(msg)) == FLUX_OBJ_STRING) {
        uint32_t len = flux_string_len(msg);
        const char *data = flux_string_data(msg);
        fprintf(stderr, "panic: %.*s\n", (int)len, data);
    } else {
        fprintf(stderr, "panic: ");
        flux_print(msg);
        fprintf(stderr, "\n");
    }
    flux_trace_print();
    exit(1);
}

/* Allocate a tuple: { u8 obj_tag=0xF3, u8[3] pad, u32 arity, i64 elements[] }. */
static int64_t make_tuple(int64_t *fields, uint32_t arity) {
    uint32_t payload = 4 + 4 + arity * 8; /* pad+arity header + elements */
    void *ptr = flux_gc_alloc_header(payload, (uint8_t)arity, FLUX_OBJ_TUPLE);
    *(uint32_t *)((char *)ptr + 4) = arity;
    int64_t *elems = (int64_t *)((char *)ptr + 8);
    for (uint32_t i = 0; i < arity; i++) {
        elems[i] = fields[i];
    }
    return (int64_t)ptr;
}

/* flux_try(thunk): call thunk(), return ("ok", result) or ("error", message). */
int64_t flux_try(int64_t thunk) {
    jmp_buf buf;
    jmp_buf *prev = flux_try_jmp;
    flux_try_jmp = &buf;
    flux_try_error_msg = 0;

    int64_t result;
    if (setjmp(buf) == 0) {
        /* Normal path: call the thunk with 0 args. */
        result = flux_call_closure_c(thunk, NULL, 0);
        flux_try_jmp = prev;
        /* Return ("ok", result) as a 2-tuple. */
        int64_t ok_str = flux_string_new("ok", 2);
        int64_t fields[2];
        fields[0] = ok_str;
        fields[1] = result;
        return make_tuple(fields, 2);
    } else {
        /* Error path: flux_panic called longjmp. */
        flux_try_jmp = prev;
        int64_t err_str = flux_string_new("error", 5);
        int64_t msg = flux_try_error_msg;
        if (msg == 0) {
            msg = flux_string_new("unknown error", 13);
        } else if (!flux_is_ptr(msg)) {
            /* Convert non-string panic value to string. */
            msg = flux_string_new("runtime error", 13);
        }
        int64_t fields[2];
        fields[0] = err_str;
        fields[1] = msg;
        return make_tuple(fields, 2);
    }
}

typedef struct {
    void    *fn_ptr;
    int32_t  remaining_arity;
    int32_t  capture_count;
    int32_t  applied_count;
    int32_t  _pad;
    int64_t  payload[];
} FluxClosure;

static int64_t flux_call_not_function_error(int64_t value) {
    int64_t prefix = flux_string_new("Cannot call non-function value (got ", 36);
    int64_t ty = flux_type_of(value);
    int64_t suffix = flux_string_new(").", 2);
    flux_panic(flux_string_concat(flux_string_concat(prefix, ty), suffix));
    return FLUX_NONE;
}

static int64_t flux_wrong_arity_error(int32_t expected, int32_t got) {
    char buf[96];
    int len = snprintf(buf, sizeof(buf),
                       "wrong number of arguments: want=%d, got=%d",
                       expected, got);
    if (len < 0) {
        flux_panic(flux_string_new("wrong number of arguments", 25));
        return FLUX_NONE;
    }
    flux_panic(flux_string_new(buf, (uint32_t)len));
    return FLUX_NONE;
}

int64_t flux_call_closure_exact(int64_t closure, int64_t *args, int32_t nargs) {
    if (!flux_is_ptr(closure)) {
        return flux_call_not_function_error(closure);
    }

    void *ptr = flux_untag_ptr(closure);
    if (flux_obj_tag(ptr) != FLUX_OBJ_CLOSURE) {
        return flux_call_not_function_error(closure);
    }

    FluxClosure *fn = (FluxClosure *)ptr;
    if (fn->remaining_arity != nargs) {
        return flux_wrong_arity_error(fn->remaining_arity, nargs);
    }

    return flux_call_closure_c(closure, args, nargs);
}

/* flux_assert_throws(thunk, expected_msg): assert that thunk() panics. */
int64_t flux_assert_throws(int64_t thunk, int64_t expected_msg) {
    jmp_buf buf;
    jmp_buf *prev = flux_try_jmp;
    flux_try_jmp = &buf;
    flux_try_error_msg = 0;

    if (setjmp(buf) == 0) {
        /* Thunk completed without error — assertion failure. */
        flux_call_closure_c(thunk, NULL, 0);
        flux_try_jmp = prev;
        flux_panic(flux_string_new("assert_throws failed: function completed without error", 54));
        return 0; /* unreachable */
    } else {
        /* Thunk panicked — check message if expected_msg is provided. */
        flux_try_jmp = prev;
        if (expected_msg != 0 && flux_is_ptr(expected_msg)) {
            int64_t actual = flux_try_error_msg;
            if (actual != 0 && flux_is_ptr(actual)) {
                const char *exp_data = flux_string_data(expected_msg);
                uint32_t exp_len = flux_string_len(expected_msg);
                const char *act_data = flux_string_data(actual);
                uint32_t act_len = flux_string_len(actual);
                /* Check if actual contains expected. */
                int found = 0;
                if (act_len >= exp_len) {
                    for (uint32_t i = 0; i <= act_len - exp_len; i++) {
                        if (memcmp(act_data + i, exp_data, exp_len) == 0) {
                            found = 1;
                            break;
                        }
                    }
                }
                if (!found) {
                    flux_panic(flux_string_new("assert_throws: error message mismatch", 37));
                }
            }
        }
        return 0; /* FLUX_NONE */
    }
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

int64_t flux_string_to_bytes(int64_t s) {
    return flux_string_new(flux_string_data(s), flux_string_len(s));
}

int64_t flux_bytes_to_string(int64_t b) {
    return flux_string_new(flux_string_data(b), flux_string_len(b));
}

int64_t flux_bytes_length(int64_t b) {
    return flux_string_length(b);
}

int64_t flux_bytes_slice(int64_t b, int64_t start, int64_t end) {
    return flux_string_slice(b, start, end);
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
    /* Integer */
    if (flux_is_int(val)) return flux_int_to_string(val);

    /* Sentinel values */
    if (val == FLUX_NONE)       return flux_string_new("None", 4);
    if (val == FLUX_TRUE)       return flux_string_new("true", 4);
    if (val == FLUX_FALSE)      return flux_string_new("false", 5);
    if (val == FLUX_EMPTY_LIST) return flux_string_new("[]", 2);

    /* Heap pointer */
    if (flux_is_ptr(val)) {
        void *ptr = flux_untag_ptr(val);
        uint8_t obj = flux_obj_tag(ptr);

        if (obj == FLUX_OBJ_FLOAT) {
            return flux_float_to_string(val);
        }
        if (obj == FLUX_OBJ_STRING) {
            /* Wrap in quotes to match VM's ToString behavior. */
            const char *sd = flux_string_data(val);
            uint32_t sl = flux_string_len(val);
            char tmp[4096];
            char *buf = (sl + 3 <= sizeof(tmp)) ? tmp : (char *)malloc(sl + 3);
            buf[0] = '"';
            memcpy(buf + 1, sd, sl);
            buf[sl + 1] = '"';
            int64_t result = flux_string_new(buf, sl + 2);
            if (buf != tmp) free(buf);
            return result;
        }
        if (obj == FLUX_OBJ_ARRAY) {
            /* Render array as "[|elem1, elem2, ...|]" */
            uint32_t len = *(uint32_t *)((char *)ptr + 4);
            int64_t *elems = (int64_t *)((char *)ptr + 16);
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
        if (obj == FLUX_OBJ_ADT) {
            int32_t ctor_tag = *(int32_t *)ptr;
            int32_t field_count = *((int32_t *)ptr + 1);
            const char *ctor_name = flux_lookup_user_ctor_name(ctor_tag);
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
            if (ctor_name) {
                int64_t *fields = (int64_t *)((char *)ptr + 8);
                char buf[4096];
                int pos = 0;
                pos += snprintf(buf + pos, sizeof(buf) - pos, "%s", ctor_name);
                if (field_count > 0) {
                    pos += snprintf(buf + pos, sizeof(buf) - pos, "(");
                    for (int32_t i = 0; i < field_count && pos < (int)sizeof(buf) - 20; i++) {
                        if (i > 0) pos += snprintf(buf + pos, sizeof(buf) - pos, ", ");
                        int64_t s = flux_to_string(fields[i]);
                        const char *sd = flux_string_data(s);
                        uint32_t sl = flux_string_len(s);
                        if (pos + sl < sizeof(buf) - 10) {
                            memcpy(buf + pos, sd, sl);
                            pos += sl;
                        }
                    }
                    pos += snprintf(buf + pos, sizeof(buf) - pos, ")");
                }
                return flux_string_new(buf, (uint32_t)pos);
            }
            /* Cons list: ctor_tag=4, field_count=2 */
            if (ctor_tag == 4 && field_count == 2) {
                char buf[4096];
                int pos = 0;
                pos += snprintf(buf + pos, sizeof(buf) - pos, "[");
                int64_t cur = val;
                int first_elem = 1;
                while (flux_is_ptr(cur)) {
                    void *cp = flux_untag_ptr(cur);
                    int32_t ct = *(int32_t *)cp;
                    int32_t fc = *((int32_t *)cp + 1);
                    if (ct != 4 || fc != 2) break;
                    int64_t *fields = (int64_t *)((char *)cp + 8);
                    if (!first_elem) pos += snprintf(buf + pos, sizeof(buf) - pos, ", ");
                    first_elem = 0;
                    int64_t s = flux_to_string(fields[0]);
                    const char *sd = flux_string_data(s);
                    uint32_t sl = flux_string_len(s);
                    if (pos + sl < sizeof(buf) - 10) {
                        memcpy(buf + pos, sd, sl);
                        pos += sl;
                    }
                    cur = fields[1];
                }
                pos += snprintf(buf + pos, sizeof(buf) - pos, "]");
                return flux_string_new(buf, (uint32_t)pos);
            }
        }
        if (flux_is_hamt(ptr)) {
            return flux_hamt_format(val);
        }
    }
    return flux_string_new("<value>", 7);
}

int64_t flux_to_string_value(int64_t val) {
    /* Integer */
    if (flux_is_int(val)) return flux_int_to_string(val);

    /* Sentinel values */
    if (val == FLUX_NONE)       return flux_string_new("None", 4);
    if (val == FLUX_TRUE)       return flux_string_new("true", 4);
    if (val == FLUX_FALSE)      return flux_string_new("false", 5);
    if (val == FLUX_EMPTY_LIST) return flux_string_new("[]", 2);

    /* Heap pointer */
    if (flux_is_ptr(val)) {
        void *ptr = flux_untag_ptr(val);
        uint8_t obj = flux_obj_tag(ptr);

        if (obj == FLUX_OBJ_FLOAT) {
            return flux_float_to_string(val);
        }
        if (obj == FLUX_OBJ_STRING) {
            return flux_string_new(flux_string_data(val), flux_string_len(val));
        }
        if (obj == FLUX_OBJ_ADT) {
            int32_t ctor_tag = *(int32_t *)ptr;
            int32_t field_count = *((int32_t *)ptr + 1);
            int64_t *fields = (int64_t *)((char *)ptr + 8);
            char buf[4096];
            int pos = 0;

            if (ctor_tag == 1 && field_count >= 1) {
                pos += snprintf(buf + pos, sizeof(buf) - pos, "Some(");
                int64_t s = flux_to_string_value(fields[0]);
                const char *sd = flux_string_data(s);
                uint32_t sl = flux_string_len(s);
                if (pos + sl < sizeof(buf) - 10) { memcpy(buf + pos, sd, sl); pos += sl; }
                pos += snprintf(buf + pos, sizeof(buf) - pos, ")");
                return flux_string_new(buf, (uint32_t)pos);
            }
            if (ctor_tag == 0 && field_count == 0) {
                return flux_string_new("None", 4);
            }
        }

        return flux_to_string(val);
    }
    return flux_string_new("<value>", 7);
}

int64_t flux_read_lines(int64_t path) {
    /* Read file, split on newlines, return as Array (matching VM semantics). */
    int64_t content = flux_read_file(path);
    if (content == FLUX_NONE) {
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
    /* Check heap pointer first. */
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
    /* Sentinel: empty list, integer, etc. */
    if (collection == FLUX_EMPTY_LIST)
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
    if (val == FLUX_EMPTY_LIST) return flux_make_bool(1);
    if (!flux_is_ptr(val)) return flux_make_bool(0);
    void *ptr = flux_untag_ptr(val);
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
    if (flux_get_array_elems(collection, &elems, &len)) {
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

    /* Cons list: collect, sort stably by computed keys, rebuild list. */
    int64_t count = 0;
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (*(int32_t *)cp != 4) break;
        count++;
        cur = ((int64_t *)((char *)cp + 8))[1];
    }
    if (count <= 1) return collection;

    int64_t *sorted = (int64_t *)malloc(count * sizeof(int64_t));
    int64_t *keys_arr = (int64_t *)malloc(count * sizeof(int64_t));
    cur = collection;
    for (int64_t i = 0; i < count; i++) {
        void *cp = flux_untag_ptr(cur);
        int64_t *fields = (int64_t *)((char *)cp + 8);
        sorted[i] = fields[0];
        keys_arr[i] = call1(func, sorted[i]);
        cur = fields[1];
    }

    for (int64_t i = 1; i < count; i++) {
        int64_t key = keys_arr[i];
        int64_t val = sorted[i];
        int64_t j = i - 1;
        while (j >= 0 && flux_rt_gt(keys_arr[j], key) == flux_make_bool(1)) {
            keys_arr[j + 1] = keys_arr[j];
            sorted[j + 1] = sorted[j];
            j--;
        }
        keys_arr[j + 1] = key;
        sorted[j + 1] = val;
    }

    int64_t result = flux_make_empty_list();
    for (int64_t i = count - 1; i >= 0; i--) {
        void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_ADT);
        *(int32_t *)mem = 4; /* CONS tag */
        *((int32_t *)mem + 1) = 2;
        int64_t *fields = (int64_t *)((char *)mem + 8);
        fields[0] = sorted[i];
        fields[1] = result;
        result = flux_tag_ptr(mem);
    }
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
    if (flux_get_array_elems(collection, &elems, &len)) {
        if (len <= 1) return collection;

        int64_t *sorted = (int64_t *)malloc(len * sizeof(int64_t));
        memcpy(sorted, elems, len * sizeof(int64_t));

        /* Insertion sort by runtime comparison. */
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

    int64_t count = 0;
    int64_t cur = collection;
    while (flux_is_ptr(cur)) {
        void *cp = flux_untag_ptr(cur);
        if (*(int32_t *)cp != 4) break;
        count++;
        cur = ((int64_t *)((char *)cp + 8))[1];
    }
    if (count <= 1) return collection;

    int64_t *sorted = (int64_t *)malloc(count * sizeof(int64_t));
    cur = collection;
    for (int64_t i = 0; i < count; i++) {
        void *cp = flux_untag_ptr(cur);
        int64_t *fields = (int64_t *)((char *)cp + 8);
        sorted[i] = fields[0];
        cur = fields[1];
    }

    for (int64_t i = 1; i < count; i++) {
        int64_t key = sorted[i];
        int64_t j = i - 1;
        while (j >= 0) {
            int64_t cmp = flux_rt_gt(sorted[j], key);
            if (cmp != flux_make_bool(1)) break;
            sorted[j + 1] = sorted[j];
            j--;
        }
        sorted[j + 1] = key;
    }

    int64_t result = flux_make_empty_list();
    for (int64_t i = count - 1; i >= 0; i--) {
        void *mem = flux_gc_alloc_header(8 + 2 * 8, 2, FLUX_OBJ_ADT);
        *(int32_t *)mem = 4; /* CONS tag */
        *((int32_t *)mem + 1) = 2;
        int64_t *fields = (int64_t *)((char *)mem + 8);
        fields[0] = sorted[i];
        fields[1] = result;
        result = flux_tag_ptr(mem);
    }
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

/* ── Phase 1a native Task ABI ──────────────────────────────────────── */

typedef struct {
    pthread_mutex_t mutex;
    pthread_cond_t cond;
    pthread_t thread;
    int started;
    int completed;
    int joined;
    int cancelled;
    int detached;
    int failed;
    int64_t action;
    int64_t result;
    int64_t error;
} FluxNativeTask;

typedef struct {
    pthread_mutex_t mutex;
    uint32_t count;
    uint32_t cap;
    FluxNativeTask **tasks;
} FluxNativeScope;

static __thread FluxNativeTask *flux_current_native_task = NULL;

static int flux_current_task_cancelled(void) {
    FluxNativeTask *task = flux_current_native_task;
    if (!task) return 0;
    pthread_mutex_lock(&task->mutex);
    int cancelled = task->cancelled;
    pthread_mutex_unlock(&task->mutex);
    return cancelled;
}

static int flux_is_native_task(int64_t task) {
    return flux_is_ptr(task) && flux_obj_tag(flux_untag_ptr(task)) == FLUX_OBJ_TASK;
}

static FluxNativeTask *flux_native_task_unbox(int64_t task, const char *op) {
    if (!flux_is_native_task(task)) {
        char buf[96];
        int len = snprintf(buf, sizeof(buf), "%s expects Task", op);
        if (len < 0) {
            flux_panic(flux_string_new("Task operation expects Task", 27));
        }
        if (len >= (int)sizeof(buf)) {
            len = (int)sizeof(buf) - 1;
        }
        flux_panic(flux_string_new(buf, (uint32_t)len));
        return NULL;
    }
    return (FluxNativeTask *)flux_untag_ptr(task);
}

static void *flux_native_task_main(void *arg) {
    FluxNativeTask *task = (FluxNativeTask *)arg;

    pthread_mutex_lock(&task->mutex);
    task->started = 1;
    int cancelled = task->cancelled;
    int64_t action = task->action;
    pthread_mutex_unlock(&task->mutex);

    int failed = 0;
    int64_t result = FLUX_NONE;
    int64_t error = FLUX_NONE;
    if (!cancelled) {
        jmp_buf buf;
        jmp_buf *prev = flux_try_jmp;
        int64_t prev_msg = flux_try_error_msg;
        FluxNativeTask *prev_task = flux_current_native_task;
        flux_try_jmp = &buf;
        flux_try_error_msg = FLUX_NONE;
        flux_current_native_task = task;
        if (setjmp(buf) == 0) {
            result = flux_call_closure_c(action, NULL, 0);
        } else {
            failed = 1;
            error = flux_try_error_msg;
        }
        flux_current_native_task = prev_task;
        flux_try_jmp = prev;
        flux_try_error_msg = prev_msg;
    }

    pthread_mutex_lock(&task->mutex);
    task->result = result;
    task->failed = failed;
    task->error = error;
    task->completed = 1;
    int detached = task->detached;
    pthread_cond_broadcast(&task->cond);
    pthread_mutex_unlock(&task->mutex);
    if (detached && action != FLUX_NONE) {
        flux_drop(action);
        pthread_mutex_lock(&task->mutex);
        task->action = FLUX_NONE;
        pthread_mutex_unlock(&task->mutex);
    }
    return NULL;
}

int64_t flux_task_spawn(int64_t action) {
    if (!flux_is_ptr(action) || flux_obj_tag(flux_untag_ptr(action)) != FLUX_OBJ_CLOSURE) {
        flux_panic(flux_string_new("Task.spawn expects a closure", 28));
        return FLUX_NONE;
    }

    FluxNativeTask *task = (FluxNativeTask *)flux_gc_alloc_header(
        sizeof(FluxNativeTask), 0, FLUX_OBJ_TASK);
    memset(task, 0, sizeof(*task));
    task->action = action;
    task->result = FLUX_NONE;
    task->error = FLUX_NONE;

    flux_dup(action);
    flux_rc_share_deep(action);

    if (pthread_mutex_init(&task->mutex, NULL) != 0 ||
        pthread_cond_init(&task->cond, NULL) != 0) {
        flux_drop(action);
        flux_panic(flux_string_new("Task.spawn failed to initialize task", 36));
        return FLUX_NONE;
    }

    int rc = pthread_create(&task->thread, NULL, flux_native_task_main, task);
    if (rc != 0) {
        pthread_cond_destroy(&task->cond);
        pthread_mutex_destroy(&task->mutex);
        flux_drop(action);
        flux_panic(flux_string_new("Task.spawn failed to create thread", 34));
        return FLUX_NONE;
    }

    return flux_tag_ptr(task);
}

int64_t flux_task_blocking_join(int64_t task_value) {
    FluxNativeTask *task = flux_native_task_unbox(task_value, "Task.blocking_join");
    if (!task) return FLUX_NONE;

    pthread_mutex_lock(&task->mutex);
    while (!task->completed) {
        pthread_cond_wait(&task->cond, &task->mutex);
    }
    int joined = task->joined;
    int cancelled = task->cancelled;
    int failed = task->failed;
    int64_t result = task->result;
    int64_t error = task->error;
    task->joined = 1;
    pthread_mutex_unlock(&task->mutex);

    if (joined) {
        flux_panic(flux_string_new("Task.blocking_join called more than once", 39));
        return FLUX_NONE;
    }

    pthread_join(task->thread, NULL);
    flux_drop(task->action);
    task->action = FLUX_NONE;

    if (cancelled && result == FLUX_NONE) {
        flux_panic(flux_string_new("Task was cancelled", 18));
        return FLUX_NONE;
    }
    if (failed) {
        flux_panic(error);
        return FLUX_NONE;
    }

    return result;
}

int64_t flux_task_cancel(int64_t task_value) {
    FluxNativeTask *task = flux_native_task_unbox(task_value, "Task.cancel");
    if (!task) return FLUX_NONE;

    pthread_mutex_lock(&task->mutex);
    task->cancelled = 1;
    pthread_mutex_unlock(&task->mutex);
    return FLUX_NONE;
}

static FluxNativeTask *flux_native_task_spawn_raw(int64_t action, const char *op) {
    int64_t task_value = flux_task_spawn(action);
    FluxNativeTask *task = flux_native_task_unbox(task_value, op);
    flux_rc_share_deep(task_value);
    return task;
}

static int flux_native_task_is_completed(FluxNativeTask *task) {
    pthread_mutex_lock(&task->mutex);
    int completed = task->completed;
    pthread_mutex_unlock(&task->mutex);
    return completed;
}

static void flux_native_task_cancel_raw(FluxNativeTask *task) {
    pthread_mutex_lock(&task->mutex);
    task->cancelled = 1;
    pthread_mutex_unlock(&task->mutex);
}

static void flux_native_task_detach_cancel_raw(FluxNativeTask *task) {
    pthread_mutex_lock(&task->mutex);
    task->cancelled = 1;
    task->detached = 1;
    task->joined = 1;
    int completed = task->completed;
    int64_t action = task->action;
    task->action = FLUX_NONE;
    pthread_mutex_unlock(&task->mutex);

    pthread_detach(task->thread);
    if (completed && action != FLUX_NONE) {
        flux_drop(action);
    }
}

static int64_t flux_native_task_join_raw(FluxNativeTask *task, int rethrow) {
    pthread_mutex_lock(&task->mutex);
    while (!task->completed) {
        pthread_cond_wait(&task->cond, &task->mutex);
    }
    int joined = task->joined;
    int cancelled = task->cancelled;
    int failed = task->failed;
    int64_t result = task->result;
    int64_t error = task->error;
    task->joined = 1;
    pthread_mutex_unlock(&task->mutex);

    if (joined) {
        if (rethrow) {
            flux_panic(flux_string_new("Task joined more than once", 26));
        }
        return FLUX_NONE;
    }

    pthread_join(task->thread, NULL);
    flux_drop(task->action);
    task->action = FLUX_NONE;

    if (cancelled && result == FLUX_NONE) {
        if (rethrow) {
            flux_panic(flux_string_new("Task was cancelled", 18));
        }
        return FLUX_NONE;
    }
    if (failed) {
        if (rethrow) {
            flux_panic(error);
        }
        return error;
    }
    return result;
}

static int flux_native_task_failed(FluxNativeTask *task) {
    pthread_mutex_lock(&task->mutex);
    int failed = task->failed;
    pthread_mutex_unlock(&task->mutex);
    return failed;
}

int64_t flux_async_sleep(int64_t ms) {
    if (!flux_is_int(ms)) {
        flux_panic(flux_string_new("Async.sleep expects Int milliseconds", 36));
        return FLUX_NONE;
    }
    int64_t raw_ms = flux_untag_int(ms);
    if (raw_ms <= 0) {
        return FLUX_NONE;
    }
    while (raw_ms > 0) {
        if (flux_current_task_cancelled()) {
            return FLUX_NONE;
        }
        int64_t chunk = raw_ms > 10 ? 10 : raw_ms;
#if defined(_MSC_VER) || defined(_WIN32)
        Sleep((DWORD)chunk);
#else
        usleep((useconds_t)chunk * 1000);
#endif
        raw_ms -= chunk;
    }
    return FLUX_NONE;
}

int64_t flux_async_yield_now(void) {
#if defined(_MSC_VER) || defined(_WIN32)
    Sleep(0);
#else
    sched_yield();
#endif
    return FLUX_NONE;
}

static int64_t flux_async_unimplemented(const char *name) {
    char buf[176];
    int len = snprintf(buf, sizeof(buf),
                       "%s requires the native Rust async bridge",
                       name);
    if (len < 0) {
        flux_panic(flux_string_new("async runtime unavailable", 25));
    }
    if (len >= (int)sizeof(buf)) {
        len = (int)sizeof(buf) - 1;
    }
    flux_panic(flux_string_new(buf, (uint32_t)len));
    return FLUX_NONE;
}

static int64_t flux_call_thunk0(int64_t action, const char *op) {
    if (!flux_is_ptr(action) || flux_obj_tag(flux_untag_ptr(action)) != FLUX_OBJ_CLOSURE) {
        char buf[96];
        int len = snprintf(buf, sizeof(buf), "%s expects a closure", op);
        if (len < 0) {
            flux_panic(flux_string_new("async operation expects a closure", 33));
            return FLUX_NONE;
        }
        if (len >= (int)sizeof(buf)) {
            len = (int)sizeof(buf) - 1;
        }
        flux_panic(flux_string_new(buf, (uint32_t)len));
        return FLUX_NONE;
    }
    return flux_call_closure_c(action, NULL, 0);
}

static int64_t flux_call_closure1(int64_t action, int64_t arg, const char *op) {
    if (!flux_is_ptr(action) || flux_obj_tag(flux_untag_ptr(action)) != FLUX_OBJ_CLOSURE) {
        char buf[96];
        int len = snprintf(buf, sizeof(buf), "%s expects a closure", op);
        if (len < 0) {
            flux_panic(flux_string_new("async operation expects a closure", 33));
            return FLUX_NONE;
        }
        if (len >= (int)sizeof(buf)) {
            len = (int)sizeof(buf) - 1;
        }
        flux_panic(flux_string_new(buf, (uint32_t)len));
        return FLUX_NONE;
    }
    int64_t args[1] = { arg };
    return flux_call_closure_c(action, args, 1);
}

static FluxNativeScope *flux_native_scope_new(void) {
    FluxNativeScope *scope = (FluxNativeScope *)flux_gc_alloc_header(
        sizeof(FluxNativeScope), 0, FLUX_OBJ_SCOPE);
    memset(scope, 0, sizeof(*scope));
    pthread_mutex_init(&scope->mutex, NULL);
    return scope;
}

static int64_t flux_async_timed_out_value(void) {
    void *mem = flux_gc_alloc_header(8, 0, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = 7; /* Flow.Async.AsyncTimedOut */
    hdr[1] = 0;
    return flux_tag_ptr(mem);
}

static FluxNativeScope *flux_native_scope_unbox(int64_t scope_value, const char *op) {
    if (!flux_is_ptr(scope_value) || flux_obj_tag(flux_untag_ptr(scope_value)) != FLUX_OBJ_SCOPE) {
        char buf[96];
        int len = snprintf(buf, sizeof(buf), "%s expects Scope", op);
        if (len < 0) {
            flux_panic(flux_string_new("async operation expects Scope", 29));
            return NULL;
        }
        if (len >= (int)sizeof(buf)) {
            len = (int)sizeof(buf) - 1;
        }
        flux_panic(flux_string_new(buf, (uint32_t)len));
        return NULL;
    }
    return (FluxNativeScope *)flux_untag_ptr(scope_value);
}

static void flux_native_scope_add(FluxNativeScope *scope, FluxNativeTask *task) {
    pthread_mutex_lock(&scope->mutex);
    if (scope->count == scope->cap) {
        uint32_t next_cap = scope->cap == 0 ? 4 : scope->cap * 2;
        FluxNativeTask **next = (FluxNativeTask **)realloc(
            scope->tasks, sizeof(FluxNativeTask *) * next_cap);
        if (!next) {
            pthread_mutex_unlock(&scope->mutex);
            flux_panic(flux_string_new("Async.scope failed to grow child list", 38));
            return;
        }
        scope->tasks = next;
        scope->cap = next_cap;
    }
    scope->tasks[scope->count++] = task;
    pthread_mutex_unlock(&scope->mutex);
}

static int64_t flux_native_scope_join_children(FluxNativeScope *scope, int rethrow) {
    int64_t first_error = FLUX_NONE;
    pthread_mutex_lock(&scope->mutex);
    uint32_t count = scope->count;
    FluxNativeTask **tasks = scope->tasks;
    pthread_mutex_unlock(&scope->mutex);

    for (uint32_t i = 0; i < count; i++) {
        int64_t value = flux_native_task_join_raw(tasks[i], 0);
        if (first_error == FLUX_NONE && flux_native_task_failed(tasks[i])) {
            first_error = value;
        }
    }

    if (first_error != FLUX_NONE && rethrow) {
        flux_panic(first_error);
    }
    return first_error;
}

static void flux_native_scope_cancel_children(FluxNativeScope *scope) {
    pthread_mutex_lock(&scope->mutex);
    uint32_t count = scope->count;
    FluxNativeTask **tasks = scope->tasks;
    pthread_mutex_unlock(&scope->mutex);

    for (uint32_t i = 0; i < count; i++) {
        flux_native_task_cancel_raw(tasks[i]);
    }
}

static void flux_native_scope_clear_children(FluxNativeScope *scope) {
    pthread_mutex_lock(&scope->mutex);
    FluxNativeTask **tasks = scope->tasks;
    scope->tasks = NULL;
    scope->count = 0;
    scope->cap = 0;
    pthread_mutex_unlock(&scope->mutex);
    free(tasks);
}

static void flux_timespec_add_ms(struct timespec *ts, int64_t ms) {
    if (ms < 0) ms = 0;
    ts->tv_sec += (time_t)(ms / 1000);
    ts->tv_nsec += (long)((ms % 1000) * 1000000L);
    if (ts->tv_nsec >= 1000000000L) {
        ts->tv_sec += 1;
        ts->tv_nsec -= 1000000000L;
    }
}

static int flux_native_task_wait_ms(FluxNativeTask *task, int64_t ms) {
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    flux_timespec_add_ms(&deadline, ms);

    pthread_mutex_lock(&task->mutex);
    while (!task->completed) {
        int rc = pthread_cond_timedwait(&task->cond, &task->mutex, &deadline);
        if (rc == ETIMEDOUT && !task->completed) {
            pthread_mutex_unlock(&task->mutex);
            return 0;
        }
    }
    pthread_mutex_unlock(&task->mutex);
    return 1;
}

int64_t flux_async_both(int64_t left, int64_t right) {
    FluxNativeTask *left_task = flux_native_task_spawn_raw(left, "Async.both");
    FluxNativeTask *right_task = flux_native_task_spawn_raw(right, "Async.both");
    int64_t left_result = flux_native_task_join_raw(left_task, 0);
    int left_failed = flux_native_task_failed(left_task);
    int64_t right_result = flux_native_task_join_raw(right_task, 0);
    int right_failed = flux_native_task_failed(right_task);
    if (left_failed) {
        flux_panic(left_result);
        return FLUX_NONE;
    }
    if (right_failed) {
        flux_panic(right_result);
        return FLUX_NONE;
    }
    int64_t fields[2] = { left_result, right_result };
    return make_tuple(fields, 2);
}

int64_t flux_async_race(int64_t left, int64_t right) {
    FluxNativeTask *left_task = flux_native_task_spawn_raw(left, "Async.race");
    FluxNativeTask *right_task = flux_native_task_spawn_raw(right, "Async.race");
    FluxNativeTask *winner = NULL;
    FluxNativeTask *loser = NULL;
    while (!winner) {
        if (flux_native_task_is_completed(left_task)) {
            winner = left_task;
            loser = right_task;
            break;
        }
        if (flux_native_task_is_completed(right_task)) {
            winner = right_task;
            loser = left_task;
            break;
        }
#if defined(_MSC_VER) || defined(_WIN32)
        Sleep(1);
#else
        usleep(1000);
#endif
    }
    flux_native_task_detach_cancel_raw(loser);
    int64_t result = flux_native_task_join_raw(winner, 0);
    if (flux_native_task_failed(winner)) {
        flux_panic(result);
        return FLUX_NONE;
    }
    return result;
}

int64_t flux_async_timeout(int64_t ms, int64_t action) {
    int64_t raw_ms = flux_is_int(ms) ? flux_untag_int(ms) : 0;
    FluxNativeTask *task = flux_native_task_spawn_raw(action, "Async.timeout");
    if (!flux_native_task_wait_ms(task, raw_ms)) {
        flux_native_task_detach_cancel_raw(task);
        return FLUX_NONE;
    }
    int64_t result = flux_native_task_join_raw(task, 0);
    if (flux_native_task_failed(task)) {
        flux_panic(result);
        return FLUX_NONE;
    }
    return flux_wrap_some(result);
}

int64_t flux_async_timeout_result(int64_t ms, int64_t action) {
    int64_t raw_ms = flux_is_int(ms) ? flux_untag_int(ms) : 0;
    FluxNativeTask *task = flux_native_task_spawn_raw(action, "Async.timeout_result");
    if (!flux_native_task_wait_ms(task, raw_ms)) {
        flux_native_task_detach_cancel_raw(task);
        return flux_make_left(flux_async_timed_out_value());
    }
    int64_t result = flux_native_task_join_raw(task, 0);
    if (flux_native_task_failed(task)) {
        return flux_make_left(result);
    }
    return flux_make_right(result);
}

int64_t flux_async_scope(int64_t body) {
    FluxNativeScope *scope = flux_native_scope_new();
    int64_t scope_value = flux_tag_ptr(scope);

    jmp_buf buf;
    jmp_buf *prev = flux_try_jmp;
    int64_t prev_msg = flux_try_error_msg;
    flux_try_jmp = &buf;
    flux_try_error_msg = FLUX_NONE;

    if (setjmp(buf) == 0) {
        int64_t result = flux_call_closure1(body, scope_value, "Async.scope");
        flux_try_jmp = prev;
        flux_try_error_msg = prev_msg;
        int64_t child_error = flux_native_scope_join_children(scope, 0);
        flux_native_scope_clear_children(scope);
        if (child_error != FLUX_NONE) {
            flux_panic(child_error);
        }
        return result;
    }

    int64_t err = flux_try_error_msg;
    flux_try_jmp = prev;
    flux_try_error_msg = prev_msg;
    flux_native_scope_cancel_children(scope);
    (void)flux_native_scope_join_children(scope, 0);
    flux_native_scope_clear_children(scope);
    flux_panic(err);
    return FLUX_NONE;
}

int64_t flux_async_fork(int64_t scope, int64_t action) {
    FluxNativeScope *native_scope = flux_native_scope_unbox(scope, "Async.fork");
    if (!native_scope) return FLUX_NONE;
    FluxNativeTask *task = flux_native_task_spawn_raw(action, "Async.fork");
    flux_native_scope_add(native_scope, task);
    return FLUX_NONE;
}

int64_t flux_async_try(int64_t body) {
    jmp_buf buf;
    jmp_buf *prev = flux_try_jmp;
    int64_t prev_msg = flux_try_error_msg;
    flux_try_jmp = &buf;
    flux_try_error_msg = FLUX_NONE;

    if (setjmp(buf) == 0) {
        int64_t result = flux_call_thunk0(body, "Async.try");
        flux_try_jmp = prev;
        flux_try_error_msg = prev_msg;
        return flux_make_right(result);
    }

    int64_t err = flux_try_error_msg;
    flux_try_jmp = prev;
    flux_try_error_msg = prev_msg;
    return flux_make_left(err);
}

int64_t flux_async_finally(int64_t body, int64_t cleanup) {
    jmp_buf buf;
    jmp_buf *prev = flux_try_jmp;
    int64_t prev_msg = flux_try_error_msg;
    flux_try_jmp = &buf;
    flux_try_error_msg = FLUX_NONE;

    if (setjmp(buf) == 0) {
        int64_t result = flux_call_thunk0(body, "Async.finally");
        flux_try_jmp = prev;
        int64_t body_msg = flux_try_error_msg;
        flux_try_error_msg = prev_msg;
        (void)body_msg;
        (void)flux_call_thunk0(cleanup, "Async.finally");
        return result;
    }

    int64_t err = flux_try_error_msg;
    flux_try_jmp = prev;
    flux_try_error_msg = prev_msg;
    (void)flux_call_thunk0(cleanup, "Async.finally");
    flux_panic(err);
    return FLUX_NONE;
}

int64_t flux_async_bracket(int64_t acquire, int64_t release, int64_t body) {
    int64_t resource = flux_call_thunk0(acquire, "Async.bracket");

    jmp_buf buf;
    jmp_buf *prev = flux_try_jmp;
    int64_t prev_msg = flux_try_error_msg;
    flux_try_jmp = &buf;
    flux_try_error_msg = FLUX_NONE;

    if (setjmp(buf) == 0) {
        int64_t result = flux_call_closure1(body, resource, "Async.bracket");
        flux_try_jmp = prev;
        int64_t body_msg = flux_try_error_msg;
        flux_try_error_msg = prev_msg;
        (void)body_msg;
        (void)flux_call_closure1(release, resource, "Async.bracket");
        return result;
    }

    int64_t err = flux_try_error_msg;
    flux_try_jmp = prev;
    flux_try_error_msg = prev_msg;
    (void)flux_call_closure1(release, resource, "Async.bracket");
    flux_panic(err);
    return FLUX_NONE;
}

/* ── Native TCP compatibility shims ────────────────────────────────── */

static void flux_tcp_panic(const char *op, const char *detail) {
    char buf[256];
    int len = snprintf(buf, sizeof(buf), "%s failed: %s", op, detail ? detail : "unknown error");
    if (len < 0) {
        flux_panic(flux_string_new("TCP operation failed", 20));
        return;
    }
    if (len >= (int)sizeof(buf)) {
        len = (int)sizeof(buf) - 1;
    }
    flux_panic(flux_string_new(buf, (uint32_t)len));
}

static char *flux_string_to_cstr(int64_t value, const char *op) {
    if (!flux_is_ptr(value) || flux_obj_tag(flux_untag_ptr(value)) != FLUX_OBJ_STRING) {
        flux_tcp_panic(op, "expected String");
        return NULL;
    }
    uint32_t len = flux_string_len(value);
    char *buf = (char *)malloc((size_t)len + 1);
    if (!buf) {
        flux_tcp_panic(op, "out of memory");
        return NULL;
    }
    memcpy(buf, flux_string_data(value), len);
    buf[len] = '\0';
    return buf;
}

static int flux_tcp_port(int64_t port, const char *op) {
    if (!flux_is_int(port)) {
        flux_tcp_panic(op, "expected Int port");
        return 0;
    }
    int64_t raw = flux_untag_int(port);
    if (raw < 0 || raw > 65535) {
        flux_tcp_panic(op, "port out of range");
        return 0;
    }
    return (int)raw;
}

static int flux_tcp_fd(int64_t value, const char *op) {
    if (!flux_is_int(value)) {
        flux_tcp_panic(op, "expected TCP handle");
        return -1;
    }
    int64_t raw = flux_untag_int(value);
    if (raw < 0 || raw > INT32_MAX) {
        flux_tcp_panic(op, "invalid TCP handle");
        return -1;
    }
    return (int)raw;
}

static int flux_tcp_socket_for(const char *host, int port, int passive, const char *op) {
    char service[16];
    snprintf(service, sizeof(service), "%d", port);

    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = passive ? AI_PASSIVE : 0;

    struct addrinfo *res = NULL;
    int gai = getaddrinfo(host, service, &hints, &res);
    if (gai != 0) {
        flux_tcp_panic(op, gai_strerror(gai));
        return -1;
    }

    int fd = -1;
    for (struct addrinfo *ai = res; ai != NULL; ai = ai->ai_next) {
        fd = socket(ai->ai_family, ai->ai_socktype, ai->ai_protocol);
        if (fd < 0) {
            continue;
        }
        if (passive) {
            int yes = 1;
            (void)setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
            if (bind(fd, ai->ai_addr, ai->ai_addrlen) == 0 && listen(fd, 128) == 0) {
                break;
            }
        } else if (connect(fd, ai->ai_addr, ai->ai_addrlen) == 0) {
            break;
        }
        close(fd);
        fd = -1;
    }

    freeaddrinfo(res);
    if (fd < 0) {
        flux_tcp_panic(op, strerror(errno));
    }
    return fd;
}

static int64_t flux_tcp_addr_string(int fd, int peer, const char *op) {
    struct sockaddr_storage addr;
    socklen_t len = sizeof(addr);
    int rc = peer
        ? getpeername(fd, (struct sockaddr *)&addr, &len)
        : getsockname(fd, (struct sockaddr *)&addr, &len);
    if (rc != 0) {
        flux_tcp_panic(op, strerror(errno));
        return FLUX_NONE;
    }

    char host[NI_MAXHOST];
    char service[NI_MAXSERV];
    rc = getnameinfo(
        (struct sockaddr *)&addr,
        len,
        host,
        sizeof(host),
        service,
        sizeof(service),
        NI_NUMERICHOST | NI_NUMERICSERV);
    if (rc != 0) {
        flux_tcp_panic(op, gai_strerror(rc));
        return FLUX_NONE;
    }

    char buf[NI_MAXHOST + NI_MAXSERV + 2];
    int out = snprintf(buf, sizeof(buf), "%s:%s", host, service);
    if (out < 0) {
        flux_tcp_panic(op, "address formatting failed");
        return FLUX_NONE;
    }
    if (out >= (int)sizeof(buf)) {
        out = (int)sizeof(buf) - 1;
    }
    return flux_string_new(buf, (uint32_t)out);
}

int64_t flux_tcp_listen(int64_t host_value, int64_t port_value) {
    char *host = flux_string_to_cstr(host_value, "Tcp.listen");
    int port = flux_tcp_port(port_value, "Tcp.listen");
    int fd = flux_tcp_socket_for(host, port, 1, "Tcp.listen");
    free(host);
    return fd >= 0 ? flux_tag_int(fd) : FLUX_NONE;
}

int64_t flux_tcp_accept(int64_t listener) {
    int fd = flux_tcp_fd(listener, "Tcp.accept");
    int conn = accept(fd, NULL, NULL);
    if (conn < 0) {
        flux_tcp_panic("Tcp.accept", strerror(errno));
        return FLUX_NONE;
    }
    return flux_tag_int(conn);
}

int64_t flux_tcp_connect(int64_t host_value, int64_t port_value) {
    char *host = flux_string_to_cstr(host_value, "Tcp.connect");
    int port = flux_tcp_port(port_value, "Tcp.connect");
    int fd = flux_tcp_socket_for(host, port, 0, "Tcp.connect");
    free(host);
    return fd >= 0 ? flux_tag_int(fd) : FLUX_NONE;
}

int64_t flux_tcp_read(int64_t conn, int64_t max_value) {
    int fd = flux_tcp_fd(conn, "Tcp.read");
    if (!flux_is_int(max_value)) {
        flux_tcp_panic("Tcp.read", "expected Int byte count");
        return FLUX_NONE;
    }
    int64_t max = flux_untag_int(max_value);
    if (max <= 0) {
        return flux_string_new("", 0);
    }
    char *buf = (char *)malloc((size_t)max);
    if (!buf) {
        flux_tcp_panic("Tcp.read", "out of memory");
        return FLUX_NONE;
    }
    ssize_t nread = recv(fd, buf, (size_t)max, 0);
    if (nread < 0) {
        free(buf);
        flux_tcp_panic("Tcp.read", strerror(errno));
        return FLUX_NONE;
    }
    int64_t result = flux_string_new(buf, (uint32_t)nread);
    free(buf);
    return result;
}

int64_t flux_tcp_write(int64_t conn, int64_t bytes) {
    int fd = flux_tcp_fd(conn, "Tcp.write");
    if (!flux_is_ptr(bytes) || flux_obj_tag(flux_untag_ptr(bytes)) != FLUX_OBJ_STRING) {
        flux_tcp_panic("Tcp.write", "expected Bytes");
        return FLUX_NONE;
    }
    const char *data = flux_string_data(bytes);
    uint32_t len = flux_string_len(bytes);
    ssize_t nwritten = send(fd, data, len, 0);
    if (nwritten < 0) {
        flux_tcp_panic("Tcp.write", strerror(errno));
        return FLUX_NONE;
    }
    return flux_tag_int((int64_t)nwritten);
}

int64_t flux_tcp_close(int64_t conn) {
    int fd = flux_tcp_fd(conn, "Tcp.close");
    if (close(fd) != 0) {
        flux_tcp_panic("Tcp.close", strerror(errno));
    }
    return FLUX_NONE;
}

int64_t flux_tcp_local_addr(int64_t conn) {
    return flux_tcp_addr_string(flux_tcp_fd(conn, "Tcp.local_addr"), 0, "Tcp.local_addr");
}

int64_t flux_tcp_remote_addr(int64_t conn) {
    return flux_tcp_addr_string(flux_tcp_fd(conn, "Tcp.remote_addr"), 1, "Tcp.remote_addr");
}

int64_t flux_tcp_close_listener(int64_t listener) {
    int fd = flux_tcp_fd(listener, "Tcp.close_listener");
    if (close(fd) != 0) {
        flux_tcp_panic("Tcp.close_listener", strerror(errno));
    }
    return FLUX_NONE;
}

int64_t flux_tcp_listener_local_addr(int64_t listener) {
    return flux_tcp_addr_string(
        flux_tcp_fd(listener, "Tcp.listener_local_addr"),
        0,
        "Tcp.listener_local_addr");
}

/* ── Runtime-callable type predicates ──────────────────────────────── */
/* These return tagged booleans (FLUX_TRUE/FLUX_FALSE) for Flux code.
 * Named with _val suffix to avoid collision with the static inline
 * helpers (flux_is_int, flux_is_ptr) from flux_rt.h. */
int64_t flux_is_int_val(int64_t val) {
    return flux_make_bool(val & 1);
}

int64_t flux_is_float_val(int64_t val) {
    return flux_make_bool(flux_val_is_float(val));
}

int64_t flux_is_string_val(int64_t val) {
    return flux_make_bool(flux_is_ptr(val) && flux_obj_tag(flux_untag_ptr(val)) == FLUX_OBJ_STRING);
}

int64_t flux_is_bool_val(int64_t val) {
    return flux_make_bool(val == FLUX_TRUE || val == FLUX_FALSE);
}

int64_t flux_is_none_val(int64_t val) {
    return flux_make_bool(val == FLUX_NONE);
}

/* ── Non-inline wrappers for LLVM codegen (Phase 9 pointer tagging) ──── */

int64_t flux_box_float_rt(double f) {
    return flux_box_float(f);
}

double flux_unbox_float_rt(int64_t val) {
    return flux_unbox_float(val);
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
