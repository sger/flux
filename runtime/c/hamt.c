/*
 * hamt.c — Persistent Hash Array Mapped Trie (HAMT).
 *
 * Matches the semantics of src/runtime/hamt.rs: 5-bit-per-level trie
 * with bitmap compression and hash collision nodes.
 *
 * All keys and values are pointer-tagged i64.  Keys are hashed via a simple
 * FNV-1a variant.  The HAMT is immutable (persistent): set/delete
 * return new nodes, sharing unchanged subtrees.
 *
 * Each node is a counted block (FLUX_OBJ_MAP) whose fields are tagged
 * values: a branch's children, or a leaf's or collision's keys and values.
 * A node holds a counted reference to each of them, so flux_drop and
 * flux_rc_promote walk a map like any other block, and a new node takes
 * its own reference to everything it shares with the node it was copied
 * from.
 */

#include "flux_rt.h"
#include <stdlib.h>
#if defined(_MSC_VER)
#include <intrin.h>
#define __builtin_popcount __popcnt
#endif
#include <string.h>
#include <stdio.h>

/* ── Constants ──────────────────────────────────────────────────────── */

#define BITS_PER_LEVEL  5
#define SLOTS_PER_LEVEL 32   /* 1 << BITS_PER_LEVEL */
#define MAX_DEPTH       13   /* ceil(64 / 5) */
#define SLOT_MASK       0x1F

/* ── Node types ─────────────────────────────────────────────────────── */

typedef enum {
    HAMT_EMPTY,
    HAMT_LEAF,
    HAMT_BRANCH,
    HAMT_COLLISION,
} HamtKind;

/*
 * Leaf:      fields = key, value
 * Branch:    fields = child nodes, popcount(bitmap) of them, never empty
 * Collision: fields = key0, value0, key1, value1, ...
 *
 * `nfields` sits where an array keeps its length, so rc.c counts both the
 * same way.
 */
typedef struct {
    int32_t  kind;
    uint32_t nfields;
    uint64_t hash;       /* leaf, collision: the hash; branch: the bitmap */
    int64_t  fields[];
} HamtNode;

static HamtNode *node_of(int64_t val) {
    return (HamtNode *)flux_untag_ptr(val);
}

static uint32_t branch_bitmap(HamtNode *n) {
    return (uint32_t)n->hash;
}

static HamtNode *branch_child(HamtNode *n, uint32_t idx) {
    return node_of(n->fields[idx]);
}

/* ── Hashing ────────────────────────────────────────────────────────── */

static uint64_t fnv1a_bytes(const uint8_t *data, size_t len) {
    uint64_t h = 14695981039346656037ULL;
    for (size_t i = 0; i < len; i++) {
        h ^= data[i];
        h *= 1099511628211ULL;
    }
    return h;
}

static uint64_t hamt_hash(int64_t key) {
    /* For string keys, hash the string content (not the pointer).
     * This ensures two strings with the same bytes get the same hash,
     * matching the VM's HashKey::String behavior. */
    if (flux_is_ptr(key)) {
        void *ptr = flux_untag_ptr(key);
        if (ptr && flux_obj_tag(ptr) == FLUX_OBJ_STRING) {
            uint32_t len = *(uint32_t *)((char *)ptr + 4);
            const char *data = (const char *)ptr + 8;
            return fnv1a_bytes((const uint8_t *)data, len);
        }
    }
    /* For non-string keys, hash the raw i64 bytes. */
    uint64_t k = (uint64_t)key;
    return fnv1a_bytes((const uint8_t *)&k, 8);
}

static uint32_t slot_at_depth(uint64_t hash, uint32_t depth) {
    return (uint32_t)((hash >> (depth * BITS_PER_LEVEL)) & SLOT_MASK);
}

static uint32_t compressed_index(uint32_t bitmap, uint32_t slot) {
    return __builtin_popcount(bitmap & ((1u << slot) - 1));
}

/* ── Key equality ───────────────────────────────────────────────────── */

