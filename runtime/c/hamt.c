/*
 * hamt.c — Persistent Hash Array Mapped Trie (HAMT).
 *
 * Matches the semantics of src/runtime/hamt.rs: 5-bit-per-level trie
 * with bitmap compression and hash collision nodes.
 *
 * All keys and values are NaN-boxed i64.  Keys are hashed via a simple
 * FNV-1a variant.  The HAMT is immutable (persistent): set/delete
 * return new nodes, sharing unchanged subtrees.
 *
 * Nodes are GC-allocated (flux_gc_alloc).
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

typedef struct HamtNode HamtNode;

/* Leaf: single key-value pair. */
typedef struct {
    uint64_t hash;
    int64_t  key;
    int64_t  value;
} HamtLeaf;

/* Branch: bitmap-compressed child array. */
typedef struct {
    uint32_t   bitmap;
    uint32_t   count;    /* popcount(bitmap) */
    HamtNode  *children; /* array of `count` HamtNode */
} HamtBranch;

/* Collision: list of entries sharing the same hash. */
typedef struct {
    uint64_t hash;
    uint32_t count;
    int64_t *keys;       /* array of count keys */
    int64_t *values;     /* array of count values */
} HamtCollision;

struct HamtNode {
    HamtKind kind;
    union {
        HamtLeaf      leaf;
        HamtBranch    branch;
        HamtCollision collision;
    } u;
};

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

static HamtNode *alloc_node(void) {
    HamtNode *n = (HamtNode *)flux_gc_alloc((uint32_t)sizeof(HamtNode));
    n->kind = HAMT_EMPTY;
    return n;
}

static HamtNode *make_leaf(uint64_t hash, int64_t key, int64_t value) {
    HamtNode *n = alloc_node();
    n->kind = HAMT_LEAF;
    n->u.leaf.hash  = hash;
    n->u.leaf.key   = key;
    n->u.leaf.value = value;
    return n;
}

static HamtNode *make_empty(void) {
    HamtNode *n = alloc_node();
    n->kind = HAMT_EMPTY;
    return n;
}

static HamtNode *copy_branch_insert(HamtBranch *br, uint32_t slot, HamtNode *child) {
    uint32_t idx = compressed_index(br->bitmap, slot);
    uint32_t new_count = br->count + 1;

    HamtNode *n = alloc_node();
    n->kind = HAMT_BRANCH;
    n->u.branch.bitmap = br->bitmap | (1u << slot);
    n->u.branch.count  = new_count;
    n->u.branch.children = (HamtNode *)flux_gc_alloc((uint32_t)(new_count * sizeof(HamtNode)));

    /* Copy children before idx, insert child, copy rest. */
    if (idx > 0) memcpy(&n->u.branch.children[0], &br->children[0], idx * sizeof(HamtNode));
    n->u.branch.children[idx] = *child;
    if (idx < br->count)
        memcpy(&n->u.branch.children[idx + 1], &br->children[idx], (br->count - idx) * sizeof(HamtNode));

    return n;
}

static HamtNode *copy_branch_replace(HamtBranch *br, uint32_t idx, HamtNode *child) {
    HamtNode *n = alloc_node();
    n->kind = HAMT_BRANCH;
    n->u.branch.bitmap = br->bitmap;
    n->u.branch.count  = br->count;
    n->u.branch.children = (HamtNode *)flux_gc_alloc((uint32_t)(br->count * sizeof(HamtNode)));
    memcpy(n->u.branch.children, br->children, br->count * sizeof(HamtNode));
    n->u.branch.children[idx] = *child;
    return n;
}

static HamtNode *copy_branch_remove(HamtBranch *br, uint32_t slot) {
    uint32_t idx = compressed_index(br->bitmap, slot);
    uint32_t new_count = br->count - 1;

    if (new_count == 0) return make_empty();

    HamtNode *n = alloc_node();
    n->kind = HAMT_BRANCH;
    n->u.branch.bitmap = br->bitmap & ~(1u << slot);
    n->u.branch.count  = new_count;
    n->u.branch.children = (HamtNode *)flux_gc_alloc((uint32_t)(new_count * sizeof(HamtNode)));
    if (idx > 0) memcpy(&n->u.branch.children[0], &br->children[0], idx * sizeof(HamtNode));
    if (idx < br->count - 1)
        memcpy(&n->u.branch.children[idx], &br->children[idx + 1], (br->count - 1 - idx) * sizeof(HamtNode));
    return n;
}

/* ── Two leaves → branch or collision ───────────────────────────────── */

