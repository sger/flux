/*
 * tcp.c — Fiber-suspending TCP primitives for proposal 0174 Phase 1b-vii.
 *
 * LLVM/native code calls these symbols for Flow.Tcp primops. The actual
 * socket state machines live in Rust's MioBackend; these wrappers decode Flux
 * values, submit an async request, and suspend the current fiber until the
 * native async scheduler resumes it.
 */

#include "flux_rt.h"
#include <stdatomic.h>
#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    uint64_t listener;
    uint64_t scope;
    uint64_t *active;
    size_t active_len;
    size_t active_cap;
    int shutting_down;
    int listener_closed;
    int stopped;
    size_t max_connections;
    size_t max_header_bytes;
    size_t max_body_bytes;
    size_t request_timeout_ms;
} HttpServerState;

static HttpServerState *http_servers = NULL;
static size_t http_server_len = 0;
static size_t http_server_cap = 0;
static int64_t http_next_server_id = 1;
static atomic_flag http_registry_lock = ATOMIC_FLAG_INIT;

static void http_lock(void) {
    while (atomic_flag_test_and_set_explicit(&http_registry_lock, memory_order_acquire)) {
    }
}

static void http_unlock(void) {
    atomic_flag_clear_explicit(&http_registry_lock, memory_order_release);
}

static int64_t *http_adt_fields(int64_t value, int32_t *tag_out, int32_t *count_out) {
    if (!flux_is_ptr(value)) return NULL;
    void *ptr = flux_untag_ptr(value);
    if (!ptr || flux_obj_tag(ptr) != FLUX_OBJ_ADT) return NULL;
    int32_t *hdr = (int32_t *)ptr;
    if (tag_out) *tag_out = hdr[0];
    if (count_out) *count_out = hdr[1];
    return (int64_t *)((char *)ptr + 8);
}

static int64_t http_make_adt_scan(
    int32_t ctor_tag,
    const int64_t *fields,
    int32_t count,
    uint8_t scan
) {
    void *mem = flux_gc_alloc_header((uint32_t)(8 + count * 8), scan, FLUX_OBJ_ADT);
    int32_t *hdr = (int32_t *)mem;
    hdr[0] = ctor_tag;
    hdr[1] = count;
    if (count > 0 && fields) {
        memcpy((char *)mem + 8, fields, (size_t)count * sizeof(int64_t));
    }
    return flux_tag_ptr(mem);
}

static int64_t http_make_adt(int32_t ctor_tag, const int64_t *fields, int32_t count) {
    uint8_t scan = count <= 255 ? (uint8_t)count : 255;
    return http_make_adt_scan(ctor_tag, fields, count, scan);
}

static HttpServerState *http_find_server(int64_t server_id) {
    if (server_id <= 0) return NULL;
    size_t index = (size_t)(server_id - 1);
    if (index >= http_server_len) return NULL;
    return &http_servers[index];
}

static int64_t http_config_int_field(int64_t config, int index, int64_t fallback) {
    int32_t count = 0;
    int64_t *fields = http_adt_fields(config, NULL, &count);
    if (!fields) return fallback;
    if (count == 1) {
        int32_t inner_count = 0;
        int64_t *inner = http_adt_fields(fields[0], NULL, &inner_count);
        if (inner && inner_count >= 5) {
            fields = inner;
            count = inner_count;
        }
    }
    if (index >= count || !flux_is_int(fields[index])) return fallback;
    int64_t value = flux_untag_int(fields[index]);
    return value >= 0 ? value : fallback;
}

static void http_active_add(HttpServerState *state, uint64_t conn) {
    if (!state || state->shutting_down) return;
    for (size_t i = 0; i < state->active_len; i++) {
        if (state->active[i] == conn) return;
    }
    if (state->active_len == state->active_cap) {
        size_t next_cap = state->active_cap == 0 ? 8 : state->active_cap * 2;
        uint64_t *next = (uint64_t *)realloc(state->active, next_cap * sizeof(uint64_t));
        if (!next) {
            fprintf(stderr, "flux_http_register_connection: out of memory\n");
            abort();
        }
        state->active = next;
        state->active_cap = next_cap;
    }
    state->active[state->active_len++] = conn;
}

static void http_active_remove(HttpServerState *state, uint64_t conn) {
    if (!state) return;
    for (size_t i = 0; i < state->active_len; i++) {
        if (state->active[i] == conn) {
            state->active[i] = state->active[state->active_len - 1];
            state->active_len--;
            return;
        }
    }
}

static void http_close_listener(HttpServerState *state) {
    if (!state || state->listener_closed) return;
    state->listener_closed = 1;
    (void)flux_async_tcp_close(state->listener);
}

static size_t http_find_bytes(const char *data, size_t len, const char *needle, size_t nlen) {
    if (nlen == 0 || len < nlen) return (size_t)-1;
    for (size_t i = 0; i <= len - nlen; i++) {
        if (memcmp(data + i, needle, nlen) == 0) return i;
    }
    return (size_t)-1;
}

