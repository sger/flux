/*
 * tape.c — the C half of the effect tape (proposal 0188 S1).
 *
 * While a tape is active, each synchronous effect primop describes its
 * request with flux_tape_start, then records its real result with
 * flux_tape_record_value or, when replaying, builds the recorded result with
 * flux_tape_replay_value. This file takes native values apart into the flat
 * FluxTapeOutcome the Rust side stores, and builds them back, by the shape
 * each primop returns. The tape, its checks and its errors are all in Rust.
 */

#include "flux_rt.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int32_t flux_tape_mode = 0;

/* A process that exits without reaching flux_rt_shutdown (a panic already
 * finishes the tape itself) still ends its tape, as a failed run. */
static void flux_tape_atexit(void) {
    static const char msg[] = "the program exited before finishing";
    flux_tape_finish(1, (const uint8_t *)msg, sizeof(msg) - 1);
}

void flux_tape_c_init(void) {
    flux_tape_mode = flux_tape_init();
    if (flux_tape_mode) {
        atexit(flux_tape_atexit);
    }
}

/* ── Requests ─────────────────────────────────────────────────────────── */

static void tape_arg_string(int64_t s) {
    flux_tape_arg((const uint8_t *)flux_string_data(s), flux_string_len(s));
}

int flux_tape_start(uint32_t kind, int64_t arg0, int64_t arg1) {
    flux_tape_begin(kind);
    if (arg0) tape_arg_string(arg0);
    if (arg1) tape_arg_string(arg1);
    return flux_tape_mode == 2;
}

/* Native arrays: { obj_tag, pad[3], u32 len, u32 capacity, pad, i64 elements[] }. */
static uint32_t tape_array_len(int64_t arr) {
    if (!flux_is_ptr(arr)) return 0;
    return *(uint32_t *)((char *)flux_untag_ptr(arr) + 4);
}

static int64_t tape_array_at(int64_t arr, uint32_t i) {
    return ((int64_t *)((char *)flux_untag_ptr(arr) + 16))[i];
}

int flux_tape_start_proc(int64_t cmd, int64_t argv) {
    flux_tape_begin(FLUX_TAPE_EV_PROC_RUN);
    tape_arg_string(cmd);
    uint32_t n = tape_array_len(argv);
    for (uint32_t i = 0; i < n; i++) {
        tape_arg_string(tape_array_at(argv, i));
    }
    return flux_tape_mode == 2;
}

/* ── Native values to outcomes ────────────────────────────────────────── */

/* ADTs: { i32 ctor_tag, i32 field_count, i64 fields[] }. */
static int32_t adt_tag(int64_t v) {
    return *(int32_t *)flux_untag_ptr(v);
}

static int64_t adt_field(int64_t v, int i) {
    return ((int64_t *)((char *)flux_untag_ptr(v) + 8))[i];
}

static void put_s0(FluxTapeOutcome *o, int64_t s) {
    o->s0 = (const uint8_t *)flux_string_data(s);
    o->n0 = flux_string_len(s);
}

static void put_s1(FluxTapeOutcome *o, int64_t s) {
    o->s1 = (const uint8_t *)flux_string_data(s);
    o->n1 = flux_string_len(s);
}

static void push_strings(FluxTapeOutcome *o, int64_t arr) {
    uint32_t n = tape_array_len(arr);
    for (uint32_t i = 0; i < n; i++) {
        int64_t item = tape_array_at(arr, i);
        flux_tape_list_push((const uint8_t *)flux_string_data(item), flux_string_len(item));
    }
    o->tag = FLUX_TAPE_OUT_LIST;
    o->list_len = n;
}

/* The IoError kinds in Flow.IoError declaration order, as this compilation
 * tagged them. */
