/* mkr_xml_mutate.h - Ruby-free XML tree mutation primitives (Phase 1).
 *
 * The write counterpart of mkr_xml_tree.c's read-only build: in-place edits over
 * the custom node arena (rename, attribute set/remove, content set, detach). All
 * primitives are Ruby-free and operate only on arena-stable pointers, so they are
 * fully covered by ASan/UBSan and the fuzzer; the Ruby boundary (glue/
 * ruby_xml_node.c) coerces+verifies arguments and maps the status codes below to
 * exceptions.
 *
 * DETACH-NEVER-DESTROY: a removed node is unlinked, never freed - the arena owns
 * node memory and a live Ruby wrapper may still alias a removed node (the same
 * invariant the read-only reader relied on). New nodes append to the arena and
 * count against the per-document byte/node budgets (fail-closed -> MKR_XML_MUT_OOM).
 *
 * SECURITY: names are validated as XML 1.0 QNames and values as XML Char, so the
 * tree stays serializable to well-formed XML; an unbound prefix or a reserved
 * name fails closed rather than producing a wrong/unserializable node.
 */
#ifndef MKR_XML_MUTATE_H
#define MKR_XML_MUTATE_H

#include <stdint.h>
#include "mkr_xml_node.h"

/* Mutation outcome (the glue maps each to a Ruby exception). */
typedef enum {
    MKR_XML_MUT_OK = 0,
    MKR_XML_MUT_OOM,         /* arena byte/node budget or allocation failure */
    MKR_XML_MUT_BAD_NAME,    /* not a well-formed XML 1.0 QName (or a reserved one) */
    MKR_XML_MUT_BAD_CHARS,   /* a non-XML-Char byte (§2.2) or a forbidden sequence
                              * for the context ("?>" in a PI, "--" in a comment,
                              * "]]>" in a CDATA section) */
    MKR_XML_MUT_UNBOUND_NS,  /* a prefix has no in-scope namespace binding */
    MKR_XML_MUT_TYPE,        /* the operation is invalid for this node type */
    MKR_XML_MUT_CYCLE,       /* the node would become a descendant of itself */
    MKR_XML_MUT_HIERARCHY,   /* invalid placement (attr/document child, second root, parentless ref) */
    MKR_XML_MUT_BAD_NS_DECL  /* a namespace declaration is invalid (xmlns:prefix bound to "") */
} mkr_xml_mut_status_t;

/* Detach +node+ from the tree: from its parent's child list, or - for an
 * attribute - from its owner element's attribute list. No-op when already
 * detached (no parent). The arena keeps the memory (detach-never-destroy); the
 * caller clears doc->root if it detaches the root element. */
void mkr_xml_detach(mkr_xml_node_t *node);

/* Rename an element or attribute in place to the QName [name, name+nlen):
 * validates it, copies it contiguously into the arena, re-splits prefix/local,
 * and re-resolves the namespace against the node's in-scope declarations (the
 * element itself for an element; the owner element for an attribute). The node
 * pointer - and thus its identity and tree position - is preserved. */
mkr_xml_mut_status_t mkr_xml_rename(mkr_xml_doc_t *doc, mkr_xml_node_t *node,
                                    const char *name, uint32_t nlen);

/* Set attribute [name,nlen]="[val,vlen]" on element +el+: replaces the value of
 * an existing attribute with the same raw QName, otherwise appends a new
 * attribute node. The namespace is resolved like the parser (xmlns / xmlns:*  ->
 * the xmlns namespace; the predefined xml: prefix -> the XML namespace; any other
 * prefix -> its in-scope URI, else MKR_XML_MUT_UNBOUND_NS; unprefixed -> none).
 * +val+ must be all XML Char. *out (may be NULL) receives the affected node. */
mkr_xml_mut_status_t mkr_xml_set_attribute(mkr_xml_doc_t *doc, mkr_xml_node_t *el,
                                           const char *name, uint32_t nlen,
                                           const char *val, uint32_t vlen,
                                           mkr_xml_node_t **out);

/* Remove the first attribute of element +el+ whose raw QName is [name,nlen].
 * Returns 1 if one was removed, 0 if none matched (detach-never-destroy). */
