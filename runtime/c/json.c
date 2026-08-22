/*
 * json.c — Native JSON parse/stringify helpers for Flow.Json.
 */

#include "flux_rt.h"
#include <ctype.h>
#include <errno.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    int32_t json_null_tag;
    int32_t json_bool_tag;
    int32_t json_number_tag;
    int32_t json_int_tag;
    int32_t json_float_tag;
    int32_t json_string_tag;
    int32_t json_array_tag;
    int32_t json_object_tag;
    int32_t json_error_tag;
    int32_t json_ok_tag;
    int32_t json_err_tag;
} JsonTags;

typedef struct {
    char *data;
    size_t len;
    size_t cap;
} JsonBuf;

typedef struct {
    const char *input;
    size_t len;
    size_t pos;
    JsonTags tags;
    char error[160];
} JsonParser;

static int64_t json_make_adt_scan(int32_t ctor_tag, const int64_t *fields, int32_t count, uint8_t scan) {
    void *mem = flux_gc_alloc_header((uint32_t)(8 + count * 8), scan, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = ctor_tag;
    hdr[1] = count;
    if (count > 0 && fields) {
        memcpy((char *)mem + 8, fields, (size_t)count * sizeof(int64_t));
    }
    return flux_tag_ptr(mem);
}

static int64_t json_make_adt(int32_t ctor_tag, const int64_t *fields, int32_t count) {
    uint8_t scan = count <= 255 ? (uint8_t)count : 255;
    return json_make_adt_scan(ctor_tag, fields, count, scan);
}

static int64_t *json_adt_fields(int64_t value, int32_t *tag_out, int32_t *count_out) {
    if (!flux_is_ptr(value)) return NULL;
    void *ptr = flux_untag_ptr(value);
    if (!ptr || flux_obj_tag(ptr) != FLUX_OBJ_ADT) return NULL;
    int32_t *hdr = (int32_t *)ptr;
    if (tag_out) *tag_out = hdr[0];
    if (count_out) *count_out = hdr[1];
    return (int64_t *)((char *)ptr + 8);
}

static void json_buf_reserve(JsonBuf *buf, size_t extra) {
    if (buf->len + extra <= buf->cap) return;
    size_t next = buf->cap == 0 ? 64 : buf->cap * 2;
    while (next < buf->len + extra) next *= 2;
    char *data = (char *)realloc(buf->data, next);
    if (!data) {
        fprintf(stderr, "json buffer out of memory\n");
        abort();
    }
    buf->data = data;
    buf->cap = next;
}

static void json_buf_byte(JsonBuf *buf, char c) {
    json_buf_reserve(buf, 1);
    buf->data[buf->len++] = c;
}

static void json_buf_mem(JsonBuf *buf, const char *data, size_t len) {
    json_buf_reserve(buf, len);
    memcpy(buf->data + buf->len, data, len);
    buf->len += len;
}

static void json_set_error(JsonParser *p, const char *message) {
    if (p->error[0] == '\0') {
        snprintf(p->error, sizeof(p->error), "%s", message);
    }
}

static void json_skip_ws(JsonParser *p) {
    while (p->pos < p->len) {
        unsigned char c = (unsigned char)p->input[p->pos];
        if (c != ' ' && c != '\n' && c != '\r' && c != '\t') break;
        p->pos++;
    }
}

static int json_hex(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

static int json_parse_u4(JsonParser *p, uint32_t *out) {
    if (p->pos + 4 > p->len) return 0;
    uint32_t v = 0;
    for (int i = 0; i < 4; i++) {
        int h = json_hex(p->input[p->pos + i]);
        if (h < 0) return 0;
        v = (v << 4) | (uint32_t)h;
    }
    p->pos += 4;
    *out = v;
    return 1;
}

static void json_push_utf8(JsonBuf *buf, uint32_t cp) {
    if (cp <= 0x7F) {
        json_buf_byte(buf, (char)cp);
    } else if (cp <= 0x7FF) {
        json_buf_byte(buf, (char)(0xC0 | (cp >> 6)));
        json_buf_byte(buf, (char)(0x80 | (cp & 0x3F)));
    } else if (cp <= 0xFFFF) {
        json_buf_byte(buf, (char)(0xE0 | (cp >> 12)));
        json_buf_byte(buf, (char)(0x80 | ((cp >> 6) & 0x3F)));
        json_buf_byte(buf, (char)(0x80 | (cp & 0x3F)));
    } else {
        json_buf_byte(buf, (char)(0xF0 | (cp >> 18)));
        json_buf_byte(buf, (char)(0x80 | ((cp >> 12) & 0x3F)));
        json_buf_byte(buf, (char)(0x80 | ((cp >> 6) & 0x3F)));
        json_buf_byte(buf, (char)(0x80 | (cp & 0x3F)));
    }
}

static int json_parse_string_raw(JsonParser *p, JsonBuf *out) {
    if (p->pos >= p->len || p->input[p->pos] != '"') {
        json_set_error(p, "expected string");
        return 0;
    }
    p->pos++;
    while (p->pos < p->len) {
        unsigned char c = (unsigned char)p->input[p->pos++];
        if (c == '"') return 1;
        if (c < 0x20) {
            json_set_error(p, "control character in string");
            return 0;
        }
        if (c != '\\') {
            json_buf_byte(out, (char)c);
            continue;
        }
        if (p->pos >= p->len) {
            json_set_error(p, "unterminated escape");
            return 0;
        }
        char e = p->input[p->pos++];
        switch (e) {
            case '"': json_buf_byte(out, '"'); break;
            case '\\': json_buf_byte(out, '\\'); break;
            case '/': json_buf_byte(out, '/'); break;
            case 'b': json_buf_byte(out, '\b'); break;
            case 'f': json_buf_byte(out, '\f'); break;
            case 'n': json_buf_byte(out, '\n'); break;
            case 'r': json_buf_byte(out, '\r'); break;
            case 't': json_buf_byte(out, '\t'); break;
            case 'u': {
                uint32_t cp = 0;
                if (!json_parse_u4(p, &cp)) {
                    json_set_error(p, "invalid unicode escape");
                    return 0;
                }
                if (cp >= 0xD800 && cp <= 0xDBFF) {
                    if (p->pos + 6 > p->len || p->input[p->pos] != '\\' || p->input[p->pos + 1] != 'u') {
                        json_set_error(p, "missing low surrogate");
                        return 0;
                    }
                    p->pos += 2;
                    uint32_t low = 0;
                    if (!json_parse_u4(p, &low) || low < 0xDC00 || low > 0xDFFF) {
                        json_set_error(p, "invalid low surrogate");
                        return 0;
                    }
                    cp = 0x10000 + (((cp - 0xD800) << 10) | (low - 0xDC00));
                } else if (cp >= 0xDC00 && cp <= 0xDFFF) {
                    json_set_error(p, "unexpected low surrogate");
                    return 0;
                }
                json_push_utf8(out, cp);
                break;
            }
            default:
                json_set_error(p, "invalid escape");
                return 0;
        }
    }
    json_set_error(p, "unterminated string");
    return 0;
}

static int64_t json_parse_value(JsonParser *p);

static int64_t json_parse_array(JsonParser *p) {
    p->pos++;
    int64_t *items = NULL;
    size_t len = 0, cap = 0;
    json_skip_ws(p);
    if (p->pos < p->len && p->input[p->pos] == ']') {
        p->pos++;
        int64_t arr = flux_array_new(NULL, 0);
        return json_make_adt(p->tags.json_array_tag, &arr, 1);
    }
    while (p->pos < p->len) {
        int64_t v = json_parse_value(p);
        if (p->error[0] != '\0') {
            free(items);
            return FLUX_NONE;
        }
        if (len == cap) {
            size_t next = cap == 0 ? 8 : cap * 2;
            int64_t *tmp = (int64_t *)realloc(items, next * sizeof(int64_t));
            if (!tmp) abort();
            items = tmp;
            cap = next;
        }
        items[len++] = v;
        json_skip_ws(p);
        if (p->pos < p->len && p->input[p->pos] == ',') {
            p->pos++;
            json_skip_ws(p);
            continue;
        }
        if (p->pos < p->len && p->input[p->pos] == ']') {
            p->pos++;
            int64_t arr = flux_array_new(items, (int32_t)len);
            free(items);
            return json_make_adt(p->tags.json_array_tag, &arr, 1);
        }
        json_set_error(p, "expected ',' or ']'");
        break;
    }
    free(items);
    if (p->error[0] == '\0') json_set_error(p, "unterminated array");
    return FLUX_NONE;
}

static int64_t json_parse_object(JsonParser *p) {
    p->pos++;
    int64_t map = flux_hamt_empty();
    json_skip_ws(p);
    if (p->pos < p->len && p->input[p->pos] == '}') {
        p->pos++;
        return json_make_adt(p->tags.json_object_tag, &map, 1);
    }
    while (p->pos < p->len) {
        JsonBuf key = {0};
        if (!json_parse_string_raw(p, &key)) {
            free(key.data);
            return FLUX_NONE;
        }
        int64_t key_val = flux_string_new(key.data ? key.data : "", (uint32_t)key.len);
        free(key.data);
        json_skip_ws(p);
        if (p->pos >= p->len || p->input[p->pos] != ':') {
            json_set_error(p, "expected ':'");
            return FLUX_NONE;
        }
        p->pos++;
        int64_t value = json_parse_value(p);
        if (p->error[0] != '\0') return FLUX_NONE;
        map = flux_hamt_set(map, key_val, value);
        json_skip_ws(p);
        if (p->pos < p->len && p->input[p->pos] == ',') {
            p->pos++;
            json_skip_ws(p);
            continue;
        }
        if (p->pos < p->len && p->input[p->pos] == '}') {
            p->pos++;
            return json_make_adt(p->tags.json_object_tag, &map, 1);
        }
        json_set_error(p, "expected ',' or '}'");
        return FLUX_NONE;
    }
    json_set_error(p, "unterminated object");
    return FLUX_NONE;
}

static int64_t json_parse_number(JsonParser *p) {
    size_t start = p->pos;
    if (p->input[p->pos] == '-') p->pos++;
    if (p->pos >= p->len) {
        json_set_error(p, "invalid number");
        return FLUX_NONE;
    }
    if (p->input[p->pos] == '0') {
        p->pos++;
        if (p->pos < p->len && isdigit((unsigned char)p->input[p->pos])) {
            json_set_error(p, "invalid leading zero");
            return FLUX_NONE;
        }
    } else if (isdigit((unsigned char)p->input[p->pos])) {
        while (p->pos < p->len && isdigit((unsigned char)p->input[p->pos])) p->pos++;
    } else {
        json_set_error(p, "invalid number");
        return FLUX_NONE;
    }
    if (p->pos < p->len && p->input[p->pos] == '.') {
        p->pos++;
        if (p->pos >= p->len || !isdigit((unsigned char)p->input[p->pos])) {
            json_set_error(p, "invalid fraction");
            return FLUX_NONE;
        }
        while (p->pos < p->len && isdigit((unsigned char)p->input[p->pos])) p->pos++;
    }
    if (p->pos < p->len && (p->input[p->pos] == 'e' || p->input[p->pos] == 'E')) {
        p->pos++;
        if (p->pos < p->len && (p->input[p->pos] == '+' || p->input[p->pos] == '-')) p->pos++;
        if (p->pos >= p->len || !isdigit((unsigned char)p->input[p->pos])) {
            json_set_error(p, "invalid exponent");
            return FLUX_NONE;
        }
        while (p->pos < p->len && isdigit((unsigned char)p->input[p->pos])) p->pos++;
    }
    char tmp[128];
    size_t n = p->pos - start;
    if (n >= sizeof(tmp)) {
        json_set_error(p, "number is too long");
        return FLUX_NONE;
    }
    memcpy(tmp, p->input + start, n);
    tmp[n] = '\0';
    char *end = NULL;
    double d = strtod(tmp, &end);
    if (!end || *end != '\0' || !isfinite(d)) {
        json_set_error(p, "invalid number");
        return FLUX_NONE;
    }
    int integral = 1;
    for (size_t i = 0; i < n; i++) {
        if (tmp[i] == '.' || tmp[i] == 'e' || tmp[i] == 'E') {
            integral = 0;
            break;
        }
    }
    if (integral) {
        errno = 0;
        char *int_end = NULL;
        long long parsed = strtoll(tmp, &int_end, 10);
        if (errno != ERANGE && int_end && *int_end == '\0') {
            int64_t i = flux_tag_int((int64_t)parsed);
            int64_t payload = json_make_adt(p->tags.json_int_tag, &i, 1);
            return json_make_adt(p->tags.json_number_tag, &payload, 1);
        }
    }
    int64_t f = flux_box_float(d);
    int64_t payload = json_make_adt(p->tags.json_float_tag, &f, 1);
    return json_make_adt(p->tags.json_number_tag, &payload, 1);
}

static int64_t json_parse_value(JsonParser *p) {
    json_skip_ws(p);
    if (p->pos >= p->len) {
        json_set_error(p, "expected JSON value");
        return FLUX_NONE;
    }
    char c = p->input[p->pos];
    if (c == 'n' && p->pos + 4 <= p->len && memcmp(p->input + p->pos, "null", 4) == 0) {
        p->pos += 4;
        return json_make_adt_scan(p->tags.json_null_tag, NULL, 0, 0);
    }
    if (c == 't' && p->pos + 4 <= p->len && memcmp(p->input + p->pos, "true", 4) == 0) {
        p->pos += 4;
        int64_t b = FLUX_TRUE;
        return json_make_adt(p->tags.json_bool_tag, &b, 1);
    }
    if (c == 'f' && p->pos + 5 <= p->len && memcmp(p->input + p->pos, "false", 5) == 0) {
        p->pos += 5;
        int64_t b = FLUX_FALSE;
        return json_make_adt(p->tags.json_bool_tag, &b, 1);
    }
    if (c == '"') {
        JsonBuf s = {0};
        if (!json_parse_string_raw(p, &s)) {
            free(s.data);
            return FLUX_NONE;
        }
        int64_t str = flux_string_new(s.data ? s.data : "", (uint32_t)s.len);
        free(s.data);
        return json_make_adt(p->tags.json_string_tag, &str, 1);
    }
    if (c == '[') return json_parse_array(p);
    if (c == '{') return json_parse_object(p);
    if (c == '-' || isdigit((unsigned char)c)) return json_parse_number(p);
    json_set_error(p, "expected JSON value");
    return FLUX_NONE;
}

static int64_t json_ok(JsonTags tags, int64_t value) {
    return json_make_adt_scan(tags.json_ok_tag, &value, 1, 0);
}

static int64_t json_err(JsonTags tags, const char *message) {
    int64_t fields[2] = { flux_string_new("$", 1), flux_string_new(message, (uint32_t)strlen(message)) };
    int64_t err = json_make_adt(tags.json_error_tag, fields, 2);
    return json_make_adt_scan(tags.json_err_tag, &err, 1, 0);
}

int64_t flux_json_parse(
    int32_t json_null_tag,
    int32_t json_bool_tag,
    int32_t json_number_tag,
    int32_t json_int_tag,
    int32_t json_float_tag,
    int32_t json_string_tag,
    int32_t json_array_tag,
    int32_t json_object_tag,
    int32_t json_error_tag,
    int32_t json_ok_tag,
    int32_t json_err_tag,
    int64_t raw_val
) {
    JsonTags tags = {
        json_null_tag, json_bool_tag, json_number_tag, json_int_tag, json_float_tag,
        json_string_tag, json_array_tag, json_object_tag, json_error_tag, json_ok_tag,
        json_err_tag
    };
    JsonParser p = {
        flux_string_data(raw_val),
        (size_t)flux_string_len(raw_val),
        0,
        tags,
        {0}
    };
    int64_t value = json_parse_value(&p);
    if (p.error[0] == '\0') {
        json_skip_ws(&p);
        if (p.pos != p.len) json_set_error(&p, "trailing characters after JSON value");
    }
    if (p.error[0] != '\0') return json_err(tags, p.error);
    return json_ok(tags, value);
}

static void json_stringify_value(JsonBuf *buf, JsonTags tags, int64_t value);

static void json_escape_string(JsonBuf *buf, const char *data, size_t len) {
    json_buf_byte(buf, '"');
    for (size_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)data[i];
        switch (c) {
            case '"': json_buf_mem(buf, "\\\"", 2); break;
            case '\\': json_buf_mem(buf, "\\\\", 2); break;
            case '\b': json_buf_mem(buf, "\\b", 2); break;
            case '\f': json_buf_mem(buf, "\\f", 2); break;
            case '\n': json_buf_mem(buf, "\\n", 2); break;
            case '\r': json_buf_mem(buf, "\\r", 2); break;
            case '\t': json_buf_mem(buf, "\\t", 2); break;
            default:
                if (c < 0x20) {
                    char tmp[7];
                    snprintf(tmp, sizeof(tmp), "\\u%04x", (unsigned)c);
                    json_buf_mem(buf, tmp, 6);
                } else {
                    json_buf_byte(buf, (char)c);
                }
                break;
        }
    }
    json_buf_byte(buf, '"');
}

static int json_key_cmp(const void *a, const void *b) {
    int64_t av = *(const int64_t *)a;
    int64_t bv = *(const int64_t *)b;
    const char *as = flux_string_data(av);
    const char *bs = flux_string_data(bv);
    uint32_t al = flux_string_len(av);
    uint32_t bl = flux_string_len(bv);
    int c = memcmp(as, bs, al < bl ? al : bl);
    if (c != 0) return c;
    return (al > bl) - (al < bl);
}

static void json_stringify_value(JsonBuf *buf, JsonTags tags, int64_t value) {
    int32_t tag = 0, count = 0;
    int64_t *fields = json_adt_fields(value, &tag, &count);
    if (tag == tags.json_null_tag) {
        json_buf_mem(buf, "null", 4);
    } else if (tag == tags.json_bool_tag && fields && count >= 1) {
        json_buf_mem(buf, fields[0] == FLUX_TRUE ? "true" : "false", fields[0] == FLUX_TRUE ? 4 : 5);
    } else if (tag == tags.json_number_tag && fields && count >= 1) {
        char tmp[64];
        int32_t payload_tag = 0, payload_count = 0;
        int64_t *payload_fields = json_adt_fields(fields[0], &payload_tag, &payload_count);
        if (payload_tag == tags.json_int_tag && payload_fields && payload_count >= 1 && flux_is_int(payload_fields[0])) {
            snprintf(tmp, sizeof(tmp), "%lld", (long long)flux_untag_int(payload_fields[0]));
        } else if (payload_tag == tags.json_float_tag && payload_fields && payload_count >= 1 && flux_val_is_float(payload_fields[0])) {
            snprintf(tmp, sizeof(tmp), "%.17g", flux_unbox_float(payload_fields[0]));
        } else if (flux_val_is_float(fields[0])) {
            snprintf(tmp, sizeof(tmp), "%.17g", flux_unbox_float(fields[0]));
        } else if (flux_is_int(fields[0])) {
            snprintf(tmp, sizeof(tmp), "%lld", (long long)flux_untag_int(fields[0]));
        } else {
            snprintf(tmp, sizeof(tmp), "0");
        }
        json_buf_mem(buf, tmp, strlen(tmp));
    } else if (tag == tags.json_string_tag && fields && count >= 1) {
        json_escape_string(buf, flux_string_data(fields[0]), (size_t)flux_string_len(fields[0]));
    } else if (tag == tags.json_array_tag && fields && count >= 1) {
        int64_t arr = fields[0];
        int64_t len_val = flux_array_len(arr);
        int64_t n = flux_is_int(len_val) ? flux_untag_int(len_val) : 0;
        json_buf_byte(buf, '[');
        for (int64_t i = 0; i < n; i++) {
            if (i > 0) json_buf_byte(buf, ',');
            json_stringify_value(buf, tags, flux_array_at(arr, flux_tag_int(i)));
        }
        json_buf_byte(buf, ']');
    } else if (tag == tags.json_object_tag && fields && count >= 1) {
        int64_t map = fields[0];
        int64_t keys = flux_hamt_keys(map);
        int64_t len_val = flux_array_len(keys);
        int64_t n = flux_is_int(len_val) ? flux_untag_int(len_val) : 0;
        int64_t *sorted = n > 0 ? (int64_t *)malloc((size_t)n * sizeof(int64_t)) : NULL;
        for (int64_t i = 0; i < n; i++) sorted[i] = flux_array_at(keys, flux_tag_int(i));
        qsort(sorted, (size_t)n, sizeof(int64_t), json_key_cmp);
        json_buf_byte(buf, '{');
        for (int64_t i = 0; i < n; i++) {
            if (i > 0) json_buf_byte(buf, ',');
            int64_t key = sorted[i];
            json_escape_string(buf, flux_string_data(key), (size_t)flux_string_len(key));
            json_buf_byte(buf, ':');
            json_stringify_value(buf, tags, flux_hamt_get(map, key));
        }
        free(sorted);
        json_buf_byte(buf, '}');
    } else {
        json_buf_mem(buf, "null", 4);
    }
}

int64_t flux_json_stringify(
    int32_t json_null_tag,
    int32_t json_bool_tag,
    int32_t json_number_tag,
    int32_t json_int_tag,
    int32_t json_float_tag,
    int32_t json_string_tag,
    int32_t json_array_tag,
    int32_t json_object_tag,
    int64_t value
) {
    JsonTags tags = {
        json_null_tag, json_bool_tag, json_number_tag, json_int_tag, json_float_tag,
        json_string_tag, json_array_tag, json_object_tag, 0, 0, 0
    };
    JsonBuf buf = {0};
    json_stringify_value(&buf, tags, value);
    int64_t out = flux_string_new(buf.data ? buf.data : "", (uint32_t)buf.len);
    free(buf.data);
    return out;
}
