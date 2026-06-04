/* mkr_xpath_node_access_xml.h — node-access contract bound to the custom XML node.
 *
 * The XML counterpart of mkr_xpath_node_access_html.h: it binds the MKR_NODE_* / MKR_ELEM_*
 * field-access macros + per-instance services to mkr_xml_node_t, so the shared
 * XPath engine body (mkr_xpath_*_body.h) compiles for XML with zero runtime
 * dispatch (§2.5). MKR_HOST_XML selects the engine's XML host-policy branches
 * (name-test matching, §8.6). The custom node carries its namespace URI and a
 * contiguous "prefix:local" qname directly, so there is no ns hash / tag table.
 */
#ifndef MKR_XPATH_NODE_ACCESS_XML_H
#define MKR_XPATH_NODE_ACCESS_XML_H

#include <lexbor/dom/dom.h>          /* for the MKR_XML_NODE_TYPE_* == LXB asserts + lxb_char_t */
#include "../xml/mkr_xml_node.h"
#include <string.h>

#define MKR_HOST_XML 1

/* Bind the engine's neutral node-type names to the XML representation's own
 * constants (the html binding maps the same names to LXB_DOM_NODE_TYPE_*). */
#define MKR_NTYPE_ELEMENT            MKR_XML_NODE_TYPE_ELEMENT
#define MKR_NTYPE_ATTRIBUTE          MKR_XML_NODE_TYPE_ATTRIBUTE
#define MKR_NTYPE_TEXT               MKR_XML_NODE_TYPE_TEXT
#define MKR_NTYPE_CDATA_SECTION      MKR_XML_NODE_TYPE_CDATA_SECTION
#define MKR_NTYPE_COMMENT            MKR_XML_NODE_TYPE_COMMENT
#define MKR_NTYPE_PI                 MKR_XML_NODE_TYPE_PI
#define MKR_NTYPE_DOCUMENT_TYPE      MKR_XML_NODE_TYPE_DOCUMENT_TYPE
#define MKR_NTYPE_ENTITY             MKR_XML_NODE_TYPE_ENTITY
#define MKR_NTYPE_ENTITY_REFERENCE   MKR_XML_NODE_TYPE_ENTITY_REFERENCE
#define MKR_NTYPE_NOTATION           MKR_XML_NODE_TYPE_NOTATION

/* The whole monomorphization rests on the two representations agreeing on the
 * node-type encoding (so a node's `type` integer means the same thing whichever
 * instance walks it). Enforce that agreement at compile time rather than by
 * comment — the XML constants must equal the Lexbor ones. */
_Static_assert((int)MKR_XML_NODE_TYPE_ELEMENT == (int)LXB_DOM_NODE_TYPE_ELEMENT,                "node-type encoding drift: ELEMENT");
_Static_assert((int)MKR_XML_NODE_TYPE_ATTRIBUTE == (int)LXB_DOM_NODE_TYPE_ATTRIBUTE,              "node-type encoding drift: ATTRIBUTE");
_Static_assert((int)MKR_XML_NODE_TYPE_TEXT == (int)LXB_DOM_NODE_TYPE_TEXT,                   "node-type encoding drift: TEXT");
_Static_assert((int)MKR_XML_NODE_TYPE_CDATA_SECTION == (int)LXB_DOM_NODE_TYPE_CDATA_SECTION,          "node-type encoding drift: CDATA_SECTION");
_Static_assert((int)MKR_XML_NODE_TYPE_ENTITY_REFERENCE == (int)LXB_DOM_NODE_TYPE_ENTITY_REFERENCE,       "node-type encoding drift: ENTITY_REFERENCE");
_Static_assert((int)MKR_XML_NODE_TYPE_ENTITY == (int)LXB_DOM_NODE_TYPE_ENTITY,                 "node-type encoding drift: ENTITY");
_Static_assert((int)MKR_XML_NODE_TYPE_PI == (int)LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION, "node-type encoding drift: PI");
_Static_assert((int)MKR_XML_NODE_TYPE_COMMENT == (int)LXB_DOM_NODE_TYPE_COMMENT,                "node-type encoding drift: COMMENT");
_Static_assert((int)MKR_XML_NODE_TYPE_DOCUMENT == (int)LXB_DOM_NODE_TYPE_DOCUMENT,               "node-type encoding drift: DOCUMENT");
_Static_assert((int)MKR_XML_NODE_TYPE_DOCUMENT_TYPE == (int)LXB_DOM_NODE_TYPE_DOCUMENT_TYPE,          "node-type encoding drift: DOCUMENT_TYPE");
_Static_assert((int)MKR_XML_NODE_TYPE_NOTATION == (int)LXB_DOM_NODE_TYPE_NOTATION,               "node-type encoding drift: NOTATION");

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

#endif /* MKR_XPATH_NODE_ACCESS_XML_H */
