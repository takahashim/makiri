/* ruby_cross_import.c - cross-kind subtree translation for Document#import_node.
 *
 * Makiri keeps HTML nodes (Lexbor lxb_dom_node_t) and XML nodes (mkr_xml_node_t)
 * as distinct C representations that cannot share a tree. import_node bridges
 * them: it deep/shallow-copies a subtree from one representation into the other,
 * owned by the target document, returning a DETACHED copy (the caller links it).
 *
 * The XML TUs (ext/makiri/xml) never include Lexbor, so this translation - the
 * one place both representations are read/written together - lives here in glue.
 * Both directions:
 *   - build the destination subtree DETACHED, then return it (never linking into a
 *     live tree mid-build), so a failure abandons a self-contained partial subtree
 *     in the destination arena (HTML mraw or the XML node arena), freed with the
 *     document - the same fail-closed model the XML deep-copy uses;
 *   - walk the source with an explicit heap stack (no C recursion -> no stack DoS),
 *     freed on every path;
 *   - report failure via mkr_xml_mut_status_t (never rb_raise here, which would
 *     leak the stack); the Ruby entry maps it with mkr_xml_mut_check.
 *
 * Namespaces (phase 4): preserved across the two representations.
 *   - HTML->XML: an mkr node's namespace is resolved from xmlns declarations at
 *     insertion time (resolve_node_ns), so a directly-set ns_uri would be
 *     overwritten when the imported subtree is later linked. We therefore SYNTHESIZE
 *     xmlns declarations: each element declares xmlns="URI" when its namespace
 *     differs from the one inherited from its translated parent (so unprefixed
 *     elements resolve correctly), and a foreign-prefixed attribute (e.g. xlink:*)
 *     gets an xmlns:PREFIX declaration on its element. The predefined xml: prefix
 *     needs none.
 *   - XML->HTML: Lexbor stores a namespace as an id, so the element's node.ns is set
 *     from the URI and a namespaced attribute is built with lxb_dom_attr_set_name_ns.
 *     Only the namespaces Lexbor knows by id (XHTML/SVG/MathML/XLink/XML/XMLNS) map;
 *     an unknown URI falls back to the null namespace (fail-soft).
 */
#include "cross_import.h"
#include "glue.h"
#include "../core/mkr_core.h"   /* mkr_grow_reserve, MKR_OK */

#include <lexbor/ns/ns.h>       /* lxb_ns_by_id, LXB_NS_* */
#include <stdlib.h>             /* free (the xmlns:PREFIX scratch; alloc via mkr_reallocarray) */
#include <string.h>             /* memcmp / memcpy / memchr */

/* Exported by Lexbor but omitted from its public headers: names an attribute from
 * (namespace URI, qualified name), splitting prefix/local and interning the ns.
 * (Same forward declaration ruby_html_mutate.c uses.) */
extern lxb_status_t
lxb_dom_attr_set_name_ns(lxb_dom_attr_t *attr, const lxb_char_t *link,
                         size_t link_length, const lxb_char_t *name,
                         size_t name_length, bool to_lowercase);

/* Also Lexbor-internal: intern a namespace URI in the document's ns table,
 * returning the entry (whose ns_id we set on a translated element's node.ns). */
extern const lxb_ns_data_t *
lxb_ns_append(lexbor_hash_t *hash, const lxb_char_t *link, size_t length);

mkr_node_kind_t
mkr_node_kind(VALUE v)
{
    if (rb_typeddata_is_kind_of(v, &mkr_html_node_type)) return MKR_NODE_KIND_HTML;
    if (rb_typeddata_is_kind_of(v, &mkr_xml_node_type))  return MKR_NODE_KIND_XML;
    return MKR_NODE_KIND_OTHER;
}

/* A DOM name/value slice must fit uint32 (the mkr arena's per-slice cap and the
 * factory signatures). A >4 GiB slice is rejected fail-closed rather than wrapped. */
#define MKR_FITS_U32(n) ((n) <= UINT32_MAX)