static int http_ascii_eq_ci(const char *a, size_t alen, const char *b) {
    size_t blen = strlen(b);
    if (alen != blen) return 0;
    for (size_t i = 0; i < alen; i++) {
        if (tolower((unsigned char)a[i]) != tolower((unsigned char)b[i])) return 0;
    }
    return 1;
}

static int64_t http_parse_failure(int32_t tag, int status, const char *message) {
    int64_t fields[2] = {
        flux_tag_int(status),
        flux_string_new(message, (uint32_t)strlen(message)),
    };
    return http_make_adt_scan(tag, fields, 2, 0);
}

static int64_t http_method_value(const char *method, size_t len, const int32_t *method_tags) {
    int index = 0;
    if (http_ascii_eq_ci(method, len, "POST")) index = 1;
    else if (http_ascii_eq_ci(method, len, "PUT")) index = 2;
    else if (http_ascii_eq_ci(method, len, "DELETE")) index = 3;
    else if (http_ascii_eq_ci(method, len, "PATCH")) index = 4;
    else if (http_ascii_eq_ci(method, len, "HEAD")) index = 5;
    else if (http_ascii_eq_ci(method, len, "OPTIONS")) index = 6;
    return http_make_adt(method_tags[index], NULL, 0);
}

static int64_t http_request_value(
    int32_t request_tag,
    const int32_t *method_tags,
    const char *method,
    size_t method_len,
    const char *target,
    size_t target_len,
    const char *body,
    size_t body_len
) {
    int64_t fields[4] = {
        http_method_value(method, method_len, method_tags),
        flux_string_new(target, (uint32_t)target_len),
        flux_hamt_empty(),
        flux_string_new(body, (uint32_t)body_len),
    };
    return http_make_adt(request_tag, fields, 4);
}

static const char *http_reason(int64_t status) {
    switch (status) {
        case 200: return "OK";
        case 201: return "Created";
        case 202: return "Accepted";
        case 204: return "No Content";
        case 400: return "Bad Request";
        case 404: return "Not Found";
        case 413: return "Payload Too Large";
        case 500: return "Internal Server Error";
        case 504: return "Gateway Timeout";
        default: return "OK";
    }
}

typedef struct {
    char *data;
    size_t len;
    size_t cap;
} HttpBuffer;

static void http_buf_append(HttpBuffer *buf, const char *data, size_t len) {
    if (len == 0) return;
    if (buf->len + len + 1 > buf->cap) {
        size_t next_cap = buf->cap == 0 ? 256 : buf->cap;
        while (next_cap < buf->len + len + 1) next_cap *= 2;
        char *next = (char *)realloc(buf->data, next_cap);
        if (!next) {
            fprintf(stderr, "http buffer append: out of memory\n");
            abort();
        }
        buf->data = next;
        buf->cap = next_cap;
    }
    memcpy(buf->data + buf->len, data, len);
    buf->len += len;
    buf->data[buf->len] = '\0';
}

static void http_buf_append_cstr(HttpBuffer *buf, const char *data) {
    http_buf_append(buf, data, strlen(data));
}

static void http_buf_append_value_string(HttpBuffer *buf, int64_t value) {
    http_buf_append(buf, flux_string_data(value), (size_t)flux_string_len(value));
}

static int http_header_name_eq(int64_t key_val, const char *expected) {
    const char *key = flux_string_data(key_val);
    uint32_t key_len = flux_string_len(key_val);
    return http_ascii_eq_ci(key, key_len, expected);
}

static const char *http_method_name_from_value(
    int64_t method_val,
    const int32_t *method_tags
) {
    int32_t tag = 0;
    int32_t count = 0;
    (void)http_adt_fields(method_val, &tag, &count);
    if (tag == method_tags[1]) return "POST";
    if (tag == method_tags[2]) return "PUT";
    if (tag == method_tags[3]) return "DELETE";
    if (tag == method_tags[4]) return "PATCH";
    if (tag == method_tags[5]) return "HEAD";
    if (tag == method_tags[6]) return "OPTIONS";
    return "GET";
}

static int64_t http_url_failure(int32_t tag, const char *message) {
    int64_t fields[2] = {
        flux_tag_int(0),
        flux_string_new(message, (uint32_t)strlen(message)),
    };
    return http_make_adt_scan(tag, fields, 2, 0);
}

