#include "compat.h"
#include "compat_internal.h"
#include "../core/mkr_safe.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <lexbor/ns/const.h>
#include <lexbor/tag/const.h>

/*
 * Per-document indices, all built in one walk of the tree (two tree passes:
 * count/size, then fill — so each table is sized once and never rehashes
 * mid-build, which keeps the OOM path trivial).
 *
 *  - attribute -> owner element: an open-addressing hash keyed on the
 *    lxb_dom_attr_t* pointer (pointer keys, no deletion, linear probing).
 *  - tag id -> elements (CSR layout): a flat `tag_nodes` array of every
 *    element grouped by tag id and kept in document order, with `tag_off`
 *    prefix-sum offsets so bucket t is tag_nodes[tag_off[t] .. tag_off[t+1]).
 *    Lets the XPath engine answer `//tag` from the document without walking.
 */

/* Tag-index buckets cover only Lexbor's static tag-id range [1, this). Custom
 * element tag ids are pointer values (see mkr_attr_owner_idx_build) and can't
 * key a dense array, so they are not indexed. */
#define MKR_TAG_INDEX_CAP LXB_TAG__LAST_ENTRY

typedef struct {
    lxb_dom_attr_t *attr;  /* key; NULL marks an empty slot */
    lxb_dom_node_t *owner; /* value: the owning element node */
} mkr_attr_owner_slot_t;

typedef struct {
    mkr_attr_owner_slot_t *slots;
    size_t cap;   /* power of two, or 0 when there are no attributes */
    size_t count;
    bool   built;

    /* tag id -> elements, document order (CSR). */
    lxb_dom_node_t **tag_nodes; /* tag_total entries, or NULL */
    size_t          *tag_off;   /* tag_max + 2 entries, or NULL */
    uintptr_t        tag_max;   /* highest tag id present */
    size_t           tag_total; /* element count (== len of tag_nodes) */
    bool             has_foreign; /* any element with ns != HTML */
} mkr_attr_owner_idx_t;

/* ------------------------------------------------------------------ */
/* tree walk + hashing                                                */
/* ------------------------------------------------------------------ */

/* Pre-order traversal helper mkr_dom_preorder_next lives in compat_internal.h
 * (shared with source_loc.c). */

/* Mix a pointer into a well-distributed bucket index. Pointers are aligned, so
 * the low bits carry little entropy; fmix64 (the finalizer from MurmurHash3)
 * spreads them across the table. */
static size_t
mkr_attr_slot_for(const mkr_attr_owner_idx_t *idx, const lxb_dom_attr_t *attr)
{
    uint64_t h = (uint64_t)(uintptr_t)attr;
    h ^= h >> 33;
    h *= 0xff51afd7ed558ccdULL;
    h ^= h >> 33;
    h *= 0xc4ceb9fe1a85ec53ULL;
    h ^= h >> 33;
    return (size_t)h & (idx->cap - 1);
}

static size_t
mkr_pow2_ceil(size_t n)
{
    size_t p = 1;
    while (p < n) {
        size_t np = p << 1;
        if (np <= p) {
            return p; /* saturate rather than wrap on overflow */
        }
        p = np;
    }
    return p;
}

/* ------------------------------------------------------------------ */
/* build / lookup                                                     */
/* ------------------------------------------------------------------ */

static void
mkr_attr_idx_insert(mkr_attr_owner_idx_t *idx, lxb_dom_attr_t *attr,
                    lxb_dom_node_t *owner)
{
    size_t i = mkr_attr_slot_for(idx, attr);
    while (idx->slots[i].attr != NULL) {
        if (idx->slots[i].attr == attr) {
            return; /* already mapped (shouldn't happen; an attr has one owner) */
        }
        i = (i + 1) & (idx->cap - 1);
    }
    idx->slots[i].attr  = attr;
    idx->slots[i].owner = owner;
    idx->count++;
}

/* Build the indices by walking +doc+. Records every (attr -> element) and
 * groups elements by tag id in document order. Two tree passes: phase 1 sizes
 * everything (attr count, per-tag element counts, max tag id, foreign flag);
 * phase 2 fills. On allocation failure returns
 * LXB_STATUS_ERROR_MEMORY_ALLOCATION with the index left unbuilt, so the caller
 * fails closed and can retry. */
