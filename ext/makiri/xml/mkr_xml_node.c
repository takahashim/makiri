/* mkr_xml_node.c - secure-by-design append-only arena + custom XML node.
 * Ruby-free. See mkr_xml_node.h and docs/xml_parser_plan.ja.md §8.1/§8.2. */
#include "mkr_xml_node.h"
#include "mkr_xml.h"
#include "mkr_xml_index.h"
#include "../core/mkr_core.h"

#include <stddef.h>   /* max_align_t */
#include <stdlib.h>
#include <string.h>

/* ASan red-zoning for the bump arena. Like Lexbor's mraw, our arena carves many
 * sub-allocations out of one malloc'd chunk, so a write past one cut into the
 * next stays inside that single malloc and is INVISIBLE to a plain ASan build
 * (the heap red-zones only guard the chunk's outer boundary - this is exactly
 * how the v3.0.0 :lexbor-contains overflow hid). So we poison each fresh chunk
 * and unpoison only the bytes a cut actually hands out, leaving the alignment
 * tail and not-yet-cut space poisoned: an intra-arena overrun then writes into
 * poisoned memory and ASan reports it. Auto-active whenever our ext is built
 * with -fsanitize=address (rake sanitize / fuzz:sanitize) - no extra flag,
 * because unlike vendored Lexbor this is our own TU. A no-op otherwise. */
#if defined(__SANITIZE_ADDRESS__)
#  define MKR_XML_HAVE_ASAN 1
#elif defined(__has_feature)
#  if __has_feature(address_sanitizer)
#    define MKR_XML_HAVE_ASAN 1
#  endif
#endif
#if defined(MKR_XML_HAVE_ASAN)
#  include <sanitizer/asan_interface.h>
#  define MKR_ARENA_POISON(p, n)   __asan_poison_memory_region((p), (n))
#  define MKR_ARENA_UNPOISON(p, n) __asan_unpoison_memory_region((p), (n))
#else
#  define MKR_ARENA_POISON(p, n)   ((void)(p), (void)(n))
#  define MKR_ARENA_UNPOISON(p, n) ((void)(p), (void)(n))
#endif

/* Alignment for arena cuts: the strictest fundamental alignment, so any object
 * fits. The chunk payload starts at an aligned offset past the header, and every
 * cut size is rounded up to ARENA_ALIGN, so each returned pointer is aligned. */
#define ARENA_ALIGN   _Alignof(max_align_t)
_Static_assert((ARENA_ALIGN & (ARENA_ALIGN - 1)) == 0, "ARENA_ALIGN must be a power of two");
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
    return doc;
}

void
mkr_xml_doc_destroy(mkr_xml_doc_t *doc)
{
    if (doc == NULL) return;
    mkr_xml_name_index_invalidate(doc); /* free the heap-side element-name index */
    /* whole-arena free: no individual node/byte free anywhere (read-only). */
    const size_t hdr = align_up(sizeof(mkr_xml_arena_chunk_t), ARENA_ALIGN);
    for (mkr_xml_arena_chunk_t *c = doc->chunks; c != NULL; ) {
        mkr_xml_arena_chunk_t *n = c->next;
        MKR_ARENA_UNPOISON((char *)c, hdr + c->cap); /* hand clean memory back to ASan's allocator */
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
        size_t chunk;
        /* saturate rather than wrap: a bogus huge memsize is harmless, a wrapped
         * small one would lie to Ruby's GC accounting. */
        if (!mkr_size_add(hdr, c->cap, &chunk) || !mkr_size_add(total, chunk, &total)) {
            return SIZE_MAX;
        }
    }
    return total;
}

/* THE single checked alloc choke point. PRIVATE - callers use the typed
 * wrappers below. Overflow- and budget-checked; on any failure sets doc->oom
 * (sticky) and returns NULL. Nothing else cuts arena. */
static void *
arena_alloc(mkr_xml_doc_t *doc, size_t size)
{
    /* fail-closed contract: the internal callers never pass NULL, but a primitive
     * must not deref it - return NULL rather than crash (there is no doc to mark). */
    if (doc == NULL || doc->oom) return NULL;

    size_t need = align_up(size, ARENA_ALIGN);
    if (need < size) { doc->oom = MKR_XML_ERR_LIMIT; return NULL; } /* align overflow */

    /* budget BEFORE allocation - fail-closed */
    size_t projected;
    if (!mkr_size_add(doc->arena_bytes, need, &projected) || projected > doc->max_bytes) {
        doc->oom = MKR_XML_ERR_LIMIT;
        return NULL;
    }

    const size_t hdr = align_up(sizeof(mkr_xml_arena_chunk_t), ARENA_ALIGN);
    mkr_xml_arena_chunk_t *c = doc->chunks;
    /* subtractive form (used <= cap invariant) avoids an addition that could
     * overflow with a pathological chunk. */
    if (c == NULL || need > c->cap - c->used) {
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
        /* poison the whole payload; each cut unpoisons only what it returns. */
        MKR_ARENA_POISON((char *)nc + hdr, cap);
    }
    void *p = (char *)c + hdr + c->used;
    c->used += need;
    doc->arena_bytes += need;
    /* Hand out exactly +size+ usable bytes; the [size, need) alignment tail (and
     * everything past it in the chunk) stays poisoned as a red-zone, so a write
     * one byte past the request is caught instead of silently clobbering the
     * next cut. ASan tolerates the sub-granule trailing byte (ARENA_ALIGN cuts
     * keep each slot on an 8-byte shadow boundary). */
    MKR_ARENA_UNPOISON(p, size);
    return p;
}