/* Intern +uri+ in the destination HTML document's namespace table and return its
 * Lexbor id, so an element's namespace survives translation for ANY URI (not just
 * the few Lexbor knows by default) - the same interning lxb_dom_attr_set_name_ns
 * does for attributes. A null/empty URI (or an intern OOM) is the null namespace. */
static lxb_ns_id_t
x2h_ns_id(lxb_dom_document_t *hdoc, const char *uri, uint32_t len)
{
    if (uri == NULL || len == 0) return LXB_NS__UNDEF;
    const lxb_ns_data_t *d = lxb_ns_append(hdoc->ns, (const lxb_char_t *)uri, len);
    return (d != NULL) ? d->ns_id : LXB_NS__UNDEF;   /* fail-soft on OOM */
}

/* The URI string for an HTML node's namespace id (borrowed from the source
 * document's interned ns table - stable for that document's lifetime), or NULL/0
 * for the null namespace. */
static const char *
mkr_html_ns_uri(lxb_dom_node_t *n, uint32_t *out_len)
{
    *out_len = 0;
    if (n->ns == LXB_NS__UNDEF) return NULL;
    size_t len = 0;
    const lxb_char_t *u = lxb_ns_by_id(n->owner_document->ns, n->ns, &len);
    if (u == NULL || !MKR_FITS_U32(len)) return NULL;
    *out_len = (uint32_t)len;
    return (const char *)u;
}

static int
mkr_uri_eq(const char *a, uint32_t al, const char *b, uint32_t bl)
{
    return al == bl && (al == 0 || memcmp(a, b, al) == 0);
}

/* ----- explicit (src, dst) work stack shared by both directions --------------
 * +def+/+deflen+ carry, for HTML->XML, the default-namespace URI in scope for the
 * destination node's children (so a child only redeclares xmlns when it differs);
 * unused (NULL/0) for XML->HTML. */
typedef struct { void *s; void *d; const char *def; uint32_t deflen; } mkr_xframe_t;
typedef struct { mkr_xframe_t *v; size_t n, cap; } mkr_xstack_t;

static int
mkr_xstack_push(mkr_xstack_t *st, void *s, void *d, const char *def, uint32_t deflen)
{
    if (mkr_grow_reserve((void **)&st->v, &st->cap, st->n + 1, sizeof(*st->v)) != MKR_OK) {
        return -1;
    }
    st->v[st->n].s = s;
    st->v[st->n].d = d;
    st->v[st->n].def = def;
    st->v[st->n].deflen = deflen;
    st->n++;
    return 0;
}

/* ===================== HTML (lxb) -> XML (mkr) =============================== */

/* Declare xmlns (prefix NULL/plen 0) or xmlns:PREFIX = uri on the detached mkr
 * element +el+, as an ordinary attribute, so the inserted subtree's prefix-based
 * namespace resolution reproduces +uri+. */
static mkr_xml_mut_status_t
h2x_declare_ns(mkr_xml_doc_t *xdoc, mkr_xml_node_t *el,
               const char *prefix, uint32_t plen, const char *uri, uint32_t ulen)
{
    if (plen == 0) {
        return mkr_xml_set_attribute(xdoc, el, "xmlns", 5, uri != NULL ? uri : "", ulen, NULL);
    }
    size_t nlen = (size_t)6 + plen;   /* "xmlns:" + prefix */
    if (!MKR_FITS_U32(nlen)) return MKR_XML_MUT_OOM;
    char *nm = mkr_reallocarray(NULL, nlen, 1);   /* overflow-checked safe alloc */
    if (nm == NULL) return MKR_XML_MUT_OOM;
    memcpy(nm, "xmlns:", 6);
    memcpy(nm + 6, prefix, plen);
    mkr_xml_mut_status_t st = mkr_xml_set_attribute(xdoc, el, nm, (uint32_t)nlen,
                                                    uri != NULL ? uri : "", ulen, NULL);
    free(nm);
    return st;
}

/* Copy +s+'s attributes onto the translated mkr element +el+, declaring an
 * xmlns:PREFIX for each foreign-prefixed attribute so resolution at link time
 * succeeds (the predefined xml: prefix needs none). */
