/*
 * flux_rt.h — Flux minimal C runtime for the core_to_llvm backend.
 *
 * This runtime provides only what cannot be expressed as pure LLVM IR:
 * GC allocation, I/O, string helpers, HAMT persistent maps, and effect
 * handler continuations.  Arithmetic, closures, ADTs, and pattern matching
 * are emitted as inline LLVM IR by the codegen and are NOT part of this
 * runtime.
 *
 * Pointer-tagged value encoding (Phase 9, Proposal 0124):
 *   bit 0 = 1  →  tagged integer: 63-bit signed (val >> 1)
 *   bit 0 = 0  →  heap pointer (8-byte aligned) or sentinel
 *
 * Sentinel values (even, < FLUX_MIN_PTR):
 *   0  = None
 *   2  = false
 *   4  = true
 *   6  = EmptyList
 *   8  = Uninit
 *   10 = YieldSentinel
 *
 * Floats are heap-boxed: pointer to { FluxHeader, double }.
 */

#ifndef FLUX_RT_H
#define FLUX_RT_H

#include <stdint.h>
#include <stddef.h>
#include <string.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── Pointer-tag sentinel values ───────────────────────────────────── */

#define FLUX_NONE              ((int64_t)0)
#define FLUX_FALSE             ((int64_t)2)
#define FLUX_TRUE              ((int64_t)4)
#define FLUX_EMPTY_LIST        ((int64_t)6)
#define FLUX_UNINIT            ((int64_t)8)
#define FLUX_YIELD_SENTINEL    ((int64_t)10)
#define FLUX_MIN_PTR           ((uint64_t)12)

/* ── Inline pointer-tag helpers ────────────────────────────────────── */

/* 63-bit signed integer range (±4.6 quintillion). */
#define FLUX_MAX_INLINE_INT  ((int64_t)((1LL << 62) - 1))
#define FLUX_MIN_INLINE_INT  ((int64_t)(-(1LL << 62)))

static inline int flux_is_int(int64_t val) {
    return (int)(val & 1);
}

static inline int flux_is_ptr(int64_t val) {
    return !(val & 1) && (uint64_t)val >= FLUX_MIN_PTR;
}

static inline int64_t flux_tag_int(int64_t raw) {
    return (raw << 1) | 1;
}

static inline int64_t flux_untag_int(int64_t val) {
    return val >> 1;  /* arithmetic right shift — sign-extends */
}

/* Pointer tagging is zero-cost: heap pointers are naturally even. */
static inline int64_t flux_tag_ptr(void *ptr) {
    return (int64_t)(uintptr_t)ptr;
}

static inline void *flux_untag_ptr(int64_t val) {
    return (void *)(uintptr_t)val;
}

/* Booleans */
static inline int64_t flux_make_bool(int b) {
    return b ? FLUX_TRUE : FLUX_FALSE;
}

/* None / EmptyList */
static inline int64_t flux_make_none(void) { return FLUX_NONE; }
static inline int64_t flux_make_empty_list(void) { return FLUX_EMPTY_LIST; }

/* Float boxing — floats are heap-allocated { FluxHeader, double }. */
void *flux_gc_alloc_header(uint32_t payload_size, uint8_t scan_fsize, uint8_t obj_tag);

static inline int64_t flux_box_float(double f) {
    void *mem = flux_gc_alloc_header(sizeof(double), 0, 0xF8 /* FLUX_OBJ_FLOAT */);
    memcpy(mem, &f, sizeof(double));
    return flux_tag_ptr(mem);
}