int64_t flux_http_parse_url(int32_t parsed_tag, int32_t failure_tag, int64_t url_val) {
    const char *url = flux_string_data(url_val);
    size_t len = (size_t)flux_string_len(url_val);
    const char *prefix = "http://";
    size_t prefix_len = strlen(prefix);
    if (len < prefix_len || memcmp(url, prefix, prefix_len) != 0) {
        return http_url_failure(failure_tag, "only http:// URLs are supported in this phase");
    }

    const char *rest = url + prefix_len;
    size_t rest_len = len - prefix_len;
    size_t split = rest_len;
    for (size_t i = 0; i < rest_len; i++) {
        if (rest[i] == '/' || rest[i] == '?') {
            split = i;
            break;
        }
    }
    if (split == 0) return http_url_failure(failure_tag, "HTTP URL missing host");

    const char *authority = rest;
    size_t authority_len = split;
    const char *target = "/";
    size_t target_len = 1;
    char *owned_target = NULL;
    if (split < rest_len) {
        if (rest[split] == '?') {
            target_len = rest_len - split + 1;
            owned_target = (char *)malloc(target_len + 1);
            if (!owned_target) {
                fprintf(stderr, "flux_http_parse_url: out of memory\n");
                abort();
            }
            owned_target[0] = '/';
            memcpy(owned_target + 1, rest + split, rest_len - split);
            owned_target[target_len] = '\0';
            target = owned_target;
        } else {
            target = rest + split;
            target_len = rest_len - split;
        }
    }

    size_t host_len = authority_len;
    int64_t port = 80;
    for (size_t i = authority_len; i > 0; i--) {
        if (authority[i - 1] == ':') {
            host_len = i - 1;
            if (host_len == 0 || i >= authority_len) {
                free(owned_target);
                return http_url_failure(failure_tag, "HTTP URL has invalid port");
            }
            char port_buf[16];
            size_t port_len = authority_len - i;
            if (port_len == 0 || port_len >= sizeof(port_buf)) {
                free(owned_target);
                return http_url_failure(failure_tag, "HTTP URL has invalid port");
            }
            memcpy(port_buf, authority + i, port_len);
            port_buf[port_len] = '\0';
            char *endptr = NULL;
            long parsed = strtol(port_buf, &endptr, 10);
            if (!endptr || *endptr != '\0' || parsed <= 0 || parsed > 65535) {
                free(owned_target);
                return http_url_failure(failure_tag, "HTTP URL has invalid port");
            }
            port = parsed;
            break;
        }
    }
    if (host_len == 0) {
        free(owned_target);
        return http_url_failure(failure_tag, "HTTP URL missing host");
    }

    int64_t fields[3] = {
        flux_string_new(authority, (uint32_t)host_len),
        flux_tag_int(port),
        flux_string_new(target, (uint32_t)target_len),
    };
    free(owned_target);
    return http_make_adt_scan(parsed_tag, fields, 3, 0);
}

int64_t flux_http_write_request(
    int32_t get_tag,
    int32_t post_tag,
    int32_t put_tag,
    int32_t delete_tag,
    int32_t patch_tag,
    int32_t head_tag,
    int32_t options_tag,
    int64_t method_val,
    int64_t host_val,
    int64_t target_val,
    int64_t headers_val,
    int64_t body_val
) {
    int32_t method_tags[7] = {
        get_tag, post_tag, put_tag, delete_tag, patch_tag, head_tag, options_tag
    };
    const char *method = http_method_name_from_value(method_val, method_tags);
    HttpBuffer buf = {0};
    http_buf_append_cstr(&buf, method);
    http_buf_append_cstr(&buf, " ");
    http_buf_append_value_string(&buf, target_val);
    http_buf_append_cstr(&buf, " HTTP/1.1\r\n");

    int has_host = 0;
    int has_connection = 0;
    int has_content_length = 0;
    int64_t keys = flux_hamt_keys(headers_val);
    int64_t key_count_val = flux_array_len(keys);
    int64_t key_count = flux_untag_int(key_count_val);
    for (int64_t i = 0; i < key_count; i++) {
        int64_t key = flux_array_get(keys, flux_tag_int(i));
        int64_t value = flux_hamt_get(headers_val, key);
        if (http_header_name_eq(key, "Host")) has_host = 1;
        else if (http_header_name_eq(key, "Connection")) has_connection = 1;
        else if (http_header_name_eq(key, "Content-Length")) has_content_length = 1;
        http_buf_append_value_string(&buf, key);
        http_buf_append_cstr(&buf, ": ");
        http_buf_append_value_string(&buf, value);
        http_buf_append_cstr(&buf, "\r\n");
    }
    if (!has_host) {
        http_buf_append_cstr(&buf, "Host: ");
        http_buf_append_value_string(&buf, host_val);
        http_buf_append_cstr(&buf, "\r\n");
    }
    if (!has_connection) {
        http_buf_append_cstr(&buf, "Connection: close\r\n");
    }
    if (!has_content_length) {
        char len_buf[64];
        snprintf(len_buf, sizeof(len_buf), "Content-Length: %u\r\n", flux_string_len(body_val));
        http_buf_append_cstr(&buf, len_buf);
    }
    http_buf_append_cstr(&buf, "\r\n");
    http_buf_append_value_string(&buf, body_val);
    int64_t result = flux_string_new(buf.data ? buf.data : "", (uint32_t)buf.len);
    free(buf.data);
    return result;
}

static int64_t http_response_value(
    int32_t response_tag,
    int status,
    const char *body,
    size_t body_len
) {
    int64_t fields[3] = {
        flux_tag_int(status),
        flux_hamt_empty(),
        flux_string_new(body, (uint32_t)body_len),
    };
    return http_make_adt(response_tag, fields, 3);
}