static int keys_equal(int64_t a, int64_t b) {
    if (a == b) return 1;
    /* For boxed string values, compare by content. */
    if (flux_is_ptr(a) && flux_is_ptr(b)) {
        return flux_string_eq(a, b);
    }
    return 0;
}

/* ── Node allocation ────────────────────────────────────────────────── */

static HamtNode *alloc_node(int32_t kind, uint32_t nfields, uint64_t hash) {
    HamtNode *n = (HamtNode *)flux_gc_alloc_header(
        (uint32_t)(sizeof(HamtNode) + nfields * sizeof(int64_t)), 0, FLUX_OBJ_MAP);
    n->kind = kind;
    n->nfields = nfields;
    n->hash = hash;
    return n;
}

/* The leaf takes the references to `key` and `value` it is given. */
static HamtNode *make_leaf(uint64_t hash, int64_t key, int64_t value) {
    HamtNode *n = alloc_node(HAMT_LEAF, 2, hash);
    n->fields[0] = key;
    n->fields[1] = value;
    return n;
}

static HamtNode *make_empty(void) {
    return alloc_node(HAMT_EMPTY, 0, 0);
}

/* `fields[from..from+count]` of `src` into `dst` at `at`, each counted. */
static void copy_fields(HamtNode *dst, uint32_t at, HamtNode *src, uint32_t from, uint32_t count) {
    if (count == 0) return;
    memcpy(&dst->fields[at], &src->fields[from], count * sizeof(int64_t));
    flux_dup_range(&dst->fields[at], count);
}

/* A copy of branch `br` with `child` (owned) inserted at `slot`. */
static HamtNode *copy_branch_insert(HamtNode *br, uint32_t slot, HamtNode *child) {
    uint32_t bitmap = branch_bitmap(br);
    uint32_t idx = compressed_index(bitmap, slot);
    HamtNode *n = alloc_node(HAMT_BRANCH, br->nfields + 1, bitmap | (1u << slot));
    copy_fields(n, 0, br, 0, idx);
    n->fields[idx] = flux_tag_ptr(child);
    copy_fields(n, idx + 1, br, idx, br->nfields - idx);
    return n;
}

/* A copy of branch `br` with the child at `idx` replaced by `child` (owned). */
static HamtNode *copy_branch_replace(HamtNode *br, uint32_t idx, HamtNode *child) {
    HamtNode *n = alloc_node(HAMT_BRANCH, br->nfields, br->hash);
    copy_fields(n, 0, br, 0, idx);
    n->fields[idx] = flux_tag_ptr(child);
    copy_fields(n, idx + 1, br, idx + 1, br->nfields - idx - 1);
    return n;
}

/* A copy of branch `br` without the child at `slot`; `br` has at least two. */
static HamtNode *copy_branch_remove(HamtNode *br, uint32_t slot) {
    uint32_t bitmap = branch_bitmap(br);
    uint32_t idx = compressed_index(bitmap, slot);
    HamtNode *n = alloc_node(HAMT_BRANCH, br->nfields - 1, bitmap & ~(1u << slot));
    copy_fields(n, 0, br, 0, idx);
    copy_fields(n, idx, br, idx + 1, br->nfields - idx - 1);
    return n;
}

/* ── Two leaves → branch or collision ───────────────────────────────── */

/* Takes both leaves (owned). */
static HamtNode *merge_leaves(HamtNode *a, HamtNode *b, uint32_t depth) {
    if (depth >= MAX_DEPTH || a->hash == b->hash) {
        /* Hash collision. */
        HamtNode *n = alloc_node(HAMT_COLLISION, 4, a->hash);
        copy_fields(n, 0, a, 0, 2);
        copy_fields(n, 2, b, 0, 2);
        flux_drop(flux_tag_ptr(a));
        flux_drop(flux_tag_ptr(b));
        return n;
    }

    uint32_t slot_a = slot_at_depth(a->hash, depth);
    uint32_t slot_b = slot_at_depth(b->hash, depth);

    if (slot_a != slot_b) {
        uint32_t bitmap = (1u << slot_a) | (1u << slot_b);
        HamtNode *n = alloc_node(HAMT_BRANCH, 2, bitmap);
        n->fields[compressed_index(bitmap, slot_a)] = flux_tag_ptr(a);
        n->fields[compressed_index(bitmap, slot_b)] = flux_tag_ptr(b);
        return n;
    }
    /* Same slot — recurse deeper. */
    HamtNode *merged = merge_leaves(a, b, depth + 1);
    HamtNode *n = alloc_node(HAMT_BRANCH, 1, 1u << slot_a);
    n->fields[0] = flux_tag_ptr(merged);
    return n;
}