static mkr_xml_mut_status_t
h2x_copy_attrs(mkr_xml_doc_t *xdoc, lxb_dom_node_t *s, mkr_xml_node_t *el)
{
    for (lxb_dom_attr_t *a = lxb_dom_element_first_attribute(lxb_dom_interface_element(s));
         a != NULL; a = lxb_dom_element_next_attribute(a)) {
        size_t anl, avl;
        const lxb_char_t *an = lxb_dom_attr_qualified_name(a, &anl);
        const lxb_char_t *av = lxb_dom_attr_value(a, &avl);
        if (!MKR_FITS_U32(anl) || !MKR_FITS_U32(avl)) return MKR_XML_MUT_OOM;

        lxb_ns_id_t ans = a->node.ns;
        if (ans != LXB_NS__UNDEF && ans != LXB_NS_HTML && ans != LXB_NS_XML) {
            const lxb_char_t *colon = memchr(an, ':', anl);
            if (colon != NULL) {
                uint32_t ulen;
                const char *uri = mkr_html_ns_uri(&a->node, &ulen);
                if (uri != NULL) {
                    mkr_xml_mut_status_t st = h2x_declare_ns(
                        xdoc, el, (const char *)an, (uint32_t)(colon - an), uri, ulen);
                    if (st != MKR_XML_MUT_OK) return st;
                }
            }
        }
        mkr_xml_mut_status_t st = mkr_xml_set_attribute(
            xdoc, el, (const char *)an, (uint32_t)anl,
            av != NULL ? (const char *)av : "", (uint32_t)avl, NULL);
        if (st != MKR_XML_MUT_OK) return st;
    }
    return MKR_XML_MUT_OK;
}

/* Translate ONE lxb node into a fresh mkr node (own fields + attributes, NOT its
 * children). +pdef+/+pdef_len+ is the default namespace inherited from the
 * translated parent; *cdef / *cdef_len receive the default namespace in scope for
 * THIS node's children. *out is the new node, or NULL to SKIP an unsupported type;
 * an error status fails the whole import. */
static mkr_xml_mut_status_t
h2x_make(mkr_xml_doc_t *xdoc, lxb_dom_node_t *s, const char *pdef, uint32_t pdef_len,
         const char **cdef, uint32_t *cdef_len, mkr_xml_node_t **out)
{
    *out = NULL;
    *cdef = pdef;            /* default: a non-element does not change the scope */
    *cdef_len = pdef_len;

    switch (s->type) {
    case LXB_DOM_NODE_TYPE_ELEMENT: {
        size_t nl;
        const lxb_char_t *nm = lxb_dom_element_qualified_name(lxb_dom_interface_element(s), &nl);
        if (!MKR_FITS_U32(nl)) return MKR_XML_MUT_OOM;
        mkr_xml_node_t *el = NULL;
        mkr_xml_mut_status_t st = mkr_xml_new_element(xdoc, (const char *)nm, (uint32_t)nl, &el);
        if (st != MKR_XML_MUT_OK) return st;

        /* Declare the element's default namespace iff it differs from the inherited
         * one, so this (unprefixed, like all HTML elements) element resolves to it.
         * An element with no namespace under an inherited default undeclares (xmlns=""). */
        uint32_t eul;
        const char *euri = mkr_html_ns_uri(s, &eul);
        if (!mkr_uri_eq(euri, eul, pdef, pdef_len)) {
            st = h2x_declare_ns(xdoc, el, NULL, 0, euri, eul);
            if (st != MKR_XML_MUT_OK) return st;
            *cdef = (euri != NULL) ? euri : "";
            *cdef_len = eul;
        }

        st = h2x_copy_attrs(xdoc, s, el);
        if (st != MKR_XML_MUT_OK) return st;
        *out = el;
        return MKR_XML_MUT_OK;
    }
    case LXB_DOM_NODE_TYPE_TEXT:
    case LXB_DOM_NODE_TYPE_CDATA_SECTION:
    case LXB_DOM_NODE_TYPE_COMMENT: {
        const lexbor_str_t *d = &lxb_dom_interface_character_data(s)->data;
        if (!MKR_FITS_U32(d->length)) return MKR_XML_MUT_OOM;
        uint8_t t = (s->type == LXB_DOM_NODE_TYPE_TEXT)           ? MKR_XML_NODE_TYPE_TEXT
                  : (s->type == LXB_DOM_NODE_TYPE_CDATA_SECTION)  ? MKR_XML_NODE_TYPE_CDATA_SECTION
                                                                  : MKR_XML_NODE_TYPE_COMMENT;
        return mkr_xml_new_chardata(xdoc, t, d->data != NULL ? (const char *)d->data : "",
                                    (uint32_t)d->length, out);
    }
    case LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION: {
        size_t tl;
        const lxb_char_t *tg = lxb_dom_processing_instruction_target(
            lxb_dom_interface_processing_instruction(s), &tl);
        const lexbor_str_t *d = &lxb_dom_interface_character_data(s)->data;
        if (!MKR_FITS_U32(tl) || !MKR_FITS_U32(d->length)) return MKR_XML_MUT_OOM;
        return mkr_xml_new_pi(xdoc, (const char *)tg, (uint32_t)tl,
                              d->data != NULL ? (const char *)d->data : "", (uint32_t)d->length, out);
    }
    case LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT: {
        mkr_xml_node_t *f = mkr_xml_arena_node(xdoc, MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT);
        if (f == NULL) return MKR_XML_MUT_OOM;
        *out = f;
        return MKR_XML_MUT_OK;
    }
    default:
        return MKR_XML_MUT_OK;   /* unsupported descendant type: skip (*out stays NULL) */
    }
}