static int64_t http_response_parse_failure(int32_t tag, int status, const char *message) {
    int64_t fields[2] = {
        flux_tag_int(status),
        flux_string_new(message, (uint32_t)strlen(message)),
    };
    return http_make_adt_scan(tag, fields, 2, 0);
}

int64_t flux_http_parse_response(
    int32_t need_more_tag,
    int32_t parsed_tag,
    int32_t failure_tag,
    int32_t response_tag,
    int64_t raw_val
) {
    const char *raw = flux_string_data(raw_val);
    size_t raw_len = (size_t)flux_string_len(raw_val);
    size_t max_header = 65536;
    size_t max_body = 8388608;

    size_t header_end = http_find_bytes(raw, raw_len, "\r\n\r\n", 4);
    if (header_end == (size_t)-1) {
        if (raw_len > max_header) {
            return http_response_parse_failure(failure_tag, 413, "HTTP response header block exceeds max_header_bytes");
        }
        return http_make_adt_scan(need_more_tag, NULL, 0, 0);
    }
    if (header_end > max_header) {
        return http_response_parse_failure(failure_tag, 413, "HTTP response header block exceeds max_header_bytes");
    }

    size_t line_end = http_find_bytes(raw, header_end, "\r\n", 2);
    if (line_end == (size_t)-1) {
        return http_response_parse_failure(failure_tag, 0, "missing status line");
    }
    if (line_end < 12 || memcmp(raw, "HTTP/1.1 ", 9) != 0) {
        return http_response_parse_failure(failure_tag, 0, "unsupported HTTP response version");
    }
    char status_buf[4];
    memcpy(status_buf, raw + 9, 3);
    status_buf[3] = '\0';
    char *status_end = NULL;
    long status = strtol(status_buf, &status_end, 10);
    if (!status_end || *status_end != '\0') {
        return http_response_parse_failure(failure_tag, 0, "invalid HTTP response status");
    }

    long long content_length = -1;
    int chunked = 0;
    size_t pos = line_end + 2;
    while (pos < header_end) {
        size_t end = pos;
        while (end < header_end && !(raw[end] == '\r' && end + 1 < header_end && raw[end + 1] == '\n')) {
            end++;
        }
        if (end == pos) break;
        if (raw[pos] == ' ' || raw[pos] == '\t') {
            return http_response_parse_failure(failure_tag, 0, "obsolete folded HTTP headers are rejected");
        }
        const char *colon = memchr(raw + pos, ':', end - pos);
        if (!colon) return http_response_parse_failure(failure_tag, 0, "HTTP header missing ':'");
        size_t name_len = (size_t)(colon - (raw + pos));
        const char *value = colon + 1;
        const char *value_end = raw + end;
        while (value < value_end && (*value == ' ' || *value == '\t')) value++;
        while (value_end > value && (value_end[-1] == ' ' || value_end[-1] == '\t')) value_end--;
        size_t value_len = (size_t)(value_end - value);
        if (http_ascii_eq_ci(raw + pos, name_len, "Content-Length")) {
            char buf_len[32];
            if (value_len >= sizeof(buf_len)) {
                return http_response_parse_failure(failure_tag, 0, "invalid Content-Length");
            }
            memcpy(buf_len, value, value_len);
            buf_len[value_len] = '\0';
            char *endptr = NULL;
            long long parsed = strtoll(buf_len, &endptr, 10);
            if (!endptr || *endptr != '\0' || parsed < 0) {
                return http_response_parse_failure(failure_tag, 0, "invalid Content-Length");
            }
            if (content_length >= 0 && content_length != parsed) {
                return http_response_parse_failure(failure_tag, 0, "conflicting Content-Length headers");
            }
            content_length = parsed;
        } else if (http_ascii_eq_ci(raw + pos, name_len, "Transfer-Encoding")) {
            if (http_ascii_eq_ci(value, value_len, "chunked")) chunked = 1;
        }
        pos = end + 2;
    }
    if (chunked && content_length >= 0) {
        return http_response_parse_failure(failure_tag, 0, "conflicting Content-Length and Transfer-Encoding");
    }

    size_t body_start = header_end + 4;
    const char *body = raw + body_start;
    size_t body_len = 0;
    size_t consumed = body_start;
    char *decoded = NULL;
    if (chunked) {
        size_t chunk_pos = body_start;
        size_t cap = 0;
        while (1) {
            size_t crlf = http_find_bytes(raw + chunk_pos, raw_len - chunk_pos, "\r\n", 2);
            if (crlf == (size_t)-1) return http_make_adt_scan(need_more_tag, NULL, 0, 0);
            char size_buf[32];
            size_t size_len = crlf;
            if (size_len >= sizeof(size_buf)) {
                return http_response_parse_failure(failure_tag, 0, "invalid chunk size");
            }
            memcpy(size_buf, raw + chunk_pos, size_len);
            size_buf[size_len] = '\0';
            char *semi = strchr(size_buf, ';');
            if (semi) *semi = '\0';
            char *endptr = NULL;
            long chunk_size = strtol(size_buf, &endptr, 16);
            if (!endptr || (*endptr != '\0' && *endptr != ';') || chunk_size < 0) {
                free(decoded);
                return http_response_parse_failure(failure_tag, 0, "invalid chunk size");
            }
            chunk_pos += crlf + 2;
            if (chunk_size == 0) {
                if (raw_len < chunk_pos + 2) {
                    free(decoded);
                    return http_make_adt_scan(need_more_tag, NULL, 0, 0);
                }
                if (memcmp(raw + chunk_pos, "\r\n", 2) != 0) {
                    free(decoded);
                    return http_response_parse_failure(failure_tag, 0, "chunk trailer fields are not supported in Phase 3");
                }
                consumed = chunk_pos + 2;
                body = decoded ? decoded : "";
                break;
            }
            if (body_len + (size_t)chunk_size > max_body) {
                free(decoded);
                return http_response_parse_failure(failure_tag, 413, "HTTP response body exceeds max_body_bytes");
            }
            if (raw_len < chunk_pos + (size_t)chunk_size + 2) {
                free(decoded);
                return http_make_adt_scan(need_more_tag, NULL, 0, 0);
            }
            if (memcmp(raw + chunk_pos + chunk_size, "\r\n", 2) != 0) {
                free(decoded);
                return http_response_parse_failure(failure_tag, 0, "chunk missing trailing CRLF");
            }
            if (body_len + (size_t)chunk_size > cap) {
                cap = (body_len + (size_t)chunk_size) * 2 + 16;
                char *next = (char *)realloc(decoded, cap);
                if (!next) {
                    free(decoded);
                    fprintf(stderr, "flux_http_parse_response: out of memory\n");
                    abort();
                }
                decoded = next;
            }
            memcpy(decoded + body_len, raw + chunk_pos, (size_t)chunk_size);
            body_len += (size_t)chunk_size;
            chunk_pos += (size_t)chunk_size + 2;
        }
    } else if (content_length >= 0) {
        if ((size_t)content_length > max_body) {
            return http_response_parse_failure(failure_tag, 413, "HTTP response body exceeds max_body_bytes");
        }
        if (raw_len < body_start + (size_t)content_length) {
            return http_make_adt_scan(need_more_tag, NULL, 0, 0);
        }
        body_len = (size_t)content_length;
        consumed = body_start + body_len;
    }

    int64_t response = http_response_value(response_tag, (int)status, body, body_len);
    free(decoded);
    int64_t fields[2] = { response, flux_tag_int((int64_t)consumed) };
    return http_make_adt_scan(parsed_tag, fields, 2, 0);
}

