/*
 * flux_rt.h — Flux minimal C runtime for the core_to_llvm backend.
 *
 * This runtime provides only what cannot be expressed as pure LLVM IR:
 * GC allocation, I/O, string helpers, HAMT persistent maps, and effect
 * handler continuations.  Arithmetic, closures, ADTs, and pattern matching
 * are emitted as inline LLVM IR by the codegen and are NOT part of this
 * runtime.
 *
 * NaN-box layout (must match src/runtime/nanbox.rs exactly):
 *   bits [63:50] = 0x7FFC (sentinel)
 *   bits [49:46] = 4-bit tag
 *   bits [45:0]  = 46-bit payload
 *
 * Tags:
 *   0x0 = Integer   (46-bit signed, two's complement)
 *   0x1 = Boolean   (payload: 0=false, 1=true)
 *   0x2 = None
 *   0x3 = Uninit
 *   0x4 = EmptyList
 *   0x5 = BaseFunction
 *   0x8 = BoxedValue (heap ptr >> 3 in payload)
 *
 * Floats are stored as raw IEEE 754 bits (no sentinel).
 */

#ifndef FLUX_RT_H
#define FLUX_RT_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── NaN-box constants ──────────────────────────────────────────────── */

#define FLUX_NANBOX_SENTINEL   ((uint64_t)0x7FFC000000000000ULL)
#define FLUX_SENTINEL_MASK     ((uint64_t)0xFFFC000000000000ULL)
#define FLUX_TAG_SHIFT         46
#define FLUX_TAG_MASK          ((uint64_t)0xF)
#define FLUX_PAYLOAD_MASK      ((uint64_t)((1ULL << 46) - 1))

#define FLUX_TAG_INTEGER       0x0
#define FLUX_TAG_BOOLEAN       0x1
#define FLUX_TAG_NONE          0x2
#define FLUX_TAG_UNINIT        0x3
#define FLUX_TAG_EMPTY_LIST    0x4
#define FLUX_TAG_BASE_FUNCTION 0x5
#define FLUX_TAG_BOXED_VALUE   0x8

#define FLUX_PTR_SHIFT         3

/* ── Inline NaN-box helpers ─────────────────────────────────────────── */

static inline int64_t flux_tag_int(int64_t raw) {
    uint64_t payload = (uint64_t)raw & FLUX_PAYLOAD_MASK;
    return (int64_t)(payload | FLUX_NANBOX_SENTINEL);
}

static inline int64_t flux_untag_int(int64_t val) {
    uint64_t payload = (uint64_t)val & FLUX_PAYLOAD_MASK;
    /* Sign-extend from 46 bits. */
    return (int64_t)(payload << 18) >> 18;
}

static inline int flux_is_nanbox(int64_t val) {
    return ((uint64_t)val & FLUX_SENTINEL_MASK) == FLUX_NANBOX_SENTINEL;
}

static inline int flux_nanbox_tag(int64_t val) {
    return (int)(((uint64_t)val >> FLUX_TAG_SHIFT) & FLUX_TAG_MASK);
}

static inline int flux_is_ptr(int64_t val) {
    if (!flux_is_nanbox(val)) return 0;
    return flux_nanbox_tag(val) == FLUX_TAG_BOXED_VALUE;
}

static inline void *flux_untag_ptr(int64_t val) {
    uint64_t payload = (uint64_t)val & FLUX_PAYLOAD_MASK;
    return (void *)(payload << FLUX_PTR_SHIFT);
}

static inline int64_t flux_tag_ptr(void *ptr) {
    uint64_t payload = (uint64_t)ptr >> FLUX_PTR_SHIFT;
    return (int64_t)(FLUX_NANBOX_SENTINEL
                     | ((uint64_t)FLUX_TAG_BOXED_VALUE << FLUX_TAG_SHIFT)
                     | payload);
}

static inline int64_t flux_make_none(void) {
    return (int64_t)(FLUX_NANBOX_SENTINEL
                     | ((uint64_t)FLUX_TAG_NONE << FLUX_TAG_SHIFT));
}

static inline int64_t flux_make_bool(int b) {
    return (int64_t)(FLUX_NANBOX_SENTINEL
                     | ((uint64_t)FLUX_TAG_BOOLEAN << FLUX_TAG_SHIFT)
                     | (uint64_t)(b ? 1 : 0));
}

static inline int64_t flux_make_empty_list(void) {
    return (int64_t)(FLUX_NANBOX_SENTINEL
                     | ((uint64_t)FLUX_TAG_EMPTY_LIST << FLUX_TAG_SHIFT));
}

/* ── Heap object type tags ──────────────────────────────────────────── */
/*
 * Every heap-allocated object starts with a uint8_t type tag so that
 * flux_print (and future GC) can identify the object kind at runtime.
 * The tag occupies the first byte; the remaining layout is type-specific.
 */

