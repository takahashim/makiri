/* mkr_xml_node.c — secure-by-design append-only arena + custom XML node.
 * Ruby-free. See mkr_xml_node.h and docs/xml_parser_plan.ja.md §8.1/§8.2. */
#include "mkr_xml_node.h"
#include "mkr_xml.h"
#include "../core/mkr_core.h"

#include <stdlib.h>
#include <string.h>

/* Alignment for arena cuts: >= alignof(max_align_t) on LP64. The chunk payload
 * starts at an aligned offset past the header, and every cut size is rounded up
 * to ARENA_ALIGN, so each returned pointer is ARENA_ALIGN-aligned. */
#define ARENA_ALIGN   (sizeof(void *) * 2)
#define CHUNK_MIN     (64u * 1024u)

struct mkr_xml_arena_chunk {
    struct mkr_xml_arena_chunk *next;
    size_t used, cap;
    /* payload follows, at offset align_up(sizeof(chunk), ARENA_ALIGN) */
};

static inline size_t align_up(size_t n, size_t a) { return (n + (a - 1)) & ~(a - 1); }

mkr_xml_doc_t *
mkr_xml_doc_new(void)
{
    mkr_xml_doc_t *doc = mkr_callocarray(1, sizeof *doc);
    if (doc == NULL) return NULL;
    doc->max_bytes = MKR_XML_MAX_BYTES;
    doc->max_nodes = MKR_XML_MAX_NODES;
    doc->max_attrs = MKR_XML_MAX_ATTRS;
    return doc;
}

void
mkr_xml_doc_destroy(mkr_xml_doc_t *doc)
{
    if (doc == NULL) return;
    /* whole-arena free: no individual node/byte free anywhere (read-only). */
    for (mkr_xml_arena_chunk_t *c = doc->chunks; c != NULL; ) {
        mkr_xml_arena_chunk_t *n = c->next;
        free(c);
        c = n;
    }
    free(doc);
}

size_t
mkr_xml_doc_memsize(const mkr_xml_doc_t *doc)
{
    if (doc == NULL) return 0;
    size_t total = sizeof *doc;
    const size_t hdr = align_up(sizeof(mkr_xml_arena_chunk_t), ARENA_ALIGN);
    for (const mkr_xml_arena_chunk_t *c = doc->chunks; c != NULL; c = c->next) {
        total += hdr + c->cap;
    }
    return total;
}

/* THE single checked alloc choke point. Overflow- and budget-checked; on any
 * failure sets doc->oom (sticky) and returns NULL. Nothing else cuts arena. */
void *
mkr_xml_arena_alloc(mkr_xml_doc_t *doc, size_t size)
{
    if (doc->oom) return NULL;

    size_t need = align_up(size, ARENA_ALIGN);
    if (need < size) { doc->oom = MKR_XML_ERR_LIMIT; return NULL; } /* align overflow */

    /* budget BEFORE allocation — fail-closed */
    size_t projected;
    if (!mkr_size_add(doc->arena_bytes, need, &projected) || projected > doc->max_bytes) {
        doc->oom = MKR_XML_ERR_LIMIT;
        return NULL;
    }

    const size_t hdr = align_up(sizeof(mkr_xml_arena_chunk_t), ARENA_ALIGN);
    mkr_xml_arena_chunk_t *c = doc->chunks;
    if (c == NULL || c->used + need > c->cap) {
        size_t cap = need > CHUNK_MIN ? need : CHUNK_MIN;
        size_t total;
        if (!mkr_size_add(hdr, cap, &total)) { doc->oom = MKR_XML_ERR_LIMIT; return NULL; }
        mkr_xml_arena_chunk_t *nc = mkr_reallocarray(NULL, total, 1);
        if (nc == NULL) { doc->oom = MKR_XML_ERR_OOM; return NULL; }
        nc->next = doc->chunks;
        nc->used = 0;
        nc->cap  = cap;
        doc->chunks = nc;
        c = nc;
    }
    void *p = (char *)c + hdr + c->used;
    c->used += need;
    doc->arena_bytes += need;
    return p;
}

mkr_xml_node_t *
mkr_xml_arena_node(mkr_xml_doc_t *doc, uint8_t type)
{
    size_t *counter = (type == MKR_XN_ATTRIBUTE) ? &doc->attrs : &doc->nodes;
    size_t  limit   = (type == MKR_XN_ATTRIBUTE) ? doc->max_attrs : doc->max_nodes;
    if (*counter + 1 > limit) { doc->oom = MKR_XML_ERR_LIMIT; return NULL; }

    mkr_xml_node_t *n = mkr_xml_arena_alloc(doc, sizeof *n);
    if (n == NULL) return NULL;
    memset(n, 0, sizeof *n);   /* zero-init: no uninitialised reads; 0 = "no source loc" */
    n->type = type;
    (*counter)++;
    return n;
}