static int64_t flux_tcp_suspend_or_abort(uint64_t request_id, const char *which) {
    if (request_id == 0) {
        fprintf(stderr, "%s: async TCP request registration failed\n", which);
        abort();
    }
    return flux_async_suspend_request(request_id);
}

int64_t flux_tcp_connect(int64_t host_val, int64_t port_val) {
    const uint8_t *host = (const uint8_t *)flux_string_data(host_val);
    uintptr_t host_len = (uintptr_t)flux_string_len(host_val);
    int64_t port = flux_untag_int(port_val);
    uint64_t request_id = flux_async_tcp_connect(host, host_len, port);
    return flux_tcp_suspend_or_abort(request_id, "flux_tcp_connect");
}

int64_t flux_tcp_read(int64_t handle_val, int64_t max_val) {
    uint64_t handle = (uint64_t)flux_untag_int(handle_val);
    int64_t raw_max = flux_untag_int(max_val);
    uintptr_t max = raw_max > 0 ? (uintptr_t)raw_max : 0;
    uint64_t request_id = flux_async_tcp_read(handle, max);
    return flux_tcp_suspend_or_abort(request_id, "flux_tcp_read");
}

int64_t flux_tcp_write_all(int64_t handle_val, int64_t data_val) {
    uint64_t handle = (uint64_t)flux_untag_int(handle_val);
    const uint8_t *data = (const uint8_t *)flux_string_data(data_val);
    uintptr_t len = (uintptr_t)flux_string_len(data_val);
    uint64_t request_id = flux_async_tcp_write_all(handle, data, len);
    return flux_tcp_suspend_or_abort(request_id, "flux_tcp_write_all");
}

int64_t flux_tcp_close(int64_t handle_val) {
    uint64_t handle = (uint64_t)flux_untag_int(handle_val);
    (void)flux_async_tcp_close(handle);
    return FLUX_NONE;
}

int64_t flux_tcp_listen(int64_t host_val, int64_t port_val) {
    const uint8_t *host = (const uint8_t *)flux_string_data(host_val);
    uintptr_t host_len = (uintptr_t)flux_string_len(host_val);
    int64_t port = flux_untag_int(port_val);
    uint64_t request_id = flux_async_tcp_listen(host, host_len, port);
    return flux_tcp_suspend_or_abort(request_id, "flux_tcp_listen");
}