/* The children to translate under +s+. An HTML <template> keeps its content in a
 * separate document fragment, NOT the normal child chain, so a plain first_child
 * walk would silently drop template contents. We descend into the content fragment
 * instead: mkr (XML) has no template-content concept, so the contents become
 * ordinary children of the translated element (lossless, the natural XML shape). */
static lxb_dom_node_t *
h2x_children_of(lxb_dom_node_t *s)
{
    if (s->type == LXB_DOM_NODE_TYPE_ELEMENT
        && s->local_name == LXB_TAG_TEMPLATE && s->ns == LXB_NS_HTML) {
        lxb_dom_document_fragment_t *content = lxb_html_interface_template(s)->content;
        return content != NULL ? lxb_dom_interface_node(content)->first_child : NULL;
    }
    return s->first_child;
}

mkr_xml_mut_status_t
mkr_cross_html_to_xml(mkr_xml_doc_t *xdoc, lxb_dom_node_t *src, int deep, mkr_xml_node_t **out)
{
    *out = NULL;
    mkr_xml_node_t *root = NULL;
    const char *rdef = NULL; uint32_t rdef_len = 0;
    mkr_xml_mut_status_t st = h2x_make(xdoc, src, NULL, 0, &rdef, &rdef_len, &root);
    if (st != MKR_XML_MUT_OK) return st;
    if (root == NULL) return MKR_XML_MUT_TYPE;   /* root node type has no XML counterpart */

    if (deep) {
        mkr_xstack_t stk = { NULL, 0, 0 };
        if (mkr_xstack_push(&stk, src, root, rdef, rdef_len) != 0) { free(stk.v); return MKR_XML_MUT_OOM; }
        while (stk.n > 0) {
            mkr_xframe_t f = stk.v[--stk.n];
            lxb_dom_node_t *s = (lxb_dom_node_t *)f.s;
            mkr_xml_node_t *d = (mkr_xml_node_t *)f.d;
            for (lxb_dom_node_t *c = h2x_children_of(s); c != NULL; c = c->next) {
                mkr_xml_node_t *dc = NULL;
                const char *cdef = NULL; uint32_t cdef_len = 0;
                st = h2x_make(xdoc, c, f.def, f.deflen, &cdef, &cdef_len, &dc);
                if (st != MKR_XML_MUT_OK) goto done;
                if (dc == NULL) continue;                 /* skipped node type */
                st = mkr_xml_insert_child(xdoc, d, dc);    /* detached parent: ns deferred */
                if (st != MKR_XML_MUT_OK) goto done;
                if (h2x_children_of(c) != NULL
                    && mkr_xstack_push(&stk, c, dc, cdef, cdef_len) != 0) {
                    st = MKR_XML_MUT_OOM; goto done;
                }
            }
        }
    done:
        free(stk.v);
        if (st != MKR_XML_MUT_OK) return st;   /* partial subtree abandoned in the arena */
    }
    *out = root;
    return MKR_XML_MUT_OK;
}

