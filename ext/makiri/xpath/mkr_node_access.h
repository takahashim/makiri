/* mkr_node_access.h — node-access contract for the XPath engine.
 *
 * The engine's per-node work (navigation, name tests, namespace tests) is
 * expressed through these MKR_NODE_* / MKR_ELEM_* macros instead of touching a
 * concrete node struct directly. This lets the same engine logic be compiled
 * for two node representations with ZERO runtime dispatch (compile-time
 * monomorphization — measured ~0% vs direct field reads, whereas a runtime
 * kind-branch costs ~+150%; see tmp/xml_spike/accessor_perf.c and
 * docs/xml_parser_plan.ja.md §2.5):
 *
 *   - HTML: this header, accessors → lxb_dom_node_t fields / Lexbor lookups.
 *   - XML (planned, §8.5): a sibling header binding the same macros to the
 *     custom mkr_xml_node_t, then `#include`-ing the shared engine body.
 *
 * For the HTML instance every macro expands to exactly the field read / call
 * the engine used before, so the compiled object code (and `rake bench`) is
 * unchanged by construction. Node-type numeric values are shared between the
 * representations (the custom XML node reuses LXB_DOM_NODE_TYPE_* values), so
 * the type switches in the engine stay representation-neutral.
 */
#ifndef MKR_NODE_ACCESS_H
#define MKR_NODE_ACCESS_H

#include <lexbor/dom/dom.h>
#include <lexbor/ns/ns.h>

/* (The DOM node handle for this instance is lxb_dom_node_t; the XML instance
 * will bind these macros to mkr_xml_node_t. No engine-wide typedef here — the
 * engine already uses mkr_node_t for its XPath AST node.) */

/* --- neutral node-type constants ---
 * The shared engine body compares MKR_NODE_TYPE(n) against these instead of the
 * Lexbor LXB_DOM_NODE_TYPE_* names, so the body is representation-neutral. For
 * the HTML instance they ARE the Lexbor values (byte-identical codegen); the XML
 * node reuses the same numeric values.
 *
 * NB: named MKR_NTYPE_* (node *type*), distinct from the engine's existing
 * mkr_nt_kind_t enum MKR_NT_* (node *test* kinds: text()/comment()/pi()/...).
 * An earlier MKR_NT_* name collided with that enum and broke //comment() etc. */
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

/* --- type / namespace-id (numeric, shared values) --- */
#define MKR_NODE_TYPE(n)          ((n)->type)
#define MKR_NODE_NS_ID(n)         ((n)->ns)

/* --- navigation (return the node handle for this instance) --- */
#define MKR_NODE_FIRST_CHILD(n)   ((n)->first_child)
#define MKR_NODE_LAST_CHILD(n)    ((n)->last_child)
#define MKR_NODE_NEXT(n)          ((n)->next)
#define MKR_NODE_PREV(n)          ((n)->prev)
#define MKR_NODE_PARENT(n)        ((n)->parent)

/* --- element / attribute handles & iteration --- */
#define MKR_NODE_AS_ELEMENT(n)    lxb_dom_interface_element(n)   /* node -> element handle */
#define MKR_ELEM_FIRST_ATTR(el)   ((el)->first_attr)
#define MKR_ATTR_NEXT(a)          ((a)->next)
#define MKR_ATTR_VALUE(a, lenp)   lxb_dom_attr_value((a), (lenp)) /* borrowed value bytes */

/* Strict element name tests resolve in the HTML namespace, so a foreign
 * (non-HTML, non-null) element namespace is a non-match. The XML instance will
 * redefine this against its own namespace representation. */
#define MKR_NODE_IS_FOREIGN_NS(n) \
    ((n)->ns != LXB_NS_HTML && (n)->ns != LXB_NS__UNDEF)

/* --- name lookups (return borrowed lxb_char_t* bytes, set *(lenp)) --- */
#define MKR_ELEM_LOCAL_NAME(n, lenp) \
    lxb_dom_element_local_name((lxb_dom_element_t *)(n), (lenp))
#define MKR_ATTR_LOCAL_NAME(n, lenp) \
    lxb_dom_attr_local_name((lxb_dom_attr_t *)(n), (lenp))
#define MKR_ELEM_QUALIFIED_NAME(n, lenp) \
    mkr_dom_node_name_qualified((n), (lenp))
#define MKR_ATTR_QUALIFIED_NAME(n, lenp) \
    lxb_dom_attr_qualified_name((lxb_dom_attr_t *)(n), (lenp))
#define MKR_NODE_PI_NAME(n, lenp) \
    lxb_dom_node_name((n), (lenp))

/* element attribute value by (raw) name; returns borrowed bytes or NULL. */
#define MKR_ELEM_GET_ATTRIBUTE(el, name, nlen, vlenp) \
    lxb_dom_element_get_attribute((el), (const lxb_char_t *)(name), (nlen), (vlenp))

/* --- per-instance services (resolved against the concrete representation) ---
 * The engine body uses these instead of calling Lexbor directly, so the XML
 * instance can bind them to the custom node (which carries ns_uri / value
 * directly and has no Lexbor ns hash / tag table). */

/* Borrowed namespace-URI bytes for a node (or NULL, *lenp=0, if it has none).
 * HTML resolves the node's ns-id against the document's ns hash. */
static inline const char *
mkr_html_node_ns_uri(lxb_dom_node_t *node, lxb_dom_document_t *doc, size_t *lenp)
{
    *lenp = 0;
    if (node->ns == LXB_NS__UNDEF || doc == NULL || doc->ns == NULL) return NULL;
    return (const char *)lxb_ns_by_id(doc->ns, node->ns, lenp);
}
#define MKR_NODE_NS_URI(node, doc, lenp) mkr_html_node_ns_uri((node), (doc), (lenp))

/* Append a node's own text content to a mkr_buf_t, setting `st` (mkr_status_t).
 * HTML uses lxb_dom_node_text_content (allocates; freed after the append). */
#define MKR_NODE_APPEND_OWN_TEXT(node, buf, st)                              \
    do {                                                                     \
        size_t mkr__tlen = 0;                                                \
        lxb_char_t *mkr__t = lxb_dom_node_text_content((node), &mkr__tlen);  \
        (st) = (mkr__t == NULL) ? MKR_OK : mkr_buf_append((buf), mkr__t, mkr__tlen); \
        if (mkr__t) lxb_dom_document_destroy_text((node)->owner_document, mkr__t); \
    } while (0)

/* Resolve a tag-name to a Lexbor tag id for the //tag element-index fast path.
 * Only ever reached when the element index is present (HTML); for XML the index
 * is NULL so the fast path bails before this, but it must still compile. */
#define MKR_DOC_TAG_ID_BY_NAME(doc, ptr, len) \
    lxb_tag_id_by_name((doc)->tags, (const lxb_char_t *)(ptr), (len))

#endif /* MKR_NODE_ACCESS_H */