int64_t flux_tcp_accept(int64_t listener_val) {
    uint64_t listener = (uint64_t)flux_untag_int(listener_val);
    uint64_t request_id = flux_async_tcp_accept(listener);
    return flux_tcp_suspend_or_abort(request_id, "flux_tcp_accept");
}

int64_t flux_http_serve_config(
    int64_t listener_val,
    int64_t scope_val,
    int64_t config_val
) {
    http_lock();
    if (http_server_len == http_server_cap) {
        size_t next_cap = http_server_cap == 0 ? 8 : http_server_cap * 2;
        HttpServerState *next =
            (HttpServerState *)realloc(http_servers, next_cap * sizeof(HttpServerState));
        if (!next) {
            http_unlock();
            fprintf(stderr, "flux_http_serve_config: out of memory\n");
            abort();
        }
        http_servers = next;
        http_server_cap = next_cap;
    }

    int64_t id = http_next_server_id++;
    HttpServerState *state = &http_servers[http_server_len++];
    memset(state, 0, sizeof(*state));
    state->listener = (uint64_t)flux_untag_int(listener_val);
    state->scope = (uint64_t)flux_untag_int(scope_val);
    state->max_connections = (size_t)http_config_int_field(config_val, 0, 10000);
    state->max_header_bytes = (size_t)http_config_int_field(config_val, 1, 65536);
    state->max_body_bytes = (size_t)http_config_int_field(config_val, 2, 8388608);
    state->request_timeout_ms = (size_t)http_config_int_field(config_val, 3, 30000);
    http_unlock();
    return flux_tag_int(id);
}

int64_t flux_http_shutdown(int64_t handle_val) {
    http_lock();
    HttpServerState *state = http_find_server(flux_untag_int(handle_val));
    if (state) {
        state->shutting_down = 1;
        if (state->active_len == 0) {
            state->stopped = 1;
        }
    }
    http_unlock();
    return FLUX_NONE;
}

int64_t flux_http_shutdown_now(int64_t handle_val) {
    http_lock();
    HttpServerState *state = http_find_server(flux_untag_int(handle_val));
    if (state) {
        state->shutting_down = 1;
        http_close_listener(state);
        for (size_t i = 0; i < state->active_len; i++) {
            (void)flux_async_tcp_close(state->active[i]);
        }
        state->active_len = 0;
        (void)flux_async_cancel_scope(state->scope);
    }
    http_unlock();
    return FLUX_NONE;
}