static HamtNode *merge_leaves(HamtLeaf *a, HamtLeaf *b, uint32_t depth) {
    if (depth >= MAX_DEPTH || a->hash == b->hash) {
        /* Hash collision. */
        HamtNode *n = alloc_node();
        n->kind = HAMT_COLLISION;
        n->u.collision.hash  = a->hash;
        n->u.collision.count = 2;
        n->u.collision.keys   = (int64_t *)flux_gc_alloc(2 * sizeof(int64_t));
        n->u.collision.values = (int64_t *)flux_gc_alloc(2 * sizeof(int64_t));
        n->u.collision.keys[0]   = a->key;
        n->u.collision.values[0] = a->value;
        n->u.collision.keys[1]   = b->key;
        n->u.collision.values[1] = b->value;
        return n;
    }

    uint32_t slot_a = slot_at_depth(a->hash, depth);
    uint32_t slot_b = slot_at_depth(b->hash, depth);

    HamtNode *n = alloc_node();
    n->kind = HAMT_BRANCH;

    if (slot_a != slot_b) {
        n->u.branch.bitmap = (1u << slot_a) | (1u << slot_b);
        n->u.branch.count  = 2;
        n->u.branch.children = (HamtNode *)flux_gc_alloc(2 * sizeof(HamtNode));
        /* Order by slot index. */
        uint32_t idx_a = compressed_index(n->u.branch.bitmap, slot_a);
        uint32_t idx_b = compressed_index(n->u.branch.bitmap, slot_b);
        HamtNode *leaf_a = make_leaf(a->hash, a->key, a->value);
        HamtNode *leaf_b = make_leaf(b->hash, b->key, b->value);
        n->u.branch.children[idx_a] = *leaf_a;
        n->u.branch.children[idx_b] = *leaf_b;
    } else {
        /* Same slot — recurse deeper. */
        HamtNode *merged = merge_leaves(a, b, depth + 1);
        n->u.branch.bitmap = 1u << slot_a;
        n->u.branch.count  = 1;
        n->u.branch.children = (HamtNode *)flux_gc_alloc(sizeof(HamtNode));
        n->u.branch.children[0] = *merged;
    }
    return n;
}

/* ── Lookup ─────────────────────────────────────────────────────────── */

static int64_t *hamt_get_impl(HamtNode *node, uint64_t hash, int64_t key, uint32_t depth) {
    switch (node->kind) {
    case HAMT_EMPTY:
        return NULL;

    case HAMT_LEAF:
        if (keys_equal(node->u.leaf.key, key))
            return &node->u.leaf.value;
        return NULL;

    case HAMT_BRANCH: {
        uint32_t slot = slot_at_depth(hash, depth);
        if (!(node->u.branch.bitmap & (1u << slot))) return NULL;
        uint32_t idx = compressed_index(node->u.branch.bitmap, slot);
        return hamt_get_impl(&node->u.branch.children[idx], hash, key, depth + 1);
    }

    case HAMT_COLLISION: {
        for (uint32_t i = 0; i < node->u.collision.count; i++) {
            if (keys_equal(node->u.collision.keys[i], key))
                return &node->u.collision.values[i];
        }
        return NULL;
    }
    }
    return NULL;
}

/* ── Insert ─────────────────────────────────────────────────────────── */