static inline double flux_unbox_float(int64_t val) {
    double f;
    memcpy(&f, flux_untag_ptr(val), sizeof(double));
    return f;
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
#define FLUX_OBJ_BIGINT   0xF6
#define FLUX_OBJ_EVIDENCE 0xF7
#define FLUX_OBJ_FLOAT    0xF8
#define FLUX_OBJ_THUNK    0xF9

static inline uint8_t flux_obj_tag(void *ptr) {
    /* Read obj_tag from the FluxHeader at ptr - 8.
     * Layout: { i32 refcount, u8 scan_fsize, u8 obj_tag, u16 reserved }
     * obj_tag is at offset 5 within the header. */
    return *((uint8_t *)ptr - 3);
}

/* Thunk detection — thunks are heap pointers with FLUX_OBJ_THUNK tag. */
static inline int flux_is_thunk(int64_t val) {
    return flux_is_ptr(val) && flux_obj_tag(flux_untag_ptr(val)) == FLUX_OBJ_THUNK;
}

/* Thunk tag/untag are the same as pointer tag/untag. */
static inline int64_t flux_tag_thunk(void *ptr) { return flux_tag_ptr(ptr); }
static inline void *flux_untag_thunk(int64_t val) { return flux_untag_ptr(val); }

/* Float detection — floats are heap-boxed pointers with FLUX_OBJ_FLOAT tag. */
static inline int flux_val_is_float(int64_t val) {
    return flux_is_ptr(val) && flux_obj_tag(flux_untag_ptr(val)) == FLUX_OBJ_FLOAT;
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

/* Aether RC: dup/drop for pointer-tagged heap values. */
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
 * FluxString layout (heap-allocated, pointer-tagged):
 *   struct { uint8_t obj_tag, pad[3], uint32_t len, char data[] }
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
int64_t flux_sqrt(int64_t n);
int64_t flux_sin(int64_t n);
int64_t flux_cos(int64_t n);
int64_t flux_exp(int64_t n);
int64_t flux_log(int64_t n);
int64_t flux_floor(int64_t n);
int64_t flux_ceil(int64_t n);
int64_t flux_round(int64_t n);
int64_t flux_tan(int64_t n);
int64_t flux_asin(int64_t n);
int64_t flux_acos(int64_t n);
int64_t flux_atan(int64_t n);
int64_t flux_sinh(int64_t n);
int64_t flux_cosh(int64_t n);
int64_t flux_tanh(int64_t n);
int64_t flux_truncate(int64_t n);

/* ── Runtime-dispatching arithmetic (int/float/string) ──────────────── */

int64_t flux_rt_add(int64_t a, int64_t b);
int64_t flux_rt_sub(int64_t a, int64_t b);
int64_t flux_rt_mul(int64_t a, int64_t b);
int64_t flux_rt_div(int64_t a, int64_t b);
int64_t flux_rt_mod(int64_t a, int64_t b);
int64_t flux_rt_div_loc(int64_t a, int64_t b, int64_t line, int64_t column);
int64_t flux_rt_mod_loc(int64_t a, int64_t b, int64_t line, int64_t column);
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
/* Note: the runtime-callable type inspectors are named flux_rt_is_*
 * to avoid collision with the static inline helpers (flux_is_int, etc.)
 * in this header.  The LLVM emitter maps IsInt → "flux_is_int" but
 * see the builtins table which should remap to these names. */
int64_t flux_is_int_val(int64_t val);
int64_t flux_is_float_val(int64_t val);
int64_t flux_is_string_val(int64_t val);
int64_t flux_is_bool_val(int64_t val);
int64_t flux_is_none_val(int64_t val);

/* ── Control ────────────────────────────────────────────────────────── */

void    flux_panic(int64_t msg);
void    flux_trace_push(const char *name, const char *file, int32_t line);
void    flux_trace_pop(void);
int64_t flux_clock_now(void);
int64_t flux_try(int64_t thunk);
int64_t flux_assert_throws(int64_t thunk, int64_t expected_msg);

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
int64_t flux_call_closure_exact(int64_t closure, int64_t *args, int32_t nargs);

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
int64_t flux_evv_insert(int64_t evv, int64_t htag, int64_t marker, int64_t handler, int64_t state);

/* Yield operations. */
int64_t flux_yield_to(int64_t htag, int64_t optag, int64_t arg, int64_t arity);
int64_t flux_perform_direct(int64_t htag, int64_t optag, int64_t arg, int64_t resume, int64_t arity);
int64_t flux_yield_extend(int64_t cont);
int64_t flux_yield_prompt(int64_t marker, int64_t saved_evv, int64_t body_result);
int64_t flux_compose_conts(void);
int32_t flux_is_yielding(void);

/* Proposal 0162 Phase 3 (partial): short-circuit detection for non-TR
 * handlers on the native backend.  `flux_resume_mark_called` is used as
 * the `resume` closure passed to `flux_perform_direct`: the compiler
 * changes its identity-resume synthesis to call this function (which
 * both marks the flag and returns its argument) instead of the pure
 * identity.  When a clause returns without having invoked resume, the
 * flag is still 0 — flux_perform_direct then emits a structured error
 * instead of silently returning the wrong value. */
extern int32_t flux_resume_called;
int64_t flux_resume_mark_called(int64_t value);

#ifdef __cplusplus
}
#endif

#endif /* FLUX_RT_H */