static lxb_status_t
mkr_attr_owner_idx_build(mkr_attr_owner_idx_t *idx, lxb_dom_document_t *doc)
{
    lxb_dom_node_t *root = lxb_dom_interface_node(doc);

    /* Phase 1: one walk to size everything. Only *standard* HTML tags are
     * indexed by tag id: Lexbor's static tag ids are the small dense range
     * [1, LXB_TAG__LAST_ENTRY); a custom element's tag id is the *pointer* to
     * its interned tag data (lxb_tag_append: `data->tag_id = (lxb_tag_id_t)
     * data`), an enormous value that can't key a dense array. Elements with a
     * tag id at/above the cap are left out of the index — the engine falls
     * back to a tree walk for `//customtag`, which is rare in practice. */
    size_t   counts[MKR_TAG_INDEX_CAP] = {0}; /* counts[t] = #elements, tag t */
    size_t   n_attrs = 0;
    size_t   n_indexed = 0;       /* elements with a standard (capped) tag id */
    uintptr_t tag_max = 0;
    bool     has_foreign = false;
    for (lxb_dom_node_t *node = root; node != NULL;
         node = mkr_dom_preorder_next(node, root)) {
        if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
            continue;
        }
        if (node->ns != LXB_NS_HTML) {
            has_foreign = true;
        }
        uintptr_t tag = node->local_name; /* tag id for an element */
        if (tag != LXB_TAG__UNDEF && tag < MKR_TAG_INDEX_CAP) {
            counts[tag]++;
            n_indexed++;
            if (tag > tag_max) tag_max = tag;
        }
        for (lxb_dom_attr_t *a =
                 lxb_dom_element_first_attribute(lxb_dom_interface_element(node));
             a != NULL; a = lxb_dom_element_next_attribute(a)) {
            n_attrs++;
        }
    }

    /* Size the attr->owner table (load factor <= 0.5). */
    mkr_attr_owner_slot_t *slots = NULL;
    size_t cap = 0;
    if (n_attrs > 0) {
        cap = mkr_pow2_ceil(n_attrs * 2);
        if (cap < 8) cap = 8;
        slots = calloc(cap, sizeof(*slots));
        if (slots == NULL) return LXB_STATUS_ERROR_MEMORY_ALLOCATION;
    }

    /* Size the tag CSR: prefix-sum the per-tag counts into offsets, plus a
     * cursor (copy of the offsets) used to scatter in document order. */
    lxb_dom_node_t **tag_nodes = NULL;
    size_t          *tag_off   = NULL;
    size_t          *cursor    = NULL;
    if (n_indexed > 0) {
        size_t noff = (size_t)tag_max + 2;
        tag_off = mkr_reallocarray(NULL, noff, sizeof(*tag_off));
        cursor  = mkr_reallocarray(NULL, noff, sizeof(*cursor));
        tag_nodes = mkr_reallocarray(NULL, n_indexed, sizeof(*tag_nodes));
        if (tag_off == NULL || cursor == NULL || tag_nodes == NULL) {
            free(slots); free(tag_off); free(cursor); free(tag_nodes);
            return LXB_STATUS_ERROR_MEMORY_ALLOCATION;
        }
        tag_off[0] = 0;
        for (uintptr_t t = 0; t <= tag_max; t++) {
            tag_off[t + 1] = tag_off[t] + counts[t];
        }
        memcpy(cursor, tag_off, noff * sizeof(*cursor));
    }

    idx->slots       = slots;
    idx->cap         = cap;
    idx->count       = 0;
    idx->tag_nodes   = tag_nodes;
    idx->tag_off     = tag_off;
    idx->tag_max     = tag_max;
    idx->tag_total   = n_indexed;
    idx->has_foreign = has_foreign;

    /* Phase 2: fill. Scatter each standard-tag element into its bucket
     * (document order preserved by the cursor), insert its attributes, and
     * backfill each attribute node's parent to its owner element. Lexbor
     * leaves attr->node.parent NULL; setting it to the semantically-correct
     * owner is safe (Lexbor walks the tree via first_child/next and attributes
     * never appear in that chain) and lets the native XPath engine read
     * node->parent for the parent/ancestor axes and document-order sorting
     * uniformly. */
    for (lxb_dom_node_t *node = root; node != NULL;
         node = mkr_dom_preorder_next(node, root)) {
        if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
            continue;
        }
        uintptr_t tag = node->local_name;
        if (tag != LXB_TAG__UNDEF && tag < MKR_TAG_INDEX_CAP) {
            tag_nodes[cursor[tag]++] = node;
        }
        for (lxb_dom_attr_t *a =
                 lxb_dom_element_first_attribute(lxb_dom_interface_element(node));
             a != NULL; a = lxb_dom_element_next_attribute(a)) {
            mkr_attr_idx_insert(idx, a, node);
            lxb_dom_interface_node(a)->parent = node;
        }
    }

    free(cursor);
    idx->built = true;
    return LXB_STATUS_OK;
}