/* ===================== XML (mkr) -> HTML (lxb) =============================== */

/* Copy +s+'s attributes onto the translated lxb element +el+, preserving each
 * attribute's namespace (a null-namespace attribute via set_attribute, a
 * namespaced one via an explicit lxb_dom_attr_set_name_ns). */
static mkr_xml_mut_status_t
x2h_copy_attrs(lxb_dom_document_t *hdoc, const mkr_xml_node_t *s, lxb_dom_element_t *el)
{
    for (const mkr_xml_node_t *a = s->attrs; a != NULL; a = a->next) {
        const char *val = a->value != NULL ? a->value : "";
        if (a->ns_uri_len == 0) {
            if (lxb_dom_element_set_attribute(el, (const lxb_char_t *)a->qname, a->qname_len,
                                              (const lxb_char_t *)val, a->value_len) == NULL) {
                return MKR_XML_MUT_OOM;
            }
            continue;
        }
        lxb_dom_attr_t *at = lxb_dom_attr_interface_create(hdoc);
        if (at == NULL) return MKR_XML_MUT_OOM;
        if (lxb_dom_attr_set_name_ns(at, (const lxb_char_t *)a->ns_uri, a->ns_uri_len,
                                     (const lxb_char_t *)a->qname, a->qname_len, false) != LXB_STATUS_OK
            || lxb_dom_attr_set_value(at, (const lxb_char_t *)val, a->value_len) != LXB_STATUS_OK) {
            return MKR_XML_MUT_OOM;   /* the un-appended attr is abandoned in mraw */
        }
        lxb_dom_element_attr_append(el, at);
    }
    return MKR_XML_MUT_OK;
}

/* Translate ONE mkr node into a fresh, detached lxb node (own fields + attributes,
 * NOT children). *out is the new node, or NULL to SKIP an unsupported type. An XML
 * CDATA section has no HTML counterpart, so it fails closed (MKR_XML_MUT_TYPE). */
static mkr_xml_mut_status_t
x2h_make(lxb_dom_document_t *hdoc, const mkr_xml_node_t *s, lxb_dom_node_t **out)
{
    *out = NULL;
    switch (s->type) {
    case MKR_XML_NODE_TYPE_ELEMENT: {
        lxb_dom_element_t *el = lxb_dom_document_create_element(
            hdoc, (const lxb_char_t *)s->qname, s->qname_len, NULL);
        if (el == NULL) return MKR_XML_MUT_OOM;
        /* Preserve the namespace as a Lexbor id (any URI, interned; else null). */
        lxb_dom_interface_node(el)->ns = x2h_ns_id(hdoc, s->ns_uri, s->ns_uri_len);
        mkr_xml_mut_status_t st = x2h_copy_attrs(hdoc, s, el);
        if (st != MKR_XML_MUT_OK) return st;
        *out = lxb_dom_interface_node(el);
        return MKR_XML_MUT_OK;
    }
    case MKR_XML_NODE_TYPE_TEXT: {
        lxb_dom_text_t *t = lxb_dom_document_create_text_node(
            hdoc, (const lxb_char_t *)(s->value != NULL ? s->value : ""), s->value_len);
        if (t == NULL) return MKR_XML_MUT_OOM;
        *out = lxb_dom_interface_node(t);
        return MKR_XML_MUT_OK;
    }
    case MKR_XML_NODE_TYPE_COMMENT: {
        lxb_dom_comment_t *c = lxb_dom_document_create_comment(
            hdoc, (const lxb_char_t *)(s->value != NULL ? s->value : ""), s->value_len);
        if (c == NULL) return MKR_XML_MUT_OOM;
        *out = lxb_dom_interface_node(c);
        return MKR_XML_MUT_OK;
    }
    case MKR_XML_NODE_TYPE_PI: {
        /* The PI target is the node's name (local == qname for a PI); data is value. */
        lxb_dom_processing_instruction_t *pi = lxb_dom_document_create_processing_instruction(
            hdoc, (const lxb_char_t *)s->local, s->local_len,
            (const lxb_char_t *)(s->value != NULL ? s->value : ""), s->value_len);
        if (pi == NULL) return MKR_XML_MUT_OOM;
        *out = lxb_dom_interface_node(pi);
        return MKR_XML_MUT_OK;
    }
    case MKR_XML_NODE_TYPE_CDATA_SECTION:
        return MKR_XML_MUT_TYPE;   /* HTML has no CDATA section: fail closed */
    case MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT: {
        lxb_dom_document_fragment_t *f = lxb_dom_document_create_document_fragment(hdoc);
        if (f == NULL) return MKR_XML_MUT_OOM;
        *out = lxb_dom_interface_node(f);
        return MKR_XML_MUT_OK;
    }
    default:
        return MKR_XML_MUT_OK;   /* unsupported descendant type: skip (*out stays NULL) */
    }
}

