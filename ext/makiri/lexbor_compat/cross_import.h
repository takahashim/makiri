#ifndef MAKIRI_LEXBOR_COMPAT_CROSS_IMPORT_H
#define MAKIRI_LEXBOR_COMPAT_CROSS_IMPORT_H

/* Cross-kind subtree translation for Document#import_node. Ruby-FREE: it reads and
 * writes BOTH the HTML (Lexbor lxb_dom_node_t) and XML (mkr_xml_node_t)
 * representations, so it lives in lexbor_compat - the layer that already bridges
 * Lexbor and the XML arena - rather than in the Ruby glue. The glue import_node
 * entries call these after a kind check and wrap the result. */

#include <lexbor/html/html.h>      /* lxb_dom_* + the <template> content interface */
#include <lexbor/dom/dom.h>
#include "../xml/mkr_xml_node.h"   /* mkr_xml_node_t, mkr_xml_doc_t */
#include "../xml/mkr_xml_mutate.h" /* mkr_xml_mut_status_t + node factories */

#ifdef __cplusplus
extern "C" {
#endif

/* Build a DETACHED deep (or shallow, deep == 0) copy of +src+ in the OTHER
 * representation, owned by the destination document, returned in *out (set only on
 * MKR_XML_MUT_OK). Iterative (no C recursion -> no stack DoS) and fail-closed: a
 * failure abandons a self-contained partial subtree in the destination arena (the
 * XML node arena or Lexbor's mraw), freed with the document. Namespaces are
 * preserved across the translation; an XML CDATA section has no HTML counterpart,
 * so mkr_cross_xml_to_html fails closed (MKR_XML_MUT_TYPE) when it meets one. */
mkr_xml_mut_status_t mkr_cross_html_to_xml(mkr_xml_doc_t *xdoc, lxb_dom_node_t *src,
                                           int deep, mkr_xml_node_t **out);
mkr_xml_mut_status_t mkr_cross_xml_to_html(lxb_dom_document_t *hdoc, const mkr_xml_node_t *src,
                                           int deep, lxb_dom_node_t **out);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_LEXBOR_COMPAT_CROSS_IMPORT_H */