static int
valid_xn_type(uint8_t type)
{
    switch (type) {
    case MKR_XML_NODE_TYPE_ELEMENT: case MKR_XML_NODE_TYPE_ATTRIBUTE: case MKR_XML_NODE_TYPE_TEXT:
    case MKR_XML_NODE_TYPE_CDATA_SECTION:   case MKR_XML_NODE_TYPE_PI:        case MKR_XML_NODE_TYPE_COMMENT:
    case MKR_XML_NODE_TYPE_DOCUMENT:        case MKR_XML_NODE_TYPE_DOCUMENT_TYPE:
    case MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT:
        return 1;
    default:
        return 0;
    }
}

mkr_xml_node_t *
mkr_xml_arena_node(mkr_xml_doc_t *doc, uint8_t type)
{
    if (doc == NULL) return NULL;   /* fail-closed: never deref a NULL document */
    /* guard against a caller passing a bogus type (programming error) so it can
     * never be silently miscounted/mis-walked - caught by the self-test/ASan. */
    if (!valid_xn_type(type)) { doc->oom = MKR_XML_ERR_INTERNAL; return NULL; }
    /* every node (attributes included) counts toward the single node budget; the
     * per-element attribute cap is enforced by the tree builder. */
    if (doc->nodes + 1 > doc->max_nodes) { doc->oom = MKR_XML_ERR_LIMIT; return NULL; }

    mkr_xml_node_t *n = arena_alloc(doc, sizeof *n);
    if (n == NULL) return NULL;
    memset(n, 0, sizeof *n);   /* zero-init: no uninitialised reads; 0 = "no source loc" */
    n->type = type;
    doc->nodes++;
    return n;
}

const char *
mkr_xml_arena_bytes(mkr_xml_doc_t *doc, const char *src, uint32_t len)
{
    if (len == 0) return "";          /* never read off the "" literal */
    if (src == NULL) return NULL;     /* contract: len>0 needs a real source (fail closed) */
    char *p = arena_alloc(doc, len);
    if (p == NULL) return NULL;
    memcpy(p, src, len);              /* copy-on-store: the arena owns the bytes */
    return p;
}

/* A raw, uninitialised arena buffer of exactly +len+ bytes, for the one caller
 * that must fill it itself (entity expansion writes the expanded, never-longer
 * output here). Budget-/overflow-checked via the same choke point. */
char *
mkr_xml_arena_scratch_bytes(mkr_xml_doc_t *doc, size_t len)
{
    /* a 0-length buffer needs no storage - never cut a whole chunk for it. The
     * sentinel is a valid non-NULL pointer the caller must not write to (there is
     * nothing to write), mirroring mkr_xml_arena_bytes's "" return. */
    if (len == 0) return (char *)"";
    return arena_alloc(doc, len);
}

int
mkr_xml_node_xmlns_decl(const mkr_xml_node_t *a, const char **prefix, uint32_t *plen,
                        const char **uri, uint32_t *ulen)
{
    if (a->qname_len == 5 && memcmp(a->qname, "xmlns", 5) == 0) {
        *prefix = ""; *plen = 0;
    } else if (a->qname_len > 6 && memcmp(a->qname, "xmlns:", 6) == 0) {
        *prefix = a->local; *plen = a->local_len;
    } else {
        return 0;
    }
    *uri = a->value ? a->value : ""; *ulen = a->value_len;
    return 1;
}

/* ---- self-test (Makiri.__c_selftest) - mirrors tmp/xml_spike/arena_spike.c --- */
int
mkr_xml_node_selftest(void)
{
    int idx = 0;
    mkr_xml_doc_t *doc = mkr_xml_doc_new();
    if (doc == NULL) return ++idx;            /* 1 */

    /* node zero-init + name copy */
    idx++;                                     /* 2 */
    mkr_xml_node_t *root = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_ELEMENT);
    char nm[] = "Feed";
    const char *local = mkr_xml_arena_bytes(doc, nm, 4);
    if (root == NULL || root->first_child != NULL || root->type != MKR_XML_NODE_TYPE_ELEMENT
        || local == NULL || local == nm || memcmp(local, "Feed", 4) != 0) {
        mkr_xml_doc_destroy(doc); return idx;
    }
    /* pointer alignment */
    idx++;                                     /* 3 */
    if (((uintptr_t)root % ARENA_ALIGN) != 0) { mkr_xml_doc_destroy(doc); return idx; }

    /* build 1000 children */
    idx++;                                     /* 4 */
    for (int i = 0; i < 1000; i++) {
        mkr_xml_node_t *c = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_ELEMENT);
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
    if (arena_alloc(doc, ((size_t)-1) - 8) != NULL || doc->oom == MKR_XML_OK) {
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
        if (mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_ELEMENT) == NULL) {
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
        if (mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_ELEMENT) == NULL) {
            nlimit = (doc->oom == MKR_XML_ERR_LIMIT);
            break;
        }
    }
    if (!nlimit) { mkr_xml_doc_destroy(doc); return idx; }
    mkr_xml_doc_destroy(doc);

    /* fail-closed on a NULL document (contract guard, no deref/crash) */
    idx++;                                     /* 8 */
    if (mkr_xml_arena_node(NULL, MKR_XML_NODE_TYPE_ELEMENT) != NULL
        || mkr_xml_arena_bytes(NULL, "x", 1) != NULL
        || mkr_xml_arena_scratch_bytes(NULL, 1) != NULL) {
        return idx;
    }

    return 0; /* all checks passed */
}

/* mkr_xml_parse now lives in mkr_xml_tree.c (the minimal tokenizer + tree
 * builder). This file owns only the node/arena primitives. */