/* ── Lookup ─────────────────────────────────────────────────────────── */

static int64_t *hamt_get_impl(HamtNode *node, uint64_t hash, int64_t key, uint32_t depth) {
    switch (node->kind) {
    case HAMT_EMPTY:
        return NULL;

    case HAMT_LEAF:
        if (keys_equal(node->fields[0], key))
            return &node->fields[1];
        return NULL;

    case HAMT_BRANCH: {
        uint32_t slot = slot_at_depth(hash, depth);
        uint32_t bitmap = branch_bitmap(node);
        if (!(bitmap & (1u << slot))) return NULL;
        uint32_t idx = compressed_index(bitmap, slot);
        return hamt_get_impl(branch_child(node, idx), hash, key, depth + 1);
    }

    case HAMT_COLLISION:
        for (uint32_t i = 0; i < node->nfields; i += 2) {
            if (keys_equal(node->fields[i], key))
                return &node->fields[i + 1];
        }
        return NULL;
    }
    return NULL;
}

/* ── Insert ─────────────────────────────────────────────────────────── */

/* A new node with `key` set to `value`; `node`, `key` and `value` are lent. */
static HamtNode *hamt_set_impl(HamtNode *node, uint64_t hash, int64_t key, int64_t value, uint32_t depth) {
    switch (node->kind) {
    case HAMT_EMPTY:
        flux_dup(key);
        flux_dup(value);
        return make_leaf(hash, key, value);

    case HAMT_LEAF: {
        flux_dup(key);
        flux_dup(value);
        HamtNode *leaf = make_leaf(hash, key, value);
        if (keys_equal(node->fields[0], key)) {
            /* Update existing key. */
            return leaf;
        }
        /* Split into branch or collision, sharing the old leaf. */
        flux_dup(flux_tag_ptr(node));
        return merge_leaves(node, leaf, depth);
    }

    case HAMT_BRANCH: {
        uint32_t slot = slot_at_depth(hash, depth);
        uint32_t bitmap = branch_bitmap(node);
        if (bitmap & (1u << slot)) {
            /* Slot occupied — recurse. */
            uint32_t idx = compressed_index(bitmap, slot);
            HamtNode *updated = hamt_set_impl(branch_child(node, idx), hash, key, value, depth + 1);
            return copy_branch_replace(node, idx, updated);
        }
        /* Insert new child. */
        flux_dup(key);
        flux_dup(value);
        return copy_branch_insert(node, slot, make_leaf(hash, key, value));
    }

    case HAMT_COLLISION: {
        uint32_t n_old = node->nfields;
        for (uint32_t i = 0; i < n_old; i += 2) {
            if (keys_equal(node->fields[i], key)) {
                /* Replace value. */
                HamtNode *n = alloc_node(HAMT_COLLISION, n_old, node->hash);
                copy_fields(n, 0, node, 0, i + 1);
                flux_dup(value);
                n->fields[i + 1] = value;
                copy_fields(n, i + 2, node, i + 2, n_old - i - 2);
                return n;
            }
        }
        /* Add new entry. */
        HamtNode *n = alloc_node(HAMT_COLLISION, n_old + 2, node->hash);
        copy_fields(n, 0, node, 0, n_old);
        flux_dup(key);
        flux_dup(value);
        n->fields[n_old] = key;
        n->fields[n_old + 1] = value;
        return n;
    }
    }
    return node; /* unreachable */
}

