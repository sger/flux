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
#define FLUX_TAG_THUNK         0x6
#define FLUX_TAG_BOXED_VALUE   0x8
#define FLUX_TAG_YIELD         0x9

/* Yield sentinel: returned by perform / yield_extend to signal unwinding. */
#define FLUX_YIELD_SENTINEL    ((int64_t)(FLUX_NANBOX_SENTINEL \
                                | ((uint64_t)FLUX_TAG_YIELD << FLUX_TAG_SHIFT)))

#define FLUX_PTR_SHIFT         3

/* ── Inline NaN-box helpers ─────────────────────────────────────────── */

#define FLUX_MAX_INLINE_INT  ((int64_t)((1LL << 45) - 1))   /*  35_184_372_088_831 */
#define FLUX_MIN_INLINE_INT  ((int64_t)(-(1LL << 45)))      /* -35_184_372_088_832 */

/* BigInt heap tag — defined early so inline helpers can reference it. */
#define FLUX_OBJ_BIGINT   0xF6

/* Forward declarations for overflow boxing. */
void *flux_gc_alloc(uint32_t size);
void *flux_gc_alloc_header(uint32_t payload_size, uint8_t scan_fsize, uint8_t obj_tag);

static inline int64_t flux_tag_int(int64_t raw) {
    if (raw >= FLUX_MIN_INLINE_INT && raw <= FLUX_MAX_INLINE_INT) {
        uint64_t payload = (uint64_t)raw & FLUX_PAYLOAD_MASK;
        return (int64_t)(payload | FLUX_NANBOX_SENTINEL);
    }
    /* Overflow: heap-box the full 64-bit integer.
     * Layout: { uint8_t obj_tag=FLUX_OBJ_BIGINT, pad[7], int64_t value } */
    void *mem = flux_gc_alloc_header(16, 0, FLUX_OBJ_BIGINT);
    *(uint8_t *)mem = FLUX_OBJ_BIGINT;
    *(int64_t *)((char *)mem + 8) = raw;
    uint64_t payload = (uint64_t)mem >> FLUX_PTR_SHIFT;
    return (int64_t)(FLUX_NANBOX_SENTINEL
                     | ((uint64_t)FLUX_TAG_BOXED_VALUE << FLUX_TAG_SHIFT)
                     | payload);
}