static HamtNode *hamt_set_impl(HamtNode *node, uint64_t hash, int64_t key, int64_t value, uint32_t depth) {
    switch (node->kind) {
    case HAMT_EMPTY:
        return make_leaf(hash, key, value);

    case HAMT_LEAF: {
        if (keys_equal(node->u.leaf.key, key)) {
            /* Update existing key. */
            return make_leaf(hash, key, value);
        }
        /* Split into branch or collision. */
        HamtLeaf new_leaf = { hash, key, value };
        return merge_leaves(&node->u.leaf, &new_leaf, depth);
    }

    case HAMT_BRANCH: {
        uint32_t slot = slot_at_depth(hash, depth);
        if (node->u.branch.bitmap & (1u << slot)) {
            /* Slot occupied — recurse. */
            uint32_t idx = compressed_index(node->u.branch.bitmap, slot);
            HamtNode *updated = hamt_set_impl(&node->u.branch.children[idx], hash, key, value, depth + 1);
            return copy_branch_replace(&node->u.branch, idx, updated);
        } else {
            /* Insert new child. */
            HamtNode *leaf = make_leaf(hash, key, value);
            return copy_branch_insert(&node->u.branch, slot, leaf);
        }
    }

    case HAMT_COLLISION: {
        /* Check if key exists in collision list. */
        for (uint32_t i = 0; i < node->u.collision.count; i++) {
            if (keys_equal(node->u.collision.keys[i], key)) {
                /* Replace value. */
                HamtNode *n = alloc_node();
                n->kind = HAMT_COLLISION;
                n->u.collision.hash  = node->u.collision.hash;
                n->u.collision.count = node->u.collision.count;
                n->u.collision.keys   = (int64_t *)flux_gc_alloc(n->u.collision.count * sizeof(int64_t));
                n->u.collision.values = (int64_t *)flux_gc_alloc(n->u.collision.count * sizeof(int64_t));
                memcpy(n->u.collision.keys, node->u.collision.keys, n->u.collision.count * sizeof(int64_t));
                memcpy(n->u.collision.values, node->u.collision.values, n->u.collision.count * sizeof(int64_t));
                n->u.collision.values[i] = value;
                return n;
            }
        }
        /* Add new entry. */
        uint32_t nc = node->u.collision.count + 1;
        HamtNode *n = alloc_node();
        n->kind = HAMT_COLLISION;
        n->u.collision.hash  = node->u.collision.hash;
        n->u.collision.count = nc;
        n->u.collision.keys   = (int64_t *)flux_gc_alloc(nc * sizeof(int64_t));
        n->u.collision.values = (int64_t *)flux_gc_alloc(nc * sizeof(int64_t));
        memcpy(n->u.collision.keys, node->u.collision.keys, node->u.collision.count * sizeof(int64_t));
        memcpy(n->u.collision.values, node->u.collision.values, node->u.collision.count * sizeof(int64_t));
        n->u.collision.keys[node->u.collision.count]   = key;
        n->u.collision.values[node->u.collision.count] = value;
        return n;
    }
    }
    return node; /* unreachable */
}

/* ── Delete ─────────────────────────────────────────────────────────── */

