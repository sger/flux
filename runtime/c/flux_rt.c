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
void flux_print(int64_t val) {
    uint64_t bits = (uint64_t)val;

    /* Float: top 14 bits are NOT the sentinel → raw IEEE double. */
    if ((bits & FLUX_SENTINEL_MASK) != FLUX_NANBOX_SENTINEL) {
        double d;
        memcpy(&d, &bits, sizeof(d));
        /* Print without trailing zeros: use %g. */
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
        /*
         * Boxed values can be strings, ADTs, closures, etc.
         * For now, we only handle strings (identified by the FluxString
         * layout).  Other boxed types print as "<object>".
         *
         * String detection heuristic: the first 4 bytes are the length,
         * followed by that many valid bytes.  This is fragile; Phase 8
         * should add a type tag to the object header.
         */
        void *ptr = flux_untag_ptr(val);
        if (ptr) {
            /*
             * Assume string for now — the codegen tags strings via
             * flux_string_new which uses the FluxString layout.
             * A type-tag system will be added in a later phase.
             */
            uint32_t len = *(uint32_t *)ptr;
            const char *data = (const char *)ptr + sizeof(uint32_t);
            /* Sanity check: len should be reasonable. */
            if (len < 1024 * 1024) {
                fwrite(data, 1, len, stdout);
            } else {
                printf("<object@%p>", ptr);
            }
        } else {
            printf("<null>");
        }
        break;
    }

    default:
        printf("<unknown tag=%d>", tag);
        break;
    }
}

void flux_println(int64_t val) {
    flux_print(val);
    putchar('\n');
    fflush(stdout);
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
