#include "compat.h"
#include "compat_internal.h"
#include "../core/mkr_core.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/*
 * Per-document text-extraction index (lexbor_compat/text_index.c).
 *
 * Descendant-text aggregation (Node#text, XPath string-value) walks every node
 * of a subtree chasing pointers through Lexbor's 96-byte nodes - at scale this
 * is cache-bound and dominates the cost. This index removes the per-extraction
 * walk: one build pass records, in document order, a flat array of every
 * TEXT/CDATA node's byte slice, plus a pointer-keyed map from each element /
 * fragment to the contiguous [start, end) run of slices its subtree owns. A
 * later Node#text is then a hash lookup + a single pre-sized memcpy run over a
 * cache-dense slice array, touching no element nodes at all.
 *
 * Built lazily on the first text query and cached on mkr_parsed_t.text_index.
 * Invalidated (dropped) by every mutation through the same single hook as the
 * attr/element index, so a cached slice can never point at reallocated or
 * detached text storage: any structural / content / attribute change rebuilds
 * it on the next query. The slices borrow into each text node's lexbor_str_t
 * data, which the arena keeps alive (nodes are detached, never destroyed) and
 * which only a mutation can reallocate. Fail-closed: a build OOM leaves the
 * index unbuilt and the caller falls back to its own walk.
 */

typedef struct {
    const lxb_dom_node_t *node;  /* key; NULL marks an empty slot */
    uint32_t start;              /* [start, end) into slices[] (indices, not */
    uint32_t end;                /* byte offsets; bounded by the slice count, */
                                 /* which the build caps at UINT32_MAX) */
} mkr_text_range_t;

typedef struct {
    mkr_borrowed_text_t *slices; /* doc-order TEXT/CDATA slices (borrowed) */
    size_t              *prefix; /* nslices + 1; prefix[i] = bytes before slice i */
    size_t               nslices;

    mkr_text_range_t *ranges;    /* element/fragment -> slice run, open addressing */
    size_t            ranges_cap;   /* power of two, or 0 */
    size_t            ranges_count;

    lxb_dom_node_t   *root;      /* the indexed subtree root (document root element) */
} mkr_text_idx_t;

/* Explicit (heap) DFS frame - recursion is avoided so a deep tree cannot blow
 * the C stack (matches the attr/element index discipline). */
typedef struct {
    lxb_dom_node_t *node;
    lxb_dom_node_t *child;   /* next child to visit */
    size_t          range;   /* index into ranges[] for this open element */
} mkr_tix_frame_t;

/* ------------------------------------------------------------------ */
/* hashing                                                            */
/* ------------------------------------------------------------------ */

/* pow2-ceil sizing is shared: mkr_pow2_ceil (core/mkr_core.h). */

/* Bucket index for a node key (mkr_ptr_hash spreads aligned pointers). */
static size_t
mkr_tix_slot_for(const mkr_text_idx_t *t, const lxb_dom_node_t *node)
{
    return (size_t)mkr_ptr_hash(node) & (t->ranges_cap - 1);
}

/* Insert node with start (end filled on subtree close); return its slot index.
 * The table is pre-sized for exactly the element count with load < 3/4, so it
 * never rehashes mid-build and a slot is always free (linear probing). */
static size_t
mkr_tix_range_insert(mkr_text_idx_t *t, lxb_dom_node_t *node, size_t start)
{
    size_t i = mkr_tix_slot_for(t, node);
    while (t->ranges[i].node != NULL) {
        i = (i + 1) & (t->ranges_cap - 1);
    }
    t->ranges[i].node  = node;
    t->ranges[i].start = (uint32_t)start;
    t->ranges[i].end   = (uint32_t)start; /* provisional until close */
    t->ranges_count++;
    return i;
}

static const mkr_text_range_t *
mkr_tix_range_lookup(const mkr_text_idx_t *t, const lxb_dom_node_t *node)
{
    if (t->ranges_cap == 0) return NULL;
    size_t i = mkr_tix_slot_for(t, node);
    while (t->ranges[i].node != NULL) {
        if (t->ranges[i].node == node) return &t->ranges[i];
        i = (i + 1) & (t->ranges_cap - 1);
    }
    return NULL;
}

/* ------------------------------------------------------------------ */
/* build                                                              */
/* ------------------------------------------------------------------ */

static int
mkr_tix_is_text(const lxb_dom_node_t *n)
{
    return n->type == LXB_DOM_NODE_TYPE_TEXT
        || n->type == LXB_DOM_NODE_TYPE_CDATA_SECTION;
}

static int
mkr_tix_is_container(const lxb_dom_node_t *n)
{
    return n->type == LXB_DOM_NODE_TYPE_ELEMENT
        || n->type == LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT;
}

/* Pass 1: count text slices and container (element/fragment) nodes under root
 * (inclusive), so the slice arrays and the range table are each sized once. */
static void
mkr_tix_count(lxb_dom_node_t *root, size_t *out_slices, size_t *out_containers)
{
    size_t ns = 0, nc = 0;
    for (lxb_dom_node_t *n = root; n != NULL; n = mkr_dom_preorder_next(n, root)) {
        if (mkr_tix_is_container(n)) {
            nc++;
        } else if (mkr_tix_is_text(n)) {
            const lexbor_str_t *d = &lxb_dom_interface_character_data(n)->data;
            if (d->data != NULL && d->length != 0) ns++;
        }
    }
    *out_slices = ns;
    *out_containers = nc;
}

static void
mkr_text_idx_destroy(mkr_text_idx_t *t)
{
    if (t == NULL) return;
    free(t->slices);
    free(t->prefix);
    free(t->ranges);
    free(t);
}

/* Build the index over root (the document root element). Returns NULL on OOM
 * (fail-closed: caller falls back to its own walk). */
static mkr_text_idx_t *
mkr_text_idx_build(lxb_dom_node_t *root)
{
    size_t nslices = 0, ncont = 0;
    mkr_tix_count(root, &nslices, &ncont);

    /* Slice run bounds (start/end) are stored as uint32 in the range table. A
     * document with > UINT32_MAX text slices is impossible in practice (each is
     * >= 1 byte), but guard it anyway: fail closed so the caller walks. */
    if (nslices > UINT32_MAX) return NULL;

    mkr_text_idx_t *t = mkr_callocarray(1, sizeof(*t));
    if (t == NULL) return NULL;
    t->root = root;

    if (nslices > 0) {
        t->slices = mkr_callocarray(nslices, sizeof(*t->slices));
        if (t->slices == NULL) { mkr_text_idx_destroy(t); return NULL; }
    }
    /* prefix is always allocated (>= 1 entry, prefix[0] == 0) so an empty range
     * [0,0) over a text-free subtree reads prefix[0] rather than NULL. */
    size_t pcount;
    if (!mkr_size_add(nslices, 1, &pcount)) { mkr_text_idx_destroy(t); return NULL; }
    t->prefix = mkr_callocarray(pcount, sizeof(*t->prefix));
    if (t->prefix == NULL) { mkr_text_idx_destroy(t); return NULL; }
    /* Size the range table for ncont entries at load factor < 3/4. */
    if (ncont > 0) {
        size_t want;
        if (!mkr_size_add(ncont, ncont >> 1, &want)
            || !mkr_size_add(want, 1, &want)
            || !mkr_pow2_ceil(want, &t->ranges_cap)) { mkr_text_idx_destroy(t); return NULL; }
        t->ranges = mkr_callocarray(t->ranges_cap, sizeof(*t->ranges));
        if (t->ranges == NULL) { mkr_text_idx_destroy(t); return NULL; }
    }

    /* Pass 2: explicit (heap) DFS recording each container's slice run. The
     * frame stack grows via the overflow-checked helper; depth is bounded by
     * tree depth, so it never blows the C stack. */
    mkr_tix_frame_t *st = NULL;
    size_t scap = 0, sn = 0;

    /* Seed with root (a container). */
    {
        size_t r = mkr_tix_range_insert(t, root, 0);
        if (mkr_grow_reserve((void **)&st, &scap, 1, sizeof(*st)) != MKR_OK) {
            mkr_text_idx_destroy(t); return NULL;
        }
        st[sn++] = (mkr_tix_frame_t){ root, root->first_child, r };
    }

    while (sn > 0) {
        mkr_tix_frame_t *top = &st[sn - 1];
        lxb_dom_node_t *child = top->child;
        if (child == NULL) {
            t->ranges[top->range].end = (uint32_t)t->nslices; /* close subtree */
            sn--;
            continue;
        }
        top->child = child->next; /* advance the cursor for our return */

        if (mkr_tix_is_text(child)) {
            const lexbor_str_t *d = &lxb_dom_interface_character_data(child)->data;
            if (d->data != NULL && d->length != 0) {
                size_t i = t->nslices;
                size_t total;
                if (!mkr_size_add(t->prefix[i], d->length, &total)) {
                    /* cumulative text length overflows size_t - give up the
                     * index (fail closed); the caller falls back to a walk. */
                    free(st);
                    mkr_text_idx_destroy(t);
                    return NULL;
                }
                t->slices[i].ptr = (const char *)d->data;
                t->slices[i].len = d->length;
                t->prefix[i + 1] = total;
                t->nslices = i + 1;
            }
        } else if (mkr_tix_is_container(child)) {
            size_t r = mkr_tix_range_insert(t, child, t->nslices);
            /* grow_reserve may move st; top is re-fetched at the loop head and
             * not used after this point in the iteration. */
            if (mkr_grow_reserve((void **)&st, &scap, sn + 1, sizeof(*st)) != MKR_OK) {
                free(st); mkr_text_idx_destroy(t); return NULL;
            }
            st[sn++] = (mkr_tix_frame_t){ child, child->first_child, r };
        }
        /* other kinds (comment/PI/doctype) are childless leaves: ignore. */
    }

    free(st);
    return t;
}

/* ------------------------------------------------------------------ */
/* public surface                                                     */
/* ------------------------------------------------------------------ */

void
mkr_text_index_free(void *idx)
{
    mkr_text_idx_destroy((mkr_text_idx_t *)idx);
}

void
mkr_parsed_text_index_invalidate(mkr_parsed_t *p)
{
    if (p == NULL || p->text_index == NULL) return;
    mkr_text_idx_destroy((mkr_text_idx_t *)p->text_index);
    p->text_index = NULL;
}

int
mkr_parsed_text_slices(mkr_parsed_t *p, const lxb_dom_node_t *node,
                       const mkr_borrowed_text_t **out_slices, size_t *out_n,
                       size_t *out_bytes)
{
    if (p == NULL || p->doc == NULL || node == NULL) return 0;

    mkr_text_idx_t *t = (mkr_text_idx_t *)p->text_index;
    if (t == NULL) {
        lxb_dom_node_t *root = lxb_dom_document_root((lxb_dom_document_t *)p->doc);
        if (root == NULL) return 0;
        t = mkr_text_idx_build(root);
        if (t == NULL) return 0; /* OOM: caller walks instead */
        p->text_index = t;
    }

    const mkr_text_range_t *r = mkr_tix_range_lookup(t, node);
    if (r == NULL) return 0; /* node not in the indexed tree (fragment, etc.) */

    *out_slices = t->slices + r->start;
    *out_n      = (size_t)(r->end - r->start);
    *out_bytes  = t->prefix[r->end] - t->prefix[r->start];
    return 1;
}