/* Tags are chosen to not collide with ADT constructor tags (0-255). */
#define FLUX_OBJ_STRING   0xF1
#define FLUX_OBJ_ADT      0xF2
#define FLUX_OBJ_TUPLE    0xF3
#define FLUX_OBJ_ARRAY    0xF4
#define FLUX_OBJ_CLOSURE  0xF5

static inline uint8_t flux_obj_tag(void *ptr) {
    return *(uint8_t *)ptr;
}

/* ── GC ─────────────────────────────────────────────────────────────── */

void  flux_gc_init(size_t heap_size);
void  flux_gc_shutdown(void);
void *flux_gc_alloc(uint32_t size);
void  flux_gc_free(void *ptr);
void  flux_gc_collect(void);
void  flux_gc_push_root(int64_t *root);
void  flux_gc_pop_root(void);

/* Allocation stats (for diagnostics / testing). */
size_t flux_gc_allocated(void);
size_t flux_gc_num_allocs(void);

/* ── I/O ────────────────────────────────────────────────────────────── */

void    flux_print(int64_t value);
void    flux_println(int64_t value);
int64_t flux_read_line(void);
int64_t flux_read_file(int64_t path);
int64_t flux_write_file(int64_t path, int64_t content);

/* ── Runtime lifecycle ──────────────────────────────────────────────── */

void flux_rt_init(void);
void flux_rt_shutdown(void);

/* ── Strings ────────────────────────────────────────────────────────── */

/*
 * FluxString layout (heap-allocated, pointed to by BoxedValue tag):
 *   struct { uint32_t len; char data[]; }
 */

int64_t flux_string_new(const char *data, uint32_t len);
int64_t flux_string_concat(int64_t a, int64_t b);
int64_t flux_string_slice(int64_t s, int64_t start, int64_t end);
int64_t flux_string_length(int64_t s);
int64_t flux_int_to_string(int64_t n);
int64_t flux_float_to_string(int64_t f);
int64_t flux_string_to_int(int64_t s);
int     flux_string_eq(int64_t a, int64_t b);

/* Access raw C string pointer and length (valid until next GC). */
const char *flux_string_data(int64_t s);
uint32_t    flux_string_len(int64_t s);

/* ── Arrays ─────────────────────────────────────────────────────────── */

int64_t flux_array_new(int64_t *elements, int32_t len);
int64_t flux_array_len(int64_t arr);
int64_t flux_array_get(int64_t arr, int64_t index);
int64_t flux_array_set(int64_t arr, int64_t index, int64_t value);
int64_t flux_array_push(int64_t arr, int64_t value);
int64_t flux_array_concat(int64_t a, int64_t b);
int64_t flux_array_slice(int64_t arr, int64_t start, int64_t end);
int64_t flux_array_reverse(int64_t arr);
int64_t flux_array_contains(int64_t arr, int64_t value);

/* ── HAMT (persistent hash map) ─────────────────────────────────────── */

int64_t flux_hamt_empty(void);
int64_t flux_hamt_get(int64_t map, int64_t key);
int64_t flux_hamt_set(int64_t map, int64_t key, int64_t value);
int64_t flux_hamt_delete(int64_t map, int64_t key);
int64_t flux_hamt_contains(int64_t map, int64_t key);
int64_t flux_hamt_size(int64_t map);

/* ── Numeric ────────────────────────────────────────────────────────── */

int64_t flux_abs(int64_t n);
int64_t flux_min(int64_t a, int64_t b);
int64_t flux_max(int64_t a, int64_t b);

/* ── Type inspection ────────────────────────────────────────────────── */

int64_t flux_type_of(int64_t val);
int64_t flux_is_int(int64_t val);
int64_t flux_is_float(int64_t val);
int64_t flux_is_string(int64_t val);
int64_t flux_is_bool(int64_t val);
int64_t flux_is_none(int64_t val);

/* ── Control ────────────────────────────────────────────────────────── */

void    flux_panic(int64_t msg);
int64_t flux_clock_now(void);

/* ── Extended I/O ───────────────────────────────────────────────────── */

int64_t flux_read_lines(int64_t path);
int64_t flux_trim(int64_t s);
int64_t flux_split(int64_t s, int64_t delim);
int64_t flux_join(int64_t list, int64_t sep);
int64_t flux_substring(int64_t s, int64_t start, int64_t end);
int64_t flux_parse_int(int64_t s);
int64_t flux_to_string(int64_t val);

/* ── Effect handlers ────────────────────────────────────────────────── */

void    flux_push_handler(int64_t effect_tag, void *handler_fn, void *resume_fn);
void    flux_pop_handler(void);
int64_t flux_perform(int64_t effect_tag, int64_t arg);
int64_t flux_resume(int64_t continuation, int64_t value);

#ifdef __cplusplus
}
#endif

#endif /* FLUX_RT_H */