static void io_kind_tags(const FluxIoTags *t, int32_t out[8]) {
    out[0] = t->not_found_tag;
    out[1] = t->permission_denied_tag;
    out[2] = t->already_exists_tag;
    out[3] = t->not_a_directory_tag;
    out[4] = t->is_a_directory_tag;
    out[5] = t->directory_not_empty_tag;
    out[6] = t->interrupted_tag;
    out[7] = t->other_tag;
}

/* The payload of an Ok, by the shape of the Result it came from. */
static void describe_ok(int64_t v, int shape, FluxTapeOutcome *o) {
    switch (shape) {
    case FLUX_TAPE_SH_RES_TEXT:
        o->tag = FLUX_TAPE_OUT_TEXT;
        put_s0(o, v);
        break;
    case FLUX_TAPE_SH_RES_LIST:
        push_strings(o, v);
        break;
    case FLUX_TAPE_SH_RES_META:
        o->tag = FLUX_TAPE_OUT_META;
        o->i0 = flux_untag_int(adt_field(v, 0));
        o->i1 = flux_untag_int(adt_field(v, 1));
        o->b0 = adt_field(v, 2) == FLUX_TRUE;
        o->b1 = adt_field(v, 3) == FLUX_TRUE;
        break;
    case FLUX_TAPE_SH_RES_PROC:
        o->tag = FLUX_TAPE_OUT_PROC;
        o->i0 = flux_untag_int(adt_field(v, 0));
        put_s0(o, adt_field(v, 1));
        put_s1(o, adt_field(v, 2));
        break;
    default:
        o->tag = FLUX_TAPE_OUT_UNIT;
        break;
    }
}

static void describe(int64_t v, int shape, const FluxIoTags *t, FluxTapeOutcome *o) {
    memset(o, 0, sizeof(*o));
    switch (shape) {
    case FLUX_TAPE_SH_TEXT:
        o->tag = FLUX_TAPE_OUT_TEXT;
        put_s0(o, v);
        return;
    case FLUX_TAPE_SH_BOOL:
        o->tag = FLUX_TAPE_OUT_BOOL;
        o->b0 = v == FLUX_TRUE;
        return;
    case FLUX_TAPE_SH_INT:
        o->tag = FLUX_TAPE_OUT_INT;
        o->i0 = flux_untag_int(v);
        return;
    case FLUX_TAPE_SH_OPT_TEXT:
        if (v == FLUX_NONE) {
            o->tag = FLUX_TAPE_OUT_NONE;
        } else {
            o->tag = FLUX_TAPE_OUT_SOME;
            put_s0(o, adt_field(v, 0));
        }
        return;
    case FLUX_TAPE_SH_LIST_TEXT:
        push_strings(o, v);
        return;
    case FLUX_TAPE_SH_WRITE:
        /* The legacy write_file answers a Bool; the VM's answers unit or an
         * error. The tape keeps the VM's form so either backend replays it. */
        if (v == FLUX_TRUE) {
            o->tag = FLUX_TAPE_OUT_UNIT;
        } else {
            static const char msg[] = "write_file failed";
            o->tag = FLUX_TAPE_OUT_FAIL;
            o->s0 = (const uint8_t *)msg;
            o->n0 = sizeof(msg) - 1;
        }
        return;
    default:
        break;
    }

    /* Result<_, IoError> */
    if (adt_tag(v) == t->ok_tag) {
        o->ok = 1;
        describe_ok(adt_field(v, 0), shape, o);
        return;
    }
    int64_t io_err = adt_field(v, 0);
    int32_t kinds[8];
    io_kind_tags(t, kinds);
    int32_t kind_tag = adt_tag(adt_field(io_err, 0));
    o->tag = FLUX_TAPE_OUT_ERR;
    o->err_kind = 7; /* Other */
    for (int i = 0; i < 8; i++) {
        if (kinds[i] == kind_tag) {
            o->err_kind = i;
            break;
        }
    }
    put_s0(o, adt_field(io_err, 1));
    put_s1(o, adt_field(io_err, 2));
}

