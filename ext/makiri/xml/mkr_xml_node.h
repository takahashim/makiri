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

/* The XML representation's DOM node-type constants — the counterpart of Lexbor's
 * LXB_DOM_NODE_TYPE_* for HTML. The numeric values mirror the DOM/Lexbor
 * encoding exactly (asserted in mkr_xpath_node_access_xml.h) so the monomorphized
 * XPath engine's neutral MKR_NTYPE_* bind to whichever representation it compiles
 * for. The reader produces ELEMENT/ATTRIBUTE/TEXT/CDATA_SECTION/PI/COMMENT/
 * DOCUMENT in the tree, plus a single off-tree DOCUMENT_TYPE (the DOCTYPE
 * metadata, see doc->doctype); the remaining members exist only so the shared
 * engine's node-type contract is complete for both instances. */
typedef enum {
    MKR_XML_NODE_TYPE_ELEMENT          = 1,
    MKR_XML_NODE_TYPE_ATTRIBUTE        = 2,
    MKR_XML_NODE_TYPE_TEXT             = 3,
    MKR_XML_NODE_TYPE_CDATA_SECTION    = 4,
    MKR_XML_NODE_TYPE_ENTITY_REFERENCE = 5,  /* never produced (no DTD) */
    MKR_XML_NODE_TYPE_ENTITY           = 6,  /* never produced (no DTD) */
    MKR_XML_NODE_TYPE_PI               = 7,
    MKR_XML_NODE_TYPE_COMMENT          = 8,
    MKR_XML_NODE_TYPE_DOCUMENT         = 9,
    MKR_XML_NODE_TYPE_DOCUMENT_TYPE    = 10, /* the off-tree DOCTYPE metadata node */
    MKR_XML_NODE_TYPE_NOTATION         = 12  /* never produced (no DTD) */
} mkr_xml_node_type_t;

/* Field order is chosen for size + locality, not readability:
 *   1. the hot navigation/type fields first (the engine reads type + the tree
 *      pointers in its tight walk), kept within the first cache line;
 *   2. the name/value pointers grouped;
 *   3. their lengths grouped at the end.
 * Grouping the uint32 lengths lets adjacent ones pack into shared 8-byte slots
 * instead of each wasting 4 bytes of padding before the next 8-aligned pointer
 * — 144 -> 128 bytes/node (measured), ~160 MB saved at the 10M-node cap. The
 * split is invisible to callers: a length is only ever read via its paired
 * pointer (n->qname / n->qname_len), the field NAMES are unchanged, and the
 * lengths stay uint32 (the deliberate per-slice 4 GiB cap; see the enum note).
 * Names/values are arena-owned byte slices (case-preserved). */
typedef struct mkr_xml_node {
    uint8_t  type;                 /* mkr_xml_node_type_t */
    struct mkr_xml_node *parent, *first_child, *last_child, *prev, *next;
    struct mkr_xml_node *attrs;    /* element: head of attribute list (type=ATTRIBUTE) */
    const char *qname;   /* element/attr raw "prefix:local" (contiguous; local/prefix slice into it) */
    const char *local;
    const char *prefix;
    const char *ns_uri;  /* resolved namespace URI (original case) */
    const char *value;   /* text/cdata/comment/pi-data/attr value */
    uint32_t qname_len, local_len, prefix_len, ns_uri_len, value_len;
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
    mkr_xml_node_t *root;          /* the root element */
    mkr_xml_node_t *doc_node;      /* the DOCUMENT node (parent of root); the XPath
                                    * "/" root and what a Ruby Document wraps */
    mkr_xml_node_t *doctype;       /* a DOCUMENT_TYPE node holding the DOCTYPE's
                                    * metadata, or NULL. Kept OFF the tree (it is
                                    * not a child of doc_node, so XPath is
                                    * unaffected) and exposed only via
                                    * Document#internal_subset. The DTD itself is
                                    * NOT parsed (no entities/elements). On this
                                    * node: local/qname = name, prefix = public
                                    * (external) id, value = system id; a 0-length
                                    * prefix/value means that id is absent. */
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