/* ── Delete ─────────────────────────────────────────────────────────── */

/*
 * `node` without `key`: NULL when nothing is left, `node` itself (no new
 * reference) when `key` is not in it, and otherwise a new node.
 */
static HamtNode *hamt_delete_impl(HamtNode *node, uint64_t hash, int64_t key, uint32_t depth) {
    switch (node->kind) {
    case HAMT_EMPTY:
        return node;

    case HAMT_LEAF:
        if (keys_equal(node->fields[0], key))
            return NULL;
        return node;

    case HAMT_BRANCH: {
        uint32_t slot = slot_at_depth(hash, depth);
        uint32_t bitmap = branch_bitmap(node);
        if (!(bitmap & (1u << slot))) return node;
        uint32_t idx = compressed_index(bitmap, slot);
        HamtNode *child = branch_child(node, idx);
        HamtNode *updated = hamt_delete_impl(child, hash, key, depth + 1);
        if (updated == child) return node;
        if (updated) return copy_branch_replace(node, idx, updated);
        if (node->nfields == 1) return NULL;
        if (node->nfields == 2) {
            /* If only one child is left and it's a leaf, collapse. */
            HamtNode *other = branch_child(node, 1 - idx);
            if (other->kind == HAMT_LEAF) {
                flux_dup(flux_tag_ptr(other));
                return other;
            }
        }
        return copy_branch_remove(node, slot);
    }

    case HAMT_COLLISION: {
        uint32_t n_old = node->nfields;
        for (uint32_t i = 0; i < n_old; i += 2) {
            if (!keys_equal(node->fields[i], key)) continue;
            if (n_old == 2) return NULL;
            if (n_old == 4) {
                /* Collapse to leaf. */
                uint32_t other = (i == 0) ? 2 : 0;
                flux_dup(node->fields[other]);
                flux_dup(node->fields[other + 1]);
                return make_leaf(node->hash, node->fields[other], node->fields[other + 1]);
            }
            HamtNode *n = alloc_node(HAMT_COLLISION, n_old - 2, node->hash);
            copy_fields(n, 0, node, 0, i);
            copy_fields(n, i, node, i + 2, n_old - i - 2);
            return n;
        }
        return node;
    }
    }
    return node; /* unreachable */
}

/* ── Size (count all entries) ───────────────────────────────────────── */

static uint32_t hamt_size_impl(HamtNode *node) {
    switch (node->kind) {
    case HAMT_EMPTY:     return 0;
    case HAMT_LEAF:      return 1;
    case HAMT_COLLISION: return node->nfields / 2;
    case HAMT_BRANCH: {
        uint32_t total = 0;
        for (uint32_t i = 0; i < node->nfields; i++)
            total += hamt_size_impl(branch_child(node, i));
        return total;
    }
    }
    return 0;
}

/* ── Public API (pointer-tagged) ───────────────────────────────────── */

int64_t flux_hamt_empty(void) {
    HamtNode *root = make_empty();
    return flux_tag_ptr(root);
}

int64_t flux_hamt_get(int64_t map, int64_t key) {
    HamtNode *root = node_of(map);
    if (!root) return flux_make_none();
    uint64_t hash = hamt_hash(key);
    int64_t *val = hamt_get_impl(root, hash, key, 0);
    if (!val) return flux_make_none();
    return *val;
}

/* The map operations are lent their arguments and return a new map. */

int64_t flux_hamt_set(int64_t map, int64_t key, int64_t value) {
    HamtNode *root = node_of(map);
    uint64_t hash = hamt_hash(key);
    if (!root) {
        flux_dup(key);
        flux_dup(value);
        return flux_tag_ptr(make_leaf(hash, key, value));
    }
    return flux_tag_ptr(hamt_set_impl(root, hash, key, value, 0));
}

int64_t flux_hamt_delete(int64_t map, int64_t key) {
    HamtNode *root = node_of(map);
    if (!root) return map;
    uint64_t hash = hamt_hash(key);
    HamtNode *new_root = hamt_delete_impl(root, hash, key, 0);
    if (!new_root) return flux_hamt_empty();
    /* A missing key gives the same map back. */
    if (new_root == root) flux_dup(map);
    return flux_tag_ptr(new_root);
}