int64_t flux_tape_record_value(int64_t value, int shape, const FluxIoTags *tags,
                               int32_t extra_tag) {
    (void)extra_tag;
    FluxTapeOutcome o;
    describe(value, shape, tags, &o);
    flux_tape_record(&o);
    return value;
}

/* ── Outcomes to native values ────────────────────────────────────────── */

static int64_t string_of(const uint8_t *p, size_t n) {
    return flux_string_new((const char *)p, (uint32_t)n);
}

static int64_t replayed_strings(const FluxTapeOutcome *o) {
    size_t n = o->list_len;
    int64_t *items = (int64_t *)malloc((n ? n : 1) * sizeof(int64_t));
    if (!items) return flux_array_new(NULL, 0);
    for (size_t i = 0; i < n; i++) {
        size_t len = 0;
        const uint8_t *p = flux_tape_list_item(i, &len);
        items[i] = string_of(p, len);
    }
    int64_t arr = flux_array_new(items, (int32_t)n);
    free(items);
    return arr;
}

static int64_t rebuild_ok(const FluxTapeOutcome *o, int32_t extra_tag) {
    switch (o->tag) {
    case FLUX_TAPE_OUT_TEXT:
        return string_of(o->s0, o->n0);
    case FLUX_TAPE_OUT_LIST:
        return replayed_strings(o);
    case FLUX_TAPE_OUT_META: {
        int64_t fields[4];
        fields[0] = flux_tag_int(o->i0);
        fields[1] = flux_tag_int(o->i1);
        fields[2] = flux_make_bool(o->b0);
        fields[3] = flux_make_bool(o->b1);
        return flux_io_make_adt(extra_tag, fields, 4);
    }
    case FLUX_TAPE_OUT_PROC: {
        int64_t fields[3];
        fields[0] = flux_tag_int(o->i0);
        fields[1] = string_of(o->s0, o->n0);
        fields[2] = string_of(o->s1, o->n1);
        return flux_io_make_adt(extra_tag, fields, 3);
    }
    default:
        return FLUX_NONE; /* unit */
    }
}

int64_t flux_tape_replay_value(int shape, const FluxIoTags *t, int32_t extra_tag) {
    FluxTapeOutcome o;
    flux_tape_replay(&o);

    if (o.tag == FLUX_TAPE_OUT_FAIL) {
        /* The recorded run failed here with a runtime error. */
        if (shape == FLUX_TAPE_SH_WRITE) return FLUX_FALSE;
        flux_panic(string_of(o.s0, o.n0));
    }
    switch (shape) {
    case FLUX_TAPE_SH_TEXT:
        return string_of(o.s0, o.n0);
    case FLUX_TAPE_SH_BOOL:
        return flux_make_bool(o.b0);
    case FLUX_TAPE_SH_INT:
        return flux_tag_int(o.i0);
    case FLUX_TAPE_SH_OPT_TEXT:
        if (o.tag == FLUX_TAPE_OUT_SOME) return flux_wrap_some(string_of(o.s0, o.n0));
        return flux_make_none();
    case FLUX_TAPE_SH_LIST_TEXT:
        return replayed_strings(&o);
    case FLUX_TAPE_SH_WRITE:
        return FLUX_TRUE;
    default:
        break;
    }

    /* Result<_, IoError> */
    if (o.ok) {
        int64_t payload = rebuild_ok(&o, extra_tag);
        return flux_io_make_adt(t->ok_tag, &payload, 1);
    }
    int32_t kinds[8];
    io_kind_tags(t, kinds);
    int32_t kind = (o.err_kind >= 0 && o.err_kind < 8) ? o.err_kind : 7;
    int64_t fields[3];
    fields[0] = flux_io_make_adt(kinds[kind], NULL, 0);
    fields[1] = string_of(o.s0, o.n0);
    fields[2] = string_of(o.s1, o.n1);
    int64_t io_err = flux_io_make_adt(t->io_error_tag, fields, 3);
    return flux_io_make_adt(t->err_tag, &io_err, 1);
}