int64_t flux_http_parse_request(
    int32_t need_more_tag,
    int32_t parsed_tag,
    int32_t parse_failure_tag,
    int32_t request_tag,
    int32_t get_tag,
    int32_t post_tag,
    int32_t put_tag,
    int32_t delete_tag,
    int32_t patch_tag,
    int32_t head_tag,
    int32_t options_tag,
    int64_t raw_val,
    int64_t server_val
) {
    const char *raw = flux_string_data(raw_val);
    size_t raw_len = (size_t)flux_string_len(raw_val);
    HttpServerState *state = http_find_server(flux_untag_int(server_val));
    size_t max_header = state ? state->max_header_bytes : 65536;
    size_t max_body = state ? state->max_body_bytes : 8388608;

    size_t header_end = http_find_bytes(raw, raw_len, "\r\n\r\n", 4);
    if (header_end == (size_t)-1) {
        if (raw_len > max_header) {
            return http_parse_failure(parse_failure_tag, 413, "HTTP header block exceeds max_header_bytes");
        }
        return http_make_adt_scan(need_more_tag, NULL, 0, 0);
    }
    if (header_end > max_header) {
        return http_parse_failure(parse_failure_tag, 413, "HTTP header block exceeds max_header_bytes");
    }

    size_t line_end = http_find_bytes(raw, header_end, "\r\n", 2);
    if (line_end == (size_t)-1) {
        return http_parse_failure(parse_failure_tag, 400, "missing request line");
    }
    const char *line = raw;
    const char *sp1 = memchr(line, ' ', line_end);
    if (!sp1) return http_parse_failure(parse_failure_tag, 400, "missing request target");
    const char *sp2 = memchr(sp1 + 1, ' ', (size_t)(raw + line_end - sp1 - 1));
    if (!sp2) return http_parse_failure(parse_failure_tag, 400, "missing HTTP version");
    size_t method_len = (size_t)(sp1 - line);
    size_t target_len = (size_t)(sp2 - sp1 - 1);
    size_t version_len = line_end - (size_t)(sp2 + 1 - raw);
    if (method_len == 0 || target_len == 0) {
        return http_parse_failure(parse_failure_tag, 400, "malformed request line");
    }
    for (size_t i = 0; i < method_len; i++) {
        if (!isupper((unsigned char)line[i])) {
            return http_parse_failure(parse_failure_tag, 400, "invalid HTTP method token");
        }
    }
    if (version_len != 8 || memcmp(sp2 + 1, "HTTP/1.1", 8) != 0) {
        return http_parse_failure(parse_failure_tag, 400, "unsupported HTTP version");
    }

    long long content_length = -1;
    int chunked = 0;
    int keep_alive = 1;
    size_t pos = line_end + 2;
    while (pos < header_end) {
        size_t end = pos;
        while (end < header_end && !(raw[end] == '\r' && end + 1 < header_end && raw[end + 1] == '\n')) {
            end++;
        }
        if (end == pos) break;
        if (raw[pos] == ' ' || raw[pos] == '\t') {
            return http_parse_failure(parse_failure_tag, 400, "obsolete folded HTTP headers are rejected");
        }
        const char *colon = memchr(raw + pos, ':', end - pos);
        if (!colon) return http_parse_failure(parse_failure_tag, 400, "HTTP header missing ':'");
        size_t name_len = (size_t)(colon - (raw + pos));
        const char *value = colon + 1;
        const char *value_end = raw + end;
        while (value < value_end && (*value == ' ' || *value == '\t')) value++;
        while (value_end > value && (value_end[-1] == ' ' || value_end[-1] == '\t')) value_end--;
        size_t value_len = (size_t)(value_end - value);
        if (http_ascii_eq_ci(raw + pos, name_len, "Content-Length")) {
            char buf[32];
            if (value_len >= sizeof(buf)) {
                return http_parse_failure(parse_failure_tag, 400, "invalid Content-Length");
            }
            memcpy(buf, value, value_len);
            buf[value_len] = '\0';
            char *endptr = NULL;
            long long parsed = strtoll(buf, &endptr, 10);
            if (!endptr || *endptr != '\0' || parsed < 0) {
                return http_parse_failure(parse_failure_tag, 400, "invalid Content-Length");
            }
            if (content_length >= 0 && content_length != parsed) {
                return http_parse_failure(parse_failure_tag, 400, "conflicting Content-Length headers");
            }
            content_length = parsed;
        } else if (http_ascii_eq_ci(raw + pos, name_len, "Transfer-Encoding")) {
            if (http_ascii_eq_ci(value, value_len, "chunked")) chunked = 1;
        } else if (http_ascii_eq_ci(raw + pos, name_len, "Connection")) {
            if (http_ascii_eq_ci(value, value_len, "close")) keep_alive = 0;
        }
        pos = end + 2;
    }

    if (chunked && content_length >= 0) {
        return http_parse_failure(parse_failure_tag, 400, "conflicting Content-Length and Transfer-Encoding");
    }

    size_t body_start = header_end + 4;
    const char *body = raw + body_start;
    size_t body_len = 0;
    size_t consumed = body_start;
    char *decoded = NULL;
    if (chunked) {
        size_t chunk_pos = body_start;
        size_t cap = 0;
        while (1) {
            size_t crlf = http_find_bytes(raw + chunk_pos, raw_len - chunk_pos, "\r\n", 2);
            if (crlf == (size_t)-1) return http_make_adt_scan(need_more_tag, NULL, 0, 0);
            char size_buf[32];
            size_t size_len = crlf;
            if (size_len >= sizeof(size_buf)) {
                return http_parse_failure(parse_failure_tag, 400, "invalid chunk size");
            }
            memcpy(size_buf, raw + chunk_pos, size_len);
            size_buf[size_len] = '\0';
            char *semi = strchr(size_buf, ';');
            if (semi) *semi = '\0';
            char *endptr = NULL;
            long chunk_size = strtol(size_buf, &endptr, 16);
            if (!endptr || (*endptr != '\0' && *endptr != ';') || chunk_size < 0) {
                free(decoded);
                return http_parse_failure(parse_failure_tag, 400, "invalid chunk size");
            }
            chunk_pos += crlf + 2;
            if (chunk_size == 0) {
                if (raw_len < chunk_pos + 2) {
                    free(decoded);
                    return http_make_adt_scan(need_more_tag, NULL, 0, 0);
                }
                if (memcmp(raw + chunk_pos, "\r\n", 2) != 0) {
                    free(decoded);
                    return http_parse_failure(parse_failure_tag, 400, "chunk trailer fields are not supported in Phase 3a");
                }
                consumed = chunk_pos + 2;
                body = decoded ? decoded : "";
                break;
            }
            if (body_len + (size_t)chunk_size > max_body) {
                free(decoded);
                return http_parse_failure(parse_failure_tag, 413, "HTTP chunked body exceeds max_body_bytes");
            }
            if (raw_len < chunk_pos + (size_t)chunk_size + 2) {
                free(decoded);
                return http_make_adt_scan(need_more_tag, NULL, 0, 0);
            }
            if (memcmp(raw + chunk_pos + chunk_size, "\r\n", 2) != 0) {
                free(decoded);
                return http_parse_failure(parse_failure_tag, 400, "chunk missing trailing CRLF");
            }
            if (body_len + (size_t)chunk_size > cap) {
                cap = (body_len + (size_t)chunk_size) * 2 + 16;
                char *next = (char *)realloc(decoded, cap);
                if (!next) {
                    free(decoded);
                    fprintf(stderr, "flux_http_parse_request: out of memory\n");
                    abort();
                }
                decoded = next;
            }
            memcpy(decoded + body_len, raw + chunk_pos, (size_t)chunk_size);
            body_len += (size_t)chunk_size;
            chunk_pos += (size_t)chunk_size + 2;
        }
    } else if (content_length >= 0) {
        if ((size_t)content_length > max_body) {
            return http_parse_failure(parse_failure_tag, 413, "HTTP body exceeds max_body_bytes");
        }
        if (raw_len < body_start + (size_t)content_length) {
            return http_make_adt_scan(need_more_tag, NULL, 0, 0);
        }
        body_len = (size_t)content_length;
        consumed = body_start + body_len;
    }

    int32_t method_tags[7] = {
        get_tag, post_tag, put_tag, delete_tag, patch_tag, head_tag, options_tag
    };
    int64_t request = http_request_value(
        request_tag,
        method_tags,
        line,
        method_len,
        sp1 + 1,
        target_len,
        body,
        body_len
    );
    free(decoded);
    int64_t fields[3] = { request, flux_tag_int((int64_t)consumed), flux_make_bool(keep_alive) };
    return http_make_adt_scan(parsed_tag, fields, 3, 0);
}