const char *
mkr_xml_arena_bytes(mkr_xml_doc_t *doc, const char *src, uint32_t len)
{
    if (len == 0) return "";          /* never read off the "" literal */
    char *p = mkr_xml_arena_alloc(doc, len);
    if (p == NULL) return NULL;
    memcpy(p, src, len);              /* copy-on-store: the arena owns the bytes */
    return p;
}

/* ---- self-test (Makiri.__c_selftest) — mirrors tmp/xml_spike/arena_spike.c --- */
int
mkr_xml_node_selftest(void)
{
    int idx = 0;
    mkr_xml_doc_t *doc = mkr_xml_doc_new();
    if (doc == NULL) return ++idx;            /* 1 */

    /* node zero-init + name copy */
    idx++;                                     /* 2 */
    mkr_xml_node_t *root = mkr_xml_arena_node(doc, MKR_XN_ELEMENT);
    char nm[] = "Feed";
    const char *local = mkr_xml_arena_bytes(doc, nm, 4);
    if (root == NULL || root->first_child != NULL || root->type != MKR_XN_ELEMENT
        || local == NULL || local == nm || memcmp(local, "Feed", 4) != 0) {
        mkr_xml_doc_destroy(doc); return idx;
    }
    /* pointer alignment */
    idx++;                                     /* 3 */
    if (((uintptr_t)root % ARENA_ALIGN) != 0) { mkr_xml_doc_destroy(doc); return idx; }

    /* build 1000 children */
    idx++;                                     /* 4 */
    for (int i = 0; i < 1000; i++) {
        mkr_xml_node_t *c = mkr_xml_arena_node(doc, MKR_XN_ELEMENT);
        if (c == NULL) { mkr_xml_doc_destroy(doc); return idx; }
        c->parent = root;
        if (root->last_child) { root->last_child->next = c; c->prev = root->last_child; }
        else root->first_child = c;
        root->last_child = c;
    }
    size_t cnt = 0;
    for (mkr_xml_node_t *c = root->first_child; c != NULL; c = c->next) cnt++;
    if (cnt != 1000 || doc->oom != MKR_XML_OK) { mkr_xml_doc_destroy(doc); return idx; }
    mkr_xml_doc_destroy(doc);

    /* pathological size fails closed (no overflow / wrap) */
    idx++;                                     /* 5 */
    doc = mkr_xml_doc_new();
    if (doc == NULL) return idx;
    if (mkr_xml_arena_alloc(doc, ((size_t)-1) - 8) != NULL || doc->oom == MKR_XML_OK) {
        mkr_xml_doc_destroy(doc); return idx;
    }
    mkr_xml_doc_destroy(doc);

    /* byte budget enforced inside the allocator (LIMIT before overrun) */
    idx++;                                     /* 6 */
    doc = mkr_xml_doc_new();
    if (doc == NULL) return idx;
    doc->max_bytes = 4096;
    int hit = 0;
    for (int i = 0; i < 100000; i++) {
        if (mkr_xml_arena_node(doc, MKR_XN_ELEMENT) == NULL) {
            hit = (doc->oom == MKR_XML_ERR_LIMIT);
            break;
        }
    }
    if (!hit) { mkr_xml_doc_destroy(doc); return idx; }
    mkr_xml_doc_destroy(doc);

    /* node budget enforced */
    idx++;                                     /* 7 */
    doc = mkr_xml_doc_new();
    if (doc == NULL) return idx;
    doc->max_nodes = 10;
    int nlimit = 0;
    for (int i = 0; i < 100; i++) {
        if (mkr_xml_arena_node(doc, MKR_XN_ELEMENT) == NULL) {
            nlimit = (doc->oom == MKR_XML_ERR_LIMIT);
            break;
        }
    }
    if (!nlimit) { mkr_xml_doc_destroy(doc); return idx; }
    mkr_xml_doc_destroy(doc);

    return 0; /* all checks passed */
}

/* ---- parser entry (SKELETON) ---------------------------------------------
 * Builds an empty document. The tokenizer (mkr_xml_lexer.c) and tree builder
 * (mkr_xml_tree.c) land next; for now this validates the decode -> GVL -> arena
 * -> doc -> wrap pipeline. */
mkr_xml_doc_t *
mkr_xml_parse(const char *src, size_t len, mkr_xml_status_t *status)
{
    (void)src; (void)len;
    mkr_xml_doc_t *doc = mkr_xml_doc_new();
    if (doc == NULL) { if (status) *status = MKR_XML_ERR_OOM; return NULL; }
    if (status) *status = MKR_XML_OK;
    return doc;
}
