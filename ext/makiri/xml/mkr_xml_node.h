/* mkr_xml_node.h — custom XML DOM node + secure-by-design append-only arena.
 *
 * XML does NOT use lxb_dom (§2.5): a custom node held in a per-document arena,
 * case-preserved, with zero dependency on Lexbor or its internal APIs. The arena
 * is append-only and freed whole with the document (read-only Phase 1: nodes are
 * never individually freed, so Ruby wrappers that alias a node stay valid).
 * Ruby-free. Memory-safety structure documented in docs/xml_parser_plan.ja.md
 * §8.1/§8.2 and proven by mkr_xml_node_selftest().
 */
#ifndef MKR_XML_NODE_H
#define MKR_XML_NODE_H

#include <stdint.h>
#include <stddef.h>

/* Node types reuse the lxb_dom numeric values so the monomorphized XPath engine
 * (MKR_NTYPE_*) sees the same integers for HTML and XML. */
typedef enum {
    MKR_XN_ELEMENT   = 1,
    MKR_XN_ATTRIBUTE = 2,
    MKR_XN_TEXT      = 3,
    MKR_XN_CDATA     = 4,
    MKR_XN_PI        = 7,
    MKR_XN_COMMENT   = 8,
    MKR_XN_DOCUMENT  = 9
} mkr_xn_type_t;

/* The hot navigation/type fields are at the front (the engine reads them in its
 * tight walk). Names/values are arena-owned byte slices (case-preserved). */
typedef struct mkr_xml_node {
    uint8_t  type;                 /* mkr_xn_type_t */
    struct mkr_xml_node *parent, *first_child, *last_child, *prev, *next;
    struct mkr_xml_node *attrs;    /* element: head of attribute list (type=ATTRIBUTE) */
    const char *local;   uint32_t local_len;
    const char *prefix;  uint32_t prefix_len;
    const char *ns_uri;  uint32_t ns_uri_len;   /* resolved namespace URI (original case) */
    const char *value;   uint32_t value_len;    /* text/cdata/comment/pi-data/attr value */
    uint32_t line, col;            /* source position (element only; 0 = none) */
} mkr_xml_node_t;

typedef struct mkr_xml_arena_chunk mkr_xml_arena_chunk_t;

/* A document: its append-only arena, the root element (NULL until built), and
 * the per-document budgets (§4). All nodes and name/value bytes live in the
 * arena; mkr_xml_doc_destroy frees it whole. */
typedef struct mkr_xml_doc {
    mkr_xml_arena_chunk_t *chunks; /* arena chunk list */
    size_t   arena_bytes;          /* payload bytes cut (budget) */
    size_t   max_bytes;            /* MKR_XML_MAX_BYTES */
    size_t   nodes,  max_nodes;    /* total node count (attributes included) */
    int      oom;                  /* mkr_xml_status_t once non-zero (sticky) */
    mkr_xml_node_t *root;
} mkr_xml_doc_t;
/* The PER-ELEMENT attribute cap (MKR_XML_MAX_ATTRS) is enforced by the tree
 * builder; the arena counts every node — attributes included — toward max_nodes. */

mkr_xml_doc_t *mkr_xml_doc_new(void);            /* budgets from MKR_XML_MAX_*; NULL on OOM */
void           mkr_xml_doc_destroy(mkr_xml_doc_t *doc);   /* whole-arena free */
size_t         mkr_xml_doc_memsize(const mkr_xml_doc_t *doc);

/* --- secure arena: typed wrappers only (§8.2) ---
 * The single overflow- and budget-checked alloc choke point is PRIVATE to
 * mkr_xml_node.c. Callers never get a void* to cast: nodes via *_node (zeroed,
 * type-validated), copied name/value bytes via *_bytes (the arena owns them).
 * mkr_xml_arena_scratch_bytes returns a raw, uninitialised char buffer of exactly
 * +len+ bytes for the one caller that must fill it itself (entity expansion). On
 * any failure doc->oom is set (sticky) and the wrapper returns NULL. */
mkr_xml_node_t *mkr_xml_arena_node(mkr_xml_doc_t *doc, uint8_t type);
const char     *mkr_xml_arena_bytes(mkr_xml_doc_t *doc, const char *src, uint32_t len);
char           *mkr_xml_arena_scratch_bytes(mkr_xml_doc_t *doc, size_t len);

/* Self-test of the arena's secure-by-design properties (overflow, budgets,
 * alignment, zero-init, copy-on-store, whole-free). Returns 0 on success or the
 * 1-based index of the first failing check. Run from Makiri.__c_selftest. */
int mkr_xml_node_selftest(void);

#endif /* MKR_XML_NODE_H */
