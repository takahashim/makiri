/* mkr_node_access_xml.h — node-access contract bound to the custom XML node.
 *
 * The XML counterpart of mkr_node_access.h: it binds the MKR_NODE_* / MKR_ELEM_*
 * field-access macros + per-instance services to mkr_xml_node_t, so the shared
 * XPath engine body (mkr_xpath_*_body.h) compiles for XML with zero runtime
 * dispatch (§2.5). MKR_HOST_XML selects the engine's XML host-policy branches
 * (name-test matching, §8.6). The custom node carries its namespace URI and a
 * contiguous "prefix:local" qname directly, so there is no ns hash / tag table.
 */
#ifndef MKR_NODE_ACCESS_XML_H
#define MKR_NODE_ACCESS_XML_H

#include <lexbor/dom/dom.h>          /* shared LXB_DOM_NODE_TYPE_* numeric values + lxb_char_t */
#include "../xml/mkr_xml_node.h"
#include <string.h>

#define MKR_HOST_XML 1

/* Neutral node-type constants — the custom node reuses the Lexbor values. */
#define MKR_NTYPE_ELEMENT            LXB_DOM_NODE_TYPE_ELEMENT
#define MKR_NTYPE_ATTRIBUTE          LXB_DOM_NODE_TYPE_ATTRIBUTE
#define MKR_NTYPE_TEXT               LXB_DOM_NODE_TYPE_TEXT
#define MKR_NTYPE_CDATA_SECTION      LXB_DOM_NODE_TYPE_CDATA_SECTION
#define MKR_NTYPE_COMMENT            LXB_DOM_NODE_TYPE_COMMENT
#define MKR_NTYPE_PI                 LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION
#define MKR_NTYPE_DOCUMENT_TYPE      LXB_DOM_NODE_TYPE_DOCUMENT_TYPE
#define MKR_NTYPE_ENTITY             LXB_DOM_NODE_TYPE_ENTITY
#define MKR_NTYPE_ENTITY_REFERENCE   LXB_DOM_NODE_TYPE_ENTITY_REFERENCE
#define MKR_NTYPE_NOTATION           LXB_DOM_NODE_TYPE_NOTATION

/* type / synthetic namespace-id (1 = has a namespace, 0 = none). */
#define MKR_NODE_TYPE(n)          ((n)->type)
#define MKR_NODE_NS_ID(n)         ((n)->ns_uri_len ? 1u : 0u)

/* navigation */
#define MKR_NODE_FIRST_CHILD(n)   ((n)->first_child)
#define MKR_NODE_LAST_CHILD(n)    ((n)->last_child)
#define MKR_NODE_NEXT(n)          ((n)->next)
#define MKR_NODE_PREV(n)          ((n)->prev)
#define MKR_NODE_PARENT(n)        ((n)->parent)

/* element / attribute handles & iteration — the node IS its own element handle;
 * attributes are a sibling-linked list off the element's `attrs`. */
#define MKR_NODE_AS_ELEMENT(n)    (n)
#define MKR_ELEM_FIRST_ATTR(el)   ((el)->attrs)
#define MKR_ATTR_NEXT(a)          ((a)->next)
#define MKR_ATTR_VALUE(a, lenp)   (*(lenp) = (a)->value_len, (const lxb_char_t *)(a)->value)

/* Strict unprefixed element name tests must match a no-namespace node only, so a
 * node with any namespace URI is "foreign" to an unprefixed test (§8.6). */
#define MKR_NODE_IS_FOREIGN_NS(n) ((n)->ns_uri_len != 0)

/* name lookups (borrowed bytes; the qname is the contiguous "prefix:local"). */
#define MKR_ELEM_LOCAL_NAME(n, lenp)     (*(lenp) = (n)->local_len, (const lxb_char_t *)(n)->local)
#define MKR_ATTR_LOCAL_NAME(n, lenp)     (*(lenp) = (n)->local_len, (const lxb_char_t *)(n)->local)
#define MKR_ELEM_QUALIFIED_NAME(n, lenp) (*(lenp) = (n)->qname_len, (const lxb_char_t *)(n)->qname)
#define MKR_ATTR_QUALIFIED_NAME(n, lenp) (*(lenp) = (n)->qname_len, (const lxb_char_t *)(n)->qname)
#define MKR_NODE_PI_NAME(n, lenp)        (*(lenp) = (n)->local_len, (const lxb_char_t *)(n)->local)

/* element attribute value by (raw, qualified) name; borrowed bytes or NULL. */
static inline const lxb_char_t *
mkr_xml_node_get_attribute(mkr_xml_node_t *el, const char *name, size_t nlen, size_t *vlenp)
{
    for (mkr_xml_node_t *a = el->attrs; a != NULL; a = a->next) {
        if (a->qname_len == nlen && memcmp(a->qname, name, nlen) == 0) {
            *vlenp = a->value_len;
            return (const lxb_char_t *)a->value;
        }
    }
    *vlenp = 0;
    return NULL;
}
#define MKR_ELEM_GET_ATTRIBUTE(el, name, nlen, vlenp) \
    mkr_xml_node_get_attribute((el), (const char *)(name), (nlen), (vlenp))

/* --- per-instance services (the custom node carries these directly) --- */

/* Namespace URI bytes (borrowed) or NULL (len 0) if the node is in no namespace.
 * +doc+ is unused: the node holds the resolved URI. */
#define MKR_NODE_NS_URI(node, doc, lenp) \
    (*(lenp) = (node)->ns_uri_len, (node)->ns_uri_len ? (node)->ns_uri : NULL)

/* Append a node's own text content (its value slice) to a mkr_buf_t. */
#define MKR_NODE_APPEND_OWN_TEXT(node, buf, st)                                \
    (st) = ((node)->value_len                                                  \
              ? mkr_buf_append((buf), (const lxb_char_t *)(node)->value, (node)->value_len) \
              : MKR_OK)

/* No tag-id table on the XML document; the //tag element-index fast path is
 * never enabled for XML (the element index is NULL), so this is unreachable. */
#define MKR_DOC_TAG_ID_BY_NAME(doc, ptr, len) LXB_TAG__UNDEF

#endif /* MKR_NODE_ACCESS_XML_H */