static HamtNode *hamt_delete_impl(HamtNode *node, uint64_t hash, int64_t key, uint32_t depth) {
    switch (node->kind) {
    case HAMT_EMPTY:
        return node;

    case HAMT_LEAF:
        if (keys_equal(node->u.leaf.key, key))
            return make_empty();
        return node;

    case HAMT_BRANCH: {
        uint32_t slot = slot_at_depth(hash, depth);
        if (!(node->u.branch.bitmap & (1u << slot))) return node;
        uint32_t idx = compressed_index(node->u.branch.bitmap, slot);
        HamtNode *updated = hamt_delete_impl(&node->u.branch.children[idx], hash, key, depth + 1);
        if (updated->kind == HAMT_EMPTY) {
            HamtNode *result = copy_branch_remove(&node->u.branch, slot);
            /* If only one child left and it's a leaf, collapse. */
            if (result->kind == HAMT_BRANCH && result->u.branch.count == 1
                && result->u.branch.children[0].kind == HAMT_LEAF) {
                return &result->u.branch.children[0];
            }
            return result;
        }
        return copy_branch_replace(&node->u.branch, idx, updated);
    }

    case HAMT_COLLISION: {
        for (uint32_t i = 0; i < node->u.collision.count; i++) {
            if (keys_equal(node->u.collision.keys[i], key)) {
                if (node->u.collision.count == 1) return make_empty();
                if (node->u.collision.count == 2) {
                    /* Collapse to leaf. */
                    uint32_t other = (i == 0) ? 1 : 0;
                    return make_leaf(node->u.collision.hash,
                                     node->u.collision.keys[other],
                                     node->u.collision.values[other]);
                }
                uint32_t nc = node->u.collision.count - 1;
                HamtNode *n = alloc_node();
                n->kind = HAMT_COLLISION;
                n->u.collision.hash  = node->u.collision.hash;
                n->u.collision.count = nc;
                n->u.collision.keys   = (int64_t *)flux_gc_alloc(nc * sizeof(int64_t));
                n->u.collision.values = (int64_t *)flux_gc_alloc(nc * sizeof(int64_t));
                uint32_t j = 0;
                for (uint32_t k = 0; k < node->u.collision.count; k++) {
                    if (k == i) continue;
                    n->u.collision.keys[j]   = node->u.collision.keys[k];
                    n->u.collision.values[j] = node->u.collision.values[k];
                    j++;
                }
                return n;
            }
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
    case HAMT_COLLISION: return node->u.collision.count;
    case HAMT_BRANCH: {
        uint32_t total = 0;
        for (uint32_t i = 0; i < node->u.branch.count; i++)
            total += hamt_size_impl(&node->u.branch.children[i]);
        return total;
    }
    }
    return 0;
}

/* ── Public API (NaN-boxed) ─────────────────────────────────────────── */

int64_t flux_hamt_empty(void) {
    HamtNode *root = make_empty();
    return flux_tag_ptr(root);
}

int64_t flux_hamt_get(int64_t map, int64_t key) {
    HamtNode *root = (HamtNode *)flux_untag_ptr(map);
    if (!root) return flux_make_none();
    uint64_t hash = hamt_hash(key);
    int64_t *val = hamt_get_impl(root, hash, key, 0);
    if (!val) return flux_make_none();
    return *val;
}

int64_t flux_hamt_set(int64_t map, int64_t key, int64_t value) {
    HamtNode *root = (HamtNode *)flux_untag_ptr(map);
    if (!root) root = make_empty();
    uint64_t hash = hamt_hash(key);
    HamtNode *new_root = hamt_set_impl(root, hash, key, value, 0);
    return flux_tag_ptr(new_root);
}

int64_t flux_hamt_delete(int64_t map, int64_t key) {
    HamtNode *root = (HamtNode *)flux_untag_ptr(map);
    if (!root) return map;
    uint64_t hash = hamt_hash(key);
    HamtNode *new_root = hamt_delete_impl(root, hash, key, 0);
    return flux_tag_ptr(new_root);
}

int64_t flux_hamt_contains(int64_t map, int64_t key) {
    HamtNode *root = (HamtNode *)flux_untag_ptr(map);
    if (!root) return flux_make_bool(0);
    uint64_t hash = hamt_hash(key);
    int64_t *val = hamt_get_impl(root, hash, key, 0);
    return flux_make_bool(val != NULL);
}

int64_t flux_hamt_size(int64_t map) {
    HamtNode *root = (HamtNode *)flux_untag_ptr(map);
    if (!root) return flux_tag_int(0);
    return flux_tag_int((int64_t)hamt_size_impl(root));
}

/* ── Collect all keys into a flat array ─────────────────────────────── */

static void hamt_collect_keys(HamtNode *node, int64_t *out, uint32_t *idx) {
    switch (node->kind) {
    case HAMT_EMPTY:
        break;
    case HAMT_LEAF:
        out[*idx] = node->u.leaf.key;
        (*idx)++;
        break;
    case HAMT_COLLISION:
        for (uint32_t i = 0; i < node->u.collision.count; i++) {
            out[*idx] = node->u.collision.keys[i];
            (*idx)++;
        }
        break;
    case HAMT_BRANCH:
        for (uint32_t i = 0; i < node->u.branch.count; i++) {
            hamt_collect_keys(&node->u.branch.children[i], out, idx);
        }
        break;
    }
}

static void hamt_collect_values(HamtNode *node, int64_t *out, uint32_t *idx) {
    switch (node->kind) {
    case HAMT_EMPTY: break;
    case HAMT_LEAF:
        out[*idx] = node->u.leaf.value;
        (*idx)++;
        break;
    case HAMT_COLLISION:
        for (uint32_t i = 0; i < node->u.collision.count; i++) {
            out[*idx] = node->u.collision.values[i];
            (*idx)++;
        }
        break;
    case HAMT_BRANCH:
        for (uint32_t i = 0; i < node->u.branch.count; i++) {
            hamt_collect_values(&node->u.branch.children[i], out, idx);
        }
        break;
    }
}

int64_t flux_hamt_values(int64_t map) {
    HamtNode *root = (HamtNode *)flux_untag_ptr(map);
    if (!root) return flux_array_new(NULL, 0);
    uint32_t size = hamt_size_impl(root);
    if (size == 0) return flux_array_new(NULL, 0);
    int64_t *vals = (int64_t *)malloc(size * sizeof(int64_t));
    uint32_t idx = 0;
    hamt_collect_values(root, vals, &idx);
    int64_t result = flux_array_new(vals, (int32_t)idx);
    free(vals);
    return result;
}

/* Merge two HAMTs: keys from b override keys from a. */
int64_t flux_hamt_merge(int64_t a, int64_t b) {
    HamtNode *root_b = (HamtNode *)flux_untag_ptr(b);
    if (!root_b) return a;
    uint32_t size_b = hamt_size_impl(root_b);
    if (size_b == 0) return a;

    /* Collect all key-value pairs from b, then set them into a. */
    int64_t *keys_arr = (int64_t *)malloc(size_b * sizeof(int64_t));
    int64_t *vals_arr = (int64_t *)malloc(size_b * sizeof(int64_t));
    uint32_t ki = 0, vi = 0;
    hamt_collect_keys(root_b, keys_arr, &ki);
    hamt_collect_values(root_b, vals_arr, &vi);

    int64_t result = a;
    for (uint32_t i = 0; i < ki; i++) {
        result = flux_hamt_set(result, keys_arr[i], vals_arr[i]);
    }
    free(keys_arr);
    free(vals_arr);
    return result;
}

int64_t flux_hamt_keys(int64_t map) {
    HamtNode *root = (HamtNode *)flux_untag_ptr(map);
    if (!root) return flux_array_new(NULL, 0);
    uint32_t size = hamt_size_impl(root);
    if (size == 0) return flux_array_new(NULL, 0);

    int64_t *keys = (int64_t *)malloc(size * sizeof(int64_t));
    uint32_t idx = 0;
    hamt_collect_keys(root, keys, &idx);

    int64_t result = flux_array_new(keys, (int32_t)idx);
    free(keys);
    return result;
}

/* Returns 1 if the pointer looks like a HamtNode.
 *
 * Heuristic: HAMT kind is 0-3 (first i32). To distinguish from ADTs
 * (which also use small ctor_tags), check the second i32:
 *   - ADT: second i32 is field_count (0-100, small)
 *   - HAMT_LEAF: second i32 is the low 32 bits of a hash (typically large)
 *   - HAMT_BRANCH: second i32 is a bitmap (any value)
 *   - HAMT_EMPTY: second i32 is 0 (same as ADT with 0 fields)
 *
 * For HAMT_EMPTY we check that the entire union is zeroed (16+ bytes).
 * For HAMT_LEAF, the hash field spans bytes 4-11 — if the upper 32 bits
 * are non-zero, it's almost certainly a hash, not a field_count.
 */
int flux_is_hamt(void *ptr) {
    if (!ptr) return 0;
    /* All objects (including HAMT nodes) are allocated via flux_gc_alloc
     * which calls flux_gc_alloc_header(size, 0, 0).  So FluxHeader is
     * always valid.  HAMT nodes have obj_tag=0; known types have 0xF1-0xF6. */
    uint8_t obj = flux_obj_tag(ptr);
    if (obj == FLUX_OBJ_ADT || obj == FLUX_OBJ_TUPLE || obj == FLUX_OBJ_STRING
        || obj == FLUX_OBJ_ARRAY || obj == FLUX_OBJ_CLOSURE || obj == FLUX_OBJ_BIGINT) {
        return 0;
    }
    int32_t kind = *(int32_t *)ptr;
    if (kind < 0 || kind > HAMT_COLLISION) return 0;
    if (kind == HAMT_EMPTY) {
        /* HAMT_EMPTY: union should be zeroed. Peek at bytes 8-15. */
        uint64_t probe = *(uint64_t *)((char *)ptr + 8);
        return probe == 0;
    }
    if (kind == HAMT_LEAF) {
        /* HAMT_LEAF: bytes 4-11 are a uint64_t hash.
         * If upper 32 bits are non-zero, it's a hash not a field_count. */
        uint32_t hash_hi = *(uint32_t *)((char *)ptr + 8);
        return hash_hi != 0;
    }
    /* HAMT_BRANCH or HAMT_COLLISION: second i32 is bitmap/count.
     * ADTs with ctor_tag 2-3 have field_count in second i32.
     * Branch bitmap is typically non-trivial. Accept if kind matches. */
    return 1;
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
        }
        pos += snprintf(buf + pos, sizeof(buf) - pos, ": ");
        int64_t vval = vals_arr[i];
        int is_str_val = flux_is_ptr(vval) && flux_obj_tag(flux_untag_ptr(vval)) == FLUX_OBJ_STRING;
        const char *vd;
        uint32_t vl;
        if (is_str_val) {
            vd = flux_string_data(vval);
            vl = flux_string_len(vval);
            buf[pos++] = '"';
        } else {
            int64_t vs = flux_to_string(vval);
            vd = flux_string_data(vs);
            vl = flux_string_len(vs);
        }
        if (pos + vl < sizeof(buf) - 10) { memcpy(buf + pos, vd, vl); pos += vl; }
        if (is_str_val) buf[pos++] = '"';
    }
    pos += snprintf(buf + pos, sizeof(buf) - pos, "}");

    free(keys_arr);
    free(vals_arr);
    return flux_string_new(buf, (uint32_t)pos);
}