static inline int64_t flux_untag_int(int64_t val) {
    /* Check for heap-boxed big integer. */
    if (((uint64_t)val & FLUX_SENTINEL_MASK) == FLUX_NANBOX_SENTINEL
        && ((((uint64_t)val >> FLUX_TAG_SHIFT) & FLUX_TAG_MASK) == FLUX_TAG_BOXED_VALUE)) {
        uint64_t p = (uint64_t)val & FLUX_PAYLOAD_MASK;
        void *ptr = (void *)(p << FLUX_PTR_SHIFT);
        if (ptr && *(uint8_t *)ptr == FLUX_OBJ_BIGINT) {
            return *(int64_t *)((char *)ptr + 8);
        }
    }
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

static inline int flux_is_thunk(int64_t val) {
    if (!flux_is_nanbox(val)) return 0;
    return flux_nanbox_tag(val) == FLUX_TAG_THUNK;
}

static inline void *flux_untag_thunk(int64_t val) {
    uint64_t payload = (uint64_t)val & FLUX_PAYLOAD_MASK;
    return (void *)(payload << FLUX_PTR_SHIFT);
}

static inline int64_t flux_tag_thunk(void *ptr) {
    uint64_t payload = (uint64_t)ptr >> FLUX_PTR_SHIFT;
    return (int64_t)(FLUX_NANBOX_SENTINEL
                     | ((uint64_t)FLUX_TAG_THUNK << FLUX_TAG_SHIFT)
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
/* FLUX_OBJ_BIGINT (0xF6) defined above near inline helpers */
#define FLUX_OBJ_EVIDENCE 0xF7

static inline uint8_t flux_obj_tag(void *ptr) {
    /* Read obj_tag from the FluxHeader at ptr - 8.
     * Layout: { i32 refcount, u8 scan_fsize, u8 obj_tag, u16 reserved }
     * obj_tag is at offset 5 within the header. */
    return *((uint8_t *)ptr - 3);
}

/* ── Allocation & Reference Counting (Aether RC) ──────────────────── */
/*
 * Every heap object has an 8-byte FluxHeader at (payload - 8):
 *   { int32_t refcount, uint8_t scan_fsize, uint8_t obj_tag, uint16_t reserved }
 *
 * flux_gc_alloc_header: allocate with explicit scan_fsize and obj_tag
 * flux_gc_alloc: backward-compatible (scan_fsize=0, obj_tag=0)
 * flux_dup: increment refcount
 * flux_drop: decrement refcount, recursively drop scan_fsize children, free at 0
 */

void  flux_gc_init(size_t heap_size);
void  flux_gc_shutdown(void);
void *flux_gc_alloc(uint32_t size);
void *flux_gc_alloc_header(uint32_t payload_size, uint8_t scan_fsize, uint8_t obj_tag);
void *flux_bump_alloc_slow(uint32_t payload_size, uint8_t scan_fsize, uint8_t obj_tag);
void  flux_gc_free(void *ptr);
void  flux_gc_collect(void);
void  flux_gc_push_root(int64_t *root);
void  flux_gc_pop_root(void);

/* Bump arena pointers (exported for Phase 7b inline LLVM allocation). */
extern char *flux_arena_hp;
extern char *flux_arena_limit;

/* Aether RC: dup/drop for NaN-boxed heap values. */
void flux_dup(int64_t val);
void flux_drop(int64_t val);
int  flux_rc_is_unique(int64_t val);

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
int64_t flux_reverse(int64_t collection);
int64_t flux_contains(int64_t collection, int64_t value);

/* ── HAMT (persistent hash map) ─────────────────────────────────────── */

int64_t flux_hamt_empty(void);
int64_t flux_hamt_get(int64_t map, int64_t key);
int64_t flux_hamt_set(int64_t map, int64_t key, int64_t value);
int64_t flux_hamt_delete(int64_t map, int64_t key);
int64_t flux_hamt_contains(int64_t map, int64_t key);
int64_t flux_hamt_size(int64_t map);
int     flux_is_hamt(void *ptr);
int64_t flux_hamt_format(int64_t map);

/* ── Numeric ────────────────────────────────────────────────────────── */

int64_t flux_abs(int64_t n);
int64_t flux_min(int64_t a, int64_t b);
int64_t flux_max(int64_t a, int64_t b);

/* ── Runtime-dispatching arithmetic (int/float/string) ──────────────── */

int64_t flux_rt_add(int64_t a, int64_t b);
int64_t flux_rt_sub(int64_t a, int64_t b);
int64_t flux_rt_mul(int64_t a, int64_t b);
int64_t flux_rt_div(int64_t a, int64_t b);
int64_t flux_rt_mod(int64_t a, int64_t b);
int64_t flux_rt_neg(int64_t a);

/* Print value with trailing space (for multi-arg print). */
void    flux_print_space(int64_t val);

/* Runtime-dispatching comparisons (int/float/string/tuple). */
int64_t flux_rt_eq(int64_t a, int64_t b);
int64_t flux_rt_neq(int64_t a, int64_t b);
int64_t flux_rt_lt(int64_t a, int64_t b);
int64_t flux_rt_le(int64_t a, int64_t b);
int64_t flux_rt_gt(int64_t a, int64_t b);
int64_t flux_rt_ge(int64_t a, int64_t b);

/* Runtime-dispatching index (array, tuple, HAMT). */
int64_t flux_rt_index(int64_t collection, int64_t key);

/* ── Type inspection ────────────────────────────────────────────────── */

int64_t flux_type_of(int64_t val);
int64_t flux_is_int(int64_t val);
int64_t flux_is_float(int64_t val);
int64_t flux_is_string(int64_t val);
int64_t flux_is_bool(int64_t val);
int64_t flux_is_none(int64_t val);

/* ── Control ────────────────────────────────────────────────────────── */

void    flux_panic(int64_t msg);
void    flux_trace_push(const char *name, const char *file, int32_t line);
void    flux_trace_pop(void);
int64_t flux_clock_now(void);

/* ── Extended I/O ───────────────────────────────────────────────────── */

int64_t flux_read_lines(int64_t path);
int64_t flux_trim(int64_t s);
int64_t flux_upper(int64_t s);
int64_t flux_lower(int64_t s);
int64_t flux_replace(int64_t s, int64_t from, int64_t to);
int64_t flux_chars(int64_t s);
int64_t flux_str_contains(int64_t haystack, int64_t needle);
int64_t flux_split(int64_t s, int64_t delim);
int64_t flux_join(int64_t list, int64_t sep);
int64_t flux_substring(int64_t s, int64_t start, int64_t end);
int64_t flux_parse_int(int64_t s);
int64_t flux_parse_ints(int64_t arr);
int64_t flux_to_string(int64_t val);
int64_t flux_to_string_value(int64_t val);

/* ── Collection helpers ─────────────────────────────────────────────── */

int64_t flux_rt_len(int64_t collection);
int64_t flux_to_list(int64_t arr);
int64_t flux_is_array(int64_t val);
int64_t flux_is_map(int64_t val);
int64_t flux_hamt_keys(int64_t map);
int64_t flux_hamt_values(int64_t map);
int64_t flux_hamt_get_option(int64_t map, int64_t key);
int64_t flux_hamt_merge(int64_t a, int64_t b);
int64_t flux_to_array(int64_t list);
int64_t flux_hamt_values(int64_t map);
int64_t flux_is_list(int64_t val);
int64_t flux_is_some(int64_t val);
int64_t flux_unwrap(int64_t val);
int64_t flux_unwrap_or(int64_t val, int64_t def);

int64_t flux_sum(int64_t collection);
int64_t flux_sort_default(int64_t collection);
int64_t flux_split_ints(int64_t s, int64_t delim);
int64_t flux_zip(int64_t a, int64_t b);
int64_t flux_flatten(int64_t collection);
int64_t flux_ho_flat_map(int64_t collection, int64_t func);
int64_t flux_starts_with(int64_t s, int64_t prefix);
int64_t flux_ends_with(int64_t s, int64_t suffix);

/* ── ADT construction (LIR native backend) ─────────────────────────── */
int64_t flux_wrap_some(int64_t val);
int64_t flux_make_left(int64_t val);
int64_t flux_make_right(int64_t val);

/* ── Globals table (LIR native backend) ─────────────────────────────── */
int64_t flux_get_global(int64_t idx);
void flux_set_global(int64_t idx, int64_t val);

/* ── Higher-order functions (closure calling) ──────────────────────── */
/* flux_call_closure_c is defined in LLVM IR (ccc trampoline). */
extern int64_t flux_call_closure_c(int64_t closure, int64_t *args, int32_t nargs);

int64_t flux_ho_map(int64_t collection, int64_t func);
int64_t flux_ho_filter(int64_t collection, int64_t func);
int64_t flux_ho_sort(int64_t collection, int64_t func);
int64_t flux_ho_any(int64_t collection, int64_t func);
int64_t flux_ho_all(int64_t collection, int64_t func);
int64_t flux_ho_fold(int64_t collection, int64_t init, int64_t func);
int64_t flux_ho_each(int64_t collection, int64_t func);
int64_t flux_ho_find(int64_t collection, int64_t func);

/* ── Effect handlers (Koka-style yield model, Proposal 0134) ───────── */

/* Yield state — accessible from LLVM IR for inline yield checks. */
extern int32_t flux_yield_yielding;

/* Evidence vector management. */
int64_t flux_evv_get(void);
void    flux_evv_set(int64_t evv);
int64_t flux_fresh_marker(void);
int64_t flux_evv_insert(int64_t evv, int64_t htag, int64_t marker, int64_t handler);

/* Yield operations. */
int64_t flux_yield_to(int64_t htag, int64_t optag, int64_t arg);
int64_t flux_perform_direct(int64_t htag, int64_t optag, int64_t arg, int64_t resume);
int64_t flux_yield_extend(int64_t cont);
int64_t flux_yield_prompt(int64_t marker, int64_t saved_evv, int64_t body_result);
int64_t flux_compose_conts(void);
int32_t flux_is_yielding(void);

#ifdef __cplusplus
}
#endif

#endif /* FLUX_RT_H */