/* Where a translated element's CHILDREN attach. An HTML <template> holds its
 * content in a separate document fragment (HTMLTemplateElement.content), not the
 * normal child chain, so children go there - matching a parsed template and the
 * HTML->HTML import_node fixup (mkr_fixup_template_content). Other elements link
 * children directly. */
static lxb_dom_node_t *
x2h_link_target(lxb_dom_node_t *el)
{
    if (el->type == LXB_DOM_NODE_TYPE_ELEMENT
        && el->local_name == LXB_TAG_TEMPLATE && el->ns == LXB_NS_HTML) {
        lxb_dom_document_fragment_t *content = lxb_html_interface_template(el)->content;
        if (content != NULL) return lxb_dom_interface_node(content);
    }
    return el;
}

mkr_xml_mut_status_t
mkr_cross_xml_to_html(lxb_dom_document_t *hdoc, const mkr_xml_node_t *src, int deep,
                      lxb_dom_node_t **out)
{
    *out = NULL;
    lxb_dom_node_t *root = NULL;
    mkr_xml_mut_status_t st = x2h_make(hdoc, src, &root);
    if (st != MKR_XML_MUT_OK) return st;
    if (root == NULL) return MKR_XML_MUT_TYPE;   /* root node type has no HTML counterpart */

    if (deep) {
        mkr_xstack_t stk = { NULL, 0, 0 };
        /* The frame's d is the link target for the source node's children (a
         * template element's content fragment, else the element itself). */
        if (mkr_xstack_push(&stk, (void *)src, x2h_link_target(root), NULL, 0) != 0) {
            free(stk.v); return MKR_XML_MUT_OOM;
        }
        while (stk.n > 0) {
            mkr_xframe_t f = stk.v[--stk.n];
            const mkr_xml_node_t *s = (const mkr_xml_node_t *)f.s;
            lxb_dom_node_t *d = (lxb_dom_node_t *)f.d;
            for (const mkr_xml_node_t *c = s->first_child; c != NULL; c = c->next) {
                lxb_dom_node_t *dc = NULL;
                st = x2h_make(hdoc, c, &dc);
                if (st != MKR_XML_MUT_OK) goto done;
                if (dc == NULL) continue;                 /* skipped node type */
                lxb_dom_node_insert_child(d, dc);
                if (c->first_child != NULL
                    && mkr_xstack_push(&stk, (void *)c, x2h_link_target(dc), NULL, 0) != 0) {
                    st = MKR_XML_MUT_OOM; goto done;
                }
            }
        }
    done:
        free(stk.v);
        if (st != MKR_XML_MUT_OK) return st;   /* partial subtree abandoned in mraw */
    }
    *out = root;
    return MKR_XML_MUT_OK;
}