int mkr_xml_remove_attribute(mkr_xml_node_t *el, const char *name, uint32_t nlen);

/* Set +node+'s text content. For an ELEMENT: detach every child and append a
 * single TEXT node holding [text,tlen] (an empty +text+ leaves an empty element).
 * For a TEXT/CDATA/COMMENT/PI leaf: replace its value. +text+ must be all XML
 * Char. Any other node type is MKR_XML_MUT_TYPE. */
mkr_xml_mut_status_t mkr_xml_set_content(mkr_xml_doc_t *doc, mkr_xml_node_t *node,
                                         const char *text, uint32_t tlen);

/* ---- Phase 2: building new subtrees ---------------------------------------
 *
 * Factories build a DETACHED node in +doc+'s arena (no parent); its namespace is
 * left UNRESOLVED (ns_uri NULL) until it is inserted, when the insertion site
 * resolves the whole inserted subtree against its new in-scope declarations. */

/* A new detached ELEMENT named [name,nlen] (QName-validated + split). */
mkr_xml_mut_status_t mkr_xml_new_element(mkr_xml_doc_t *doc, const char *name, uint32_t nlen,
                                         mkr_xml_node_t **out);

/* A new detached TEXT / CDATA_SECTION / COMMENT node holding [text,tlen]
 * (validated as XML Char). +type+ selects which. */
mkr_xml_mut_status_t mkr_xml_new_chardata(mkr_xml_doc_t *doc, uint8_t type,
                                          const char *text, uint32_t tlen,
                                          mkr_xml_node_t **out);

/* A new detached PROCESSING_INSTRUCTION with target [target,tlen] (an NCName,
 * not "xml" in any case) and data [data,dlen] (XML Char, may not contain "?>"). */
mkr_xml_mut_status_t mkr_xml_new_pi(mkr_xml_doc_t *doc, const char *target, uint32_t tlen,
                                    const char *data, uint32_t dlen, mkr_xml_node_t **out);

/* Deep-copy +src+ and its whole subtree (attributes included) from its arena into
 * +doc+'s arena, returning the detached copy in *out. Namespaces are left
 * unresolved (the insertion site resolves them). Iterative (no recursion -> no
 * stack DoS); fails closed on budget/OOM (the partial copy is abandoned in the
 * arena and freed with the document). For moving a node BETWEEN documents. */
mkr_xml_mut_status_t mkr_xml_import_subtree(mkr_xml_doc_t *doc, const mkr_xml_node_t *src,
                                            mkr_xml_node_t **out);

/* Insert +node+ into +doc+'s tree: as the last child of +parent+, or before /
 * after the sibling +ref+, or in place of +ref+ (replace). +node+ MUST already
 * live in +doc+'s arena (the caller imports a cross-document node first). Each:
 *   - validates placement first (no cycle, no attribute/document/doctype as a
 *     child, a single document-element root, a ref that has a parent) and
 *     resolves the inserted subtree's namespaces against the prospective context
 *     - ALL before any structural change, so a failure leaves the tree untouched
 *     (fully fail-closed, even for a move);
 *   - then detaches +node+ from any current position (move) and links it;
 *   - keeps doc->root in sync when the container is the document node.
 * Detach-never-destroy: a replaced/displaced node is unlinked, never freed. */
mkr_xml_mut_status_t mkr_xml_insert_child (mkr_xml_doc_t *doc, mkr_xml_node_t *parent,
                                           mkr_xml_node_t *node);
mkr_xml_mut_status_t mkr_xml_insert_before(mkr_xml_doc_t *doc, mkr_xml_node_t *ref,
                                           mkr_xml_node_t *node);
mkr_xml_mut_status_t mkr_xml_insert_after (mkr_xml_doc_t *doc, mkr_xml_node_t *ref,
                                           mkr_xml_node_t *node);
mkr_xml_mut_status_t mkr_xml_replace_node (mkr_xml_doc_t *doc, mkr_xml_node_t *ref,
                                           mkr_xml_node_t *node);

/* Structural self-test (validation, namespace resolution, link/unlink). Returns 0
 * on success or the 1-based index of the first failing check. Run from
 * Makiri.__c_selftest. */
int mkr_xml_mutate_selftest(void);

#endif /* MKR_XML_MUTATE_H */
