#include "compat.h"

#include <stdint.h>
#include <stdlib.h>

/*
 * attribute -> owner element index.
 *
 * An open-addressing hash table keyed on the lxb_dom_attr_t* pointer, mapping
 * each attribute to the element node that owns it. Built in a single pass over
 * the document (two-phase: count, then fill — so the table is sized once and
 * never rehashes mid-build, which keeps the OOM path trivial). Pointer keys,
 * no deletion, linear probing.
 */

typedef struct {
    lxb_dom_attr_t *attr;  /* key; NULL marks an empty slot */
    lxb_dom_node_t *owner; /* value: the owning element node */
} mkr_attr_owner_slot_t;

typedef struct {
    mkr_attr_owner_slot_t *slots;
    size_t cap;   /* power of two, or 0 when there are no attributes */
    size_t count;
    bool   built;
} mkr_attr_owner_idx_t;

/* ------------------------------------------------------------------ */
/* tree walk + hashing                                                */
/* ------------------------------------------------------------------ */

/* Pre-order successor of +node+ within the subtree rooted at +root+, or NULL
 * at the end. Iterative (no recursion) so a deeply nested document cannot blow
 * the C stack. */
static lxb_dom_node_t *
mkr_dom_preorder_next(lxb_dom_node_t *node, lxb_dom_node_t *root)
{
    if (node->first_child != NULL) {
        return node->first_child;
    }
    while (node != root && node->next == NULL) {
        node = node->parent;
    }
    if (node == root) {
        return NULL;
    }
    return node->next;
}

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

/* Build the index by walking +doc+ and recording every (attr -> element). On
 * allocation failure returns LXB_STATUS_ERROR_MEMORY_ALLOCATION with the index
 * left unbuilt (slots == NULL), so the caller fails closed and can retry. */
static lxb_status_t
mkr_attr_owner_idx_build(mkr_attr_owner_idx_t *idx, lxb_dom_document_t *doc)
{
    lxb_dom_node_t *root = lxb_dom_interface_node(doc);

    /* Phase 1: count attributes so the table is sized once. */
    size_t n_attrs = 0;
    for (lxb_dom_node_t *node = root; node != NULL;
         node = mkr_dom_preorder_next(node, root)) {
        if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
            continue;
        }
        for (lxb_dom_attr_t *a =
                 lxb_dom_element_first_attribute(lxb_dom_interface_element(node));
             a != NULL; a = lxb_dom_element_next_attribute(a)) {
            n_attrs++;
        }
    }

    if (n_attrs == 0) {
        idx->slots = NULL;
        idx->cap   = 0;
        idx->count = 0;
        idx->built = true;
        return LXB_STATUS_OK;
    }

    /* Target load factor <= 0.5: cap = next power of two >= 2 * n_attrs. */
    size_t cap = mkr_pow2_ceil(n_attrs * 2);
    if (cap < 8) {
        cap = 8;
    }

    mkr_attr_owner_slot_t *slots = calloc(cap, sizeof(*slots));
    if (slots == NULL) {
        return LXB_STATUS_ERROR_MEMORY_ALLOCATION; /* fail closed */
    }

    idx->slots = slots;
    idx->cap   = cap;
    idx->count = 0;

    /* Phase 2: fill. We also backfill the attribute node's parent pointer to
     * its owner element. Lexbor leaves attr->node.parent NULL; setting it to
     * the semantically-correct owner is safe (Lexbor walks the tree via
     * first_child/next and attributes never appear in that chain, so no cycle
     * or stray child is introduced) and lets the native XPath engine — which
     * reads node->parent for the parent/ancestor axes and document-order
     * sorting — treat attributes uniformly with no special-casing. */
    for (lxb_dom_node_t *node = root; node != NULL;
         node = mkr_dom_preorder_next(node, root)) {
        if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
            continue;
        }
        for (lxb_dom_attr_t *a =
                 lxb_dom_element_first_attribute(lxb_dom_interface_element(node));
             a != NULL; a = lxb_dom_element_next_attribute(a)) {
            mkr_attr_idx_insert(idx, a, node);
            lxb_dom_interface_node(a)->parent = node;
        }
    }

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
    free(idx);
}
