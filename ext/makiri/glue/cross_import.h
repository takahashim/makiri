#ifndef MAKIRI_CROSS_IMPORT_H
#define MAKIRI_CROSS_IMPORT_H

/* Glue-side helpers for cross-kind Document#import_node. The Ruby-FREE subtree
 * translators live in lexbor_compat/cross_import.c (they read/write both Lexbor
 * and the XML arena); this header adds the Ruby-boundary pieces the import_node
 * entries (ruby_doc.c / ruby_xml_node.c) need on top of them. */

#include "../makiri.h"
#include "../lexbor_compat/cross_import.h"  /* mkr_cross_html_to_xml / _xml_to_html */

#ifdef __cplusplus
extern "C" {
#endif

/* Which representation a wrapped Ruby node is, by its TypedData type (NOT by Ruby
 * class). A Document VALUE, a NodeSet, or any non-node is MKR_NODE_KIND_OTHER.
 * Defined in ruby_node.c. */
typedef enum { MKR_NODE_KIND_OTHER = 0, MKR_NODE_KIND_HTML, MKR_NODE_KIND_XML } mkr_node_kind_t;
mkr_node_kind_t mkr_node_kind(VALUE v);

/* Raise a Ruby exception for a non-OK mutation status (no-op on OK). Defined in
 * ruby_xml_node.c; shared by the XML mutators and the cross-import entries. */
void mkr_xml_mut_check(mkr_xml_mut_status_t st);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_CROSS_IMPORT_H */