int64_t flux_hamt_contains(int64_t map, int64_t key) {
    HamtNode *root = node_of(map);
    if (!root) return flux_make_bool(0);
    uint64_t hash = hamt_hash(key);
    int64_t *val = hamt_get_impl(root, hash, key, 0);
    return flux_make_bool(val != NULL);
}

int64_t flux_hamt_size(int64_t map) {
    HamtNode *root = node_of(map);
    if (!root) return flux_tag_int(0);
    return flux_tag_int((int64_t)hamt_size_impl(root));
}

/* ── Collect all keys or values into a flat array ───────────────────── */

/* `which` is 0 for keys, 1 for values. */
static void hamt_collect(HamtNode *node, int which, int64_t *out, uint32_t *idx) {
    switch (node->kind) {
    case HAMT_EMPTY:
        break;
    case HAMT_LEAF:
    case HAMT_COLLISION:
        for (uint32_t i = 0; i < node->nfields; i += 2) {
            out[*idx] = node->fields[i + which];
            (*idx)++;
        }
        break;
    case HAMT_BRANCH:
        for (uint32_t i = 0; i < node->nfields; i++) {
            hamt_collect(branch_child(node, i), which, out, idx);
        }
        break;
    }
}

static void hamt_collect_keys(HamtNode *node, int64_t *out, uint32_t *idx) {
    hamt_collect(node, 0, out, idx);
}

static void hamt_collect_values(HamtNode *node, int64_t *out, uint32_t *idx) {
    hamt_collect(node, 1, out, idx);
}

int64_t flux_hamt_values(int64_t map) {
    HamtNode *root = node_of(map);
    if (!root) return flux_array_new(NULL, 0);
    uint32_t size = hamt_size_impl(root);
    if (size == 0) return flux_array_new(NULL, 0);
    int64_t *vals = (int64_t *)malloc(size * sizeof(int64_t));
    uint32_t idx = 0;
    hamt_collect_values(root, vals, &idx);
    flux_dup_range(vals, idx);
    int64_t result = flux_array_new(vals, (int32_t)idx);
    free(vals);
    return result;
}

/* Merge two HAMTs: keys from b override keys from a. */
int64_t flux_hamt_merge(int64_t a, int64_t b) {
    HamtNode *root_b = node_of(b);
    uint32_t size_b = root_b ? hamt_size_impl(root_b) : 0;
    if (size_b == 0) {
        flux_dup(a);
        return a;
    }

    /* Collect all key-value pairs from b, then set them into a. */
    int64_t *keys_arr = (int64_t *)malloc(size_b * sizeof(int64_t));
    int64_t *vals_arr = (int64_t *)malloc(size_b * sizeof(int64_t));
    uint32_t ki = 0, vi = 0;
    hamt_collect_keys(root_b, keys_arr, &ki);
    hamt_collect_values(root_b, vals_arr, &vi);

    int64_t result = a;
    for (uint32_t i = 0; i < ki; i++) {
        int64_t next = flux_hamt_set(result, keys_arr[i], vals_arr[i]);
        /* `a` was only lent; every later map is this function's own. */
        if (result != a) flux_drop(result);
        result = next;
    }
    free(keys_arr);
    free(vals_arr);
    return result;
}

int64_t flux_hamt_keys(int64_t map) {
    HamtNode *root = node_of(map);
    if (!root) return flux_array_new(NULL, 0);
    uint32_t size = hamt_size_impl(root);
    if (size == 0) return flux_array_new(NULL, 0);

    int64_t *keys = (int64_t *)malloc(size * sizeof(int64_t));
    uint32_t idx = 0;
    hamt_collect_keys(root, keys, &idx);
    flux_dup_range(keys, idx);

    int64_t result = flux_array_new(keys, (int32_t)idx);
    free(keys);
    return result;
}