static lxb_dom_node_t *
mkr_attr_owner_idx_lookup(const mkr_attr_owner_idx_t *idx, lxb_dom_attr_t *attr)
{
    if (idx->cap == 0) {
        return NULL;
    }
    size_t i = mkr_attr_slot_for(idx, attr);
    for (;;) {
        if (idx->slots[i].attr == NULL) {
            return NULL;
        }
        if (idx->slots[i].attr == attr) {
            return idx->slots[i].owner;
        }
        i = (i + 1) & (idx->cap - 1);
    }
}

/* ------------------------------------------------------------------ */
/* public compat API                                                  */
/* ------------------------------------------------------------------ */

/* Lazily allocate + build the index for +p+. Returns the built index, or NULL
 * on allocation failure (fail-closed; the cache is left empty so a later call
 * retries). */
static mkr_attr_owner_idx_t *
mkr_attr_owner_ensure(mkr_parsed_t *p)
{
    if (p == NULL || p->doc == NULL) {
        return NULL;
    }

    mkr_attr_owner_idx_t *idx = p->attr_owner;
    if (idx == NULL) {
        idx = calloc(1, sizeof(*idx));
        if (idx == NULL) {
            return NULL; /* fail closed */
        }
        p->attr_owner = idx;
    }

    if (!idx->built) {
        if (mkr_attr_owner_idx_build(idx, (lxb_dom_document_t *)p->doc)
                != LXB_STATUS_OK) {
            return NULL; /* leave unbuilt; a later call retries */
        }
    }
    return idx;
}

lxb_dom_node_t *
mkr_parsed_attr_owner(mkr_parsed_t *p, lxb_dom_attr_t *attr)
{
    if (attr == NULL) {
        return NULL;
    }
    mkr_attr_owner_idx_t *idx = mkr_attr_owner_ensure(p);
    if (idx == NULL) {
        return NULL;
    }
    return mkr_attr_owner_idx_lookup(idx, attr);
}

int
mkr_parsed_attr_index_build(mkr_parsed_t *p)
{
    return mkr_attr_owner_ensure(p) != NULL ? 0 : -1;
}

void
mkr_parsed_attr_index_invalidate(mkr_parsed_t *p)
{
    if (p == NULL) {
        return;
    }
    mkr_attr_owner_free(p->attr_owner);
    p->attr_owner = NULL;
}

void
mkr_attr_owner_free(void *ptr)
{
    if (ptr == NULL) {
        return;
    }
    mkr_attr_owner_idx_t *idx = ptr;
    free(idx->slots);
    free(idx->tag_nodes);
    free(idx->tag_off);
    free(idx);
}

/* ------------------------------------------------------------------ */
/* element index (tag id -> elements)                                 */
/* ------------------------------------------------------------------ */

void *
mkr_parsed_element_index(mkr_parsed_t *p)
{
    return mkr_attr_owner_ensure(p); /* same object as the attr->owner index */
}

lxb_dom_node_t *const *
mkr_element_index_tag(const void *ptr, lxb_tag_id_t tag_id, size_t *count)
{
    const mkr_attr_owner_idx_t *idx = ptr;
    if (idx == NULL || idx->tag_nodes == NULL
        || tag_id == LXB_TAG__UNDEF || (uintptr_t)tag_id >= MKR_TAG_INDEX_CAP
        || (uintptr_t)tag_id > idx->tag_max) {
        if (count) *count = 0;
        return NULL;
    }
    size_t start = idx->tag_off[tag_id];
    size_t end   = idx->tag_off[tag_id + 1];
    if (count) *count = end - start;
    return (end > start) ? &idx->tag_nodes[start] : NULL;
}

int
mkr_element_index_has_foreign(const void *ptr)
{
    const mkr_attr_owner_idx_t *idx = ptr;
    return (idx != NULL) ? (idx->has_foreign ? 1 : 0) : 1; /* fail safe: assume foreign */
}