int64_t flux_http_write_response(int64_t response_val, int64_t keep_alive_val) {
    int32_t count = 0;
    int64_t *fields = http_adt_fields(response_val, NULL, &count);
    if (fields && count == 1) {
        int32_t inner_count = 0;
        int64_t *inner = http_adt_fields(fields[0], NULL, &inner_count);
        if (inner && inner_count >= 3) {
            fields = inner;
            count = inner_count;
        }
    }
    int64_t status = 200;
    int64_t body_val = flux_string_new("", 0);
    if (fields && count >= 3) {
        if (flux_is_int(fields[0])) status = flux_untag_int(fields[0]);
        body_val = fields[2];
    }
    const char *body = flux_string_data(body_val);
    uint32_t body_len = flux_string_len(body_val);
    const char *reason = http_reason(status);
    const char *conn = keep_alive_val == FLUX_TRUE ? "keep-alive" : "close";
    int header_len = snprintf(
        NULL,
        0,
        "HTTP/1.1 %lld %s\r\nConnection: %s\r\nContent-Length: %u\r\n\r\n",
        (long long)status,
        reason,
        conn,
        body_len
    );
    if (header_len < 0) header_len = 0;
    size_t total = (size_t)header_len + body_len;
    char *wire = (char *)malloc(total + 1);
    if (!wire) {
        fprintf(stderr, "flux_http_write_response: out of memory\n");
        abort();
    }
    snprintf(
        wire,
        (size_t)header_len + 1,
        "HTTP/1.1 %lld %s\r\nConnection: %s\r\nContent-Length: %u\r\n\r\n",
        (long long)status,
        reason,
        conn,
        body_len
    );
    if (body_len > 0) memcpy(wire + header_len, body, body_len);
    int64_t result = flux_string_new(wire, (uint32_t)total);
    free(wire);
    return result;
}

int64_t flux_http_register_connection(int64_t server_val, int64_t conn_val) {
    http_lock();
    HttpServerState *state = http_find_server(flux_untag_int(server_val));
    http_active_add(state, (uint64_t)flux_untag_int(conn_val));
    http_unlock();
    return FLUX_NONE;
}

int64_t flux_http_unregister_connection(int64_t server_val, int64_t conn_val) {
    http_lock();
    HttpServerState *state = http_find_server(flux_untag_int(server_val));
    http_active_remove(state, (uint64_t)flux_untag_int(conn_val));
    if (state && state->shutting_down && state->active_len == 0) {
        state->stopped = 1;
    }
    http_unlock();
    return FLUX_NONE;
}

int64_t flux_http_active_connection_count(int64_t server_val) {
    http_lock();
    HttpServerState *state = http_find_server(flux_untag_int(server_val));
    int64_t count = state ? (int64_t)state->active_len : 0;
    http_unlock();
    return flux_tag_int(count);
}

int64_t flux_http_is_shutting_down(int64_t server_val) {
    http_lock();
    HttpServerState *state = http_find_server(flux_untag_int(server_val));
    int shutting_down = !state || state->shutting_down;
    http_unlock();
    return flux_make_bool(shutting_down);
}

int64_t flux_http_server_stopped(int64_t server_val) {
    http_lock();
    HttpServerState *state = http_find_server(flux_untag_int(server_val));
    if (state) state->stopped = 1;
    http_unlock();
    return FLUX_NONE;
}

int64_t flux_http_is_server_stopped(int64_t server_val) {
    http_lock();
    HttpServerState *state = http_find_server(flux_untag_int(server_val));
    int stopped = !state || state->stopped;
    http_unlock();
    return flux_make_bool(stopped);
}
