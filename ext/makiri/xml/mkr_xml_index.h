#ifndef MKR_XML_INDEX_H
#define MKR_XML_INDEX_H

#include "mkr_xml_node.h"
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Element-name index for the XML arena: maps each (local name + namespace URI)
 * to the document-ordered list of elements bearing it, so a document-rooted
 * descendant name test (//entry, css("entry")) is answered from the bucket
 * instead of walking the whole tree. The HTML side has the analogous tag-id
 * index; XML element names are arbitrary strings, so this is keyed by the name
 * bytes (borrowed from the arena, stable until the next mutation).
 *
 * Lazily built and cached on the document; dropped by
 * mkr_xml_name_index_invalidate from the single XML mutation hook (the same
 * discipline as the HTML attr/text indices). Build OOM fails closed: the getter
 * returns NULL and the caller walks the tree.
 */
typedef struct mkr_xml_name_index mkr_xml_name_index_t;

/* The document's element-name index, built and cached on first call. Returns
 * NULL on OOM (caller falls back to a tree walk). */
mkr_xml_name_index_t *mkr_xml_name_index_get(mkr_xml_doc_t *doc);

/* Drop the cached index after a structural mutation (no-op when unbuilt). */
void mkr_xml_name_index_invalidate(mkr_xml_doc_t *doc);

/* Free an index directly (used by mkr_xml_doc_destroy via the invalidate hook). */
void mkr_xml_name_index_free(mkr_xml_name_index_t *idx);

/* The document-ordered elements with local name +local+ and namespace URI
 * +ns_uri+ (ns_uri == NULL / ns_uri_len == 0 means the no-namespace bucket).
 * Returns the borrowed bucket and sets *out_count, or NULL with *out_count 0. */
mkr_xml_node_t *const *mkr_xml_name_index_lookup(const mkr_xml_name_index_t *idx,
                                                 const char *local, size_t local_len,
                                                 const char *ns_uri, size_t ns_uri_len,
                                                 size_t *out_count);

#ifdef __cplusplus
}
#endif

#endif /* MKR_XML_INDEX_H */
