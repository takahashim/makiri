#include "compat.h"
#include "compat_internal.h"
#include "../core/mkr_core.h"

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
 * element tag ids are pointer values (see mkr_dom_index_build) and can't
 * key a dense array, so they are not indexed. */
#define MKR_TAG_INDEX_CAP LXB_TAG__LAST_ENTRY

typedef struct {
    lxb_dom_attr_t *attr;  /* key; NULL marks an empty slot */
    lxb_dom_node_t *owner; /* value: the owning element node */
} mkr_dom_index_attr_slot_t;

typedef struct {
    mkr_dom_index_attr_slot_t *slots;
    size_t cap;   /* power of two, or 0 when there are no attributes */
    size_t count;
    bool   built;

    /* tag id -> elements, document order (CSR). */
    lxb_dom_node_t **tag_nodes; /* tag_total entries, or NULL */
    size_t          *tag_off;   /* tag_max + 2 entries, or NULL */
    uintptr_t        tag_max;   /* highest tag id present */
    size_t           tag_total; /* element count (== len of tag_nodes) */
    bool             has_foreign; /* any element with ns != HTML */
} mkr_dom_index_t;

/* ------------------------------------------------------------------ */
/* tree walk + hashing                                                */
/* ------------------------------------------------------------------ */

/* Pre-order traversal helper mkr_dom_preorder_next lives in compat_internal.h
 * (shared with source_loc.c). */

/* Bucket index for an attribute key (mkr_ptr_hash spreads aligned pointers). */
static size_t
mkr_dom_index_attr_slot(const mkr_dom_index_t *idx, const lxb_dom_attr_t *attr)
{
    return (size_t)mkr_ptr_hash(attr) & (idx->cap - 1);
}

/* pow2-ceil sizing is shared: mkr_pow2_ceil (core/mkr_core.h). */

/* ------------------------------------------------------------------ */
/* build / lookup                                                     */
/* ------------------------------------------------------------------ */

static void
mkr_dom_index_attr_insert(mkr_dom_index_t *idx, lxb_dom_attr_t *attr,
                    lxb_dom_node_t *owner)
{
    size_t i = mkr_dom_index_attr_slot(idx, attr);
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
mkr_dom_index_build(mkr_dom_index_t *idx, lxb_dom_document_t *doc)
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
    mkr_dom_index_attr_slot_t *slots = NULL;
    size_t cap = 0;
    if (n_attrs > 0) {
        size_t need;
        if (!mkr_size_mul(n_attrs, 2, &need) || !mkr_pow2_ceil(need, &cap)) {
            return LXB_STATUS_ERROR_MEMORY_ALLOCATION;
        }
        if (cap < 8) cap = 8;
        slots = mkr_callocarray(cap, sizeof(*slots));
        if (slots == NULL) return LXB_STATUS_ERROR_MEMORY_ALLOCATION;
    }

    /* Size the tag CSR: prefix-sum the per-tag counts into offsets, plus a
     * cursor (copy of the offsets) used to scatter in document order. */
    lxb_dom_node_t **tag_nodes = NULL;
    size_t          *tag_off   = NULL;
    size_t          *cursor    = NULL;
    if (n_indexed > 0) {
        size_t noff = (size_t)tag_max + 2;
        size_t off_bytes;
        if (!mkr_size_mul(noff, sizeof(*cursor), &off_bytes)) {
            free(slots);
            return LXB_STATUS_ERROR_MEMORY_ALLOCATION;
        }
        tag_off = mkr_reallocarray(NULL, noff, sizeof(*tag_off));
        cursor  = mkr_reallocarray(NULL, noff, sizeof(*cursor));
        tag_nodes = mkr_reallocarray(NULL, n_indexed, sizeof(*tag_nodes));
        if (tag_off == NULL || cursor == NULL || tag_nodes == NULL) {
            free(slots); free(tag_off); free(cursor); free(tag_nodes);
            return LXB_STATUS_ERROR_MEMORY_ALLOCATION;
        }
        tag_off[0] = 0;
        for (uintptr_t t = 0; t <= tag_max; t++) {
            if (!mkr_size_add(tag_off[t], counts[t], &tag_off[t + 1])) {
                free(slots); free(tag_off); free(cursor); free(tag_nodes);
                return LXB_STATUS_ERROR_MEMORY_ALLOCATION;
            }
        }
        memcpy(cursor, tag_off, off_bytes);
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
            mkr_dom_index_attr_insert(idx, a, node);
            lxb_dom_interface_node(a)->parent = node;
        }
    }

    free(cursor);
    idx->built = true;
    return LXB_STATUS_OK;
}

static lxb_dom_node_t *
mkr_dom_index_attr_owner(const mkr_dom_index_t *idx, lxb_dom_attr_t *attr)
{
    if (idx->cap == 0) {
        return NULL;
    }
    size_t i = mkr_dom_index_attr_slot(idx, attr);
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
static mkr_dom_index_t *
mkr_dom_index_ensure(mkr_parsed_t *p)
{
    if (p == NULL || p->doc == NULL) {
        return NULL;
    }

    mkr_dom_index_t *idx = p->dom_index;
    if (idx == NULL) {
        idx = mkr_callocarray(1, sizeof(*idx));
        if (idx == NULL) {
            return NULL; /* fail closed */
        }
        p->dom_index = idx;
    }

    if (!idx->built) {
        if (mkr_dom_index_build(idx, (lxb_dom_document_t *)p->doc)
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
    mkr_dom_index_t *idx = mkr_dom_index_ensure(p);
    if (idx == NULL) {
        return NULL;
    }
    return mkr_dom_index_attr_owner(idx, attr);
}

int
mkr_parsed_dom_index_build(mkr_parsed_t *p)
{
    return mkr_dom_index_ensure(p) != NULL ? 0 : -1;
}

void
mkr_parsed_dom_index_invalidate(mkr_parsed_t *p)
{
    if (p == NULL) {
        return;
    }
    mkr_dom_index_free(p->dom_index);
    p->dom_index = NULL;
}

void
mkr_dom_index_free(void *ptr)
{
    if (ptr == NULL) {
        return;
    }
    mkr_dom_index_t *idx = ptr;
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
    return mkr_dom_index_ensure(p); /* same object as the attr->owner index */
}

lxb_dom_node_t *const *
mkr_element_index_tag(const void *ptr, lxb_tag_id_t tag_id, size_t *count)
{
    const mkr_dom_index_t *idx = ptr;
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
    const mkr_dom_index_t *idx = ptr;
    return (idx != NULL) ? (idx->has_foreign ? 1 : 0) : 1; /* fail safe: assume foreign */
}
