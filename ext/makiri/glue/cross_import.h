#ifndef MAKIRI_CROSS_IMPORT_H
#define MAKIRI_CROSS_IMPORT_H

/* Cross-kind import_node: translate a subtree between the HTML (Lexbor
 * lxb_dom_node_t) and XML (mkr_xml_node_t) representations. The XML TUs
 * (ext/makiri/xml) never include Lexbor by design, so the translation - the
 * only place both representations are touched at once - lives in glue
 * (ruby_cross_import.c). */

#include "../makiri.h"             /* lxb_dom_* types */
#include "../xml/mkr_xml_node.h"   /* mkr_xml_node_t, mkr_xml_doc_t */
#include "../xml/mkr_xml_mutate.h" /* mkr_xml_mut_status_t */

#ifdef __cplusplus
extern "C" {
#endif

/* Which representation a wrapped Ruby node is, by its TypedData type (NOT by Ruby
 * class). A Document VALUE, a NodeSet, or any non-node is MKR_KIND_OTHER. */
typedef enum { MKR_NODE_KIND_OTHER = 0, MKR_NODE_KIND_HTML, MKR_NODE_KIND_XML } mkr_node_kind_t;
mkr_node_kind_t mkr_node_kind(VALUE v);

/* Build a DETACHED deep (or shallow, deep=0) copy of +src+ in the destination
 * representation, owned by the destination document. Iterative (no C recursion ->
 * no stack DoS) and fail-closed: returns an mkr_xml_mut_status_t, frees its own
 * scratch on every path, and leaves any partial copy abandoned in the destination
 * arena (freed with the document). *out is set only on MKR_XML_MUT_OK.
 *
 * A source node type with no destination counterpart fails closed: an XML CDATA
 * section translated to HTML returns MKR_XML_MUT_TYPE (HTML has no CDATA). */
mkr_xml_mut_status_t mkr_cross_html_to_xml(mkr_xml_doc_t *xdoc, lxb_dom_node_t *src,
                                           int deep, mkr_xml_node_t **out);
mkr_xml_mut_status_t mkr_cross_xml_to_html(lxb_dom_document_t *hdoc, const mkr_xml_node_t *src,
                                           int deep, lxb_dom_node_t **out);

/* Raise a Ruby exception for a non-OK mutation status (no-op on OK). Defined in
 * ruby_xml_node.c; shared by the XML mutators and the cross-import entries. */
void mkr_xml_mut_check(mkr_xml_mut_status_t st);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CROSS_IMPORT_H */