int flux_is_hamt(void *ptr) {
    return ptr && flux_obj_tag(ptr) == FLUX_OBJ_MAP;
}

/* Format a HAMT as {"key": value, ...} string. */
int64_t flux_hamt_format(int64_t map) {
    HamtNode *root = (HamtNode *)flux_untag_ptr(map);
    if (!root) return flux_string_new("{}", 2);
    uint32_t size = hamt_size_impl(root);
    if (size == 0) return flux_string_new("{}", 2);

    int64_t *keys_arr = (int64_t *)malloc(size * sizeof(int64_t));
    int64_t *vals_arr = (int64_t *)malloc(size * sizeof(int64_t));
    uint32_t ki = 0, vi = 0;
    hamt_collect_keys(root, keys_arr, &ki);
    hamt_collect_values(root, vals_arr, &vi);

    /* Sort key-value pairs by key string for deterministic output. */
    for (uint32_t i = 1; i < ki; i++) {
        for (uint32_t j = i; j > 0; j--) {
            int64_t ks_a = flux_to_string(keys_arr[j - 1]);
            int64_t ks_b = flux_to_string(keys_arr[j]);
            const char *sa = flux_string_data(ks_a);
            uint32_t la = flux_string_len(ks_a);
            const char *sb = flux_string_data(ks_b);
            uint32_t lb = flux_string_len(ks_b);
            uint32_t ml = la < lb ? la : lb;
            int cmp = memcmp(sa, sb, ml);
            flux_drop(ks_a);
            flux_drop(ks_b);
            if (cmp > 0 || (cmp == 0 && la > lb)) {
                int64_t tk = keys_arr[j - 1]; keys_arr[j - 1] = keys_arr[j]; keys_arr[j] = tk;
                int64_t tv = vals_arr[j - 1]; vals_arr[j - 1] = vals_arr[j]; vals_arr[j] = tv;
            } else {
                break;
            }
        }
    }

    char buf[4096];
    int pos = 0;
    pos += snprintf(buf + pos, sizeof(buf) - pos, "{");
    for (uint32_t i = 0; i < ki && pos < (int)sizeof(buf) - 40; i++) {
        if (i > 0) pos += snprintf(buf + pos, sizeof(buf) - pos, ", ");
        int64_t key = keys_arr[i];
        int is_str_key = flux_is_ptr(key) && flux_obj_tag(flux_untag_ptr(key)) == FLUX_OBJ_STRING;
        const char *kd;
        uint32_t kl;
        if (is_str_key) {
            kd = flux_string_data(key);
            kl = flux_string_len(key);
            buf[pos++] = '"';
            if (pos + kl < sizeof(buf) - 20) { memcpy(buf + pos, kd, kl); pos += kl; }
            buf[pos++] = '"';
        } else {
            int64_t ks = flux_to_string(key);
            kd = flux_string_data(ks);
            kl = flux_string_len(ks);
            if (pos + kl < sizeof(buf) - 20) { memcpy(buf + pos, kd, kl); pos += kl; }
            flux_drop(ks);
        }
        pos += snprintf(buf + pos, sizeof(buf) - pos, ": ");
        int64_t vval = vals_arr[i];
        int is_str_val = flux_is_ptr(vval) && flux_obj_tag(flux_untag_ptr(vval)) == FLUX_OBJ_STRING;
        const char *vd;
        uint32_t vl;
        int64_t vs = FLUX_NONE;
        if (is_str_val) {
            vd = flux_string_data(vval);
            vl = flux_string_len(vval);
            buf[pos++] = '"';
        } else {
            vs = flux_to_string(vval);
            vd = flux_string_data(vs);
            vl = flux_string_len(vs);
        }
        if (pos + vl < sizeof(buf) - 10) { memcpy(buf + pos, vd, vl); pos += vl; }
        if (is_str_val) buf[pos++] = '"';
        flux_drop(vs);
    }
    pos += snprintf(buf + pos, sizeof(buf) - pos, "}");

    free(keys_arr);
    free(vals_arr);
    return flux_string_new(buf, (uint32_t)pos);
}
