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
 * Phase: namespaces are NOT yet translated (null-ns). Names/values are carried as
 * raw bytes; the destination factories re-validate (XML QName / XML Char) and fail
 * closed, so a name not well-formed in the target representation raises.
 */
#include "cross_import.h"
#include "glue.h"
#include "../core/mkr_core.h"   /* mkr_grow_reserve, MKR_OK */

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

/* ----- explicit (src, dst) work stack shared by both directions -------------- */

typedef struct { void *s; void *d; } mkr_xframe_t;
typedef struct { mkr_xframe_t *v; size_t n, cap; } mkr_xstack_t;

static int
mkr_xstack_push(mkr_xstack_t *st, void *s, void *d)
{
    if (mkr_grow_reserve((void **)&st->v, &st->cap, st->n + 1, sizeof(*st->v)) != MKR_OK) {
        return -1;
    }
    st->v[st->n].s = s;
    st->v[st->n].d = d;
    st->n++;
    return 0;
}

/* ===================== HTML (lxb) -> XML (mkr) =============================== */

/* Translate ONE lxb node into a fresh mkr node (its own fields + attributes, NOT
 * its children). *out is the new node, or NULL to SKIP an unsupported type (e.g.
 * a stray document node); an error status fails the whole import. */
static mkr_xml_mut_status_t
h2x_make(mkr_xml_doc_t *xdoc, lxb_dom_node_t *s, mkr_xml_node_t **out)
{
    *out = NULL;
    switch (s->type) {
    case LXB_DOM_NODE_TYPE_ELEMENT: {
        size_t nl;
        const lxb_char_t *nm = lxb_dom_element_qualified_name(lxb_dom_interface_element(s), &nl);
        if (!MKR_FITS_U32(nl)) return MKR_XML_MUT_OOM;
        mkr_xml_node_t *el = NULL;
        mkr_xml_mut_status_t st = mkr_xml_new_element(xdoc, (const char *)nm, (uint32_t)nl, &el);
        if (st != MKR_XML_MUT_OK) return st;
        for (lxb_dom_attr_t *a = lxb_dom_element_first_attribute(lxb_dom_interface_element(s));
             a != NULL; a = lxb_dom_element_next_attribute(a)) {
            size_t anl, avl;
            const lxb_char_t *an = lxb_dom_attr_qualified_name(a, &anl);
            const lxb_char_t *av = lxb_dom_attr_value(a, &avl);
            if (!MKR_FITS_U32(anl) || !MKR_FITS_U32(avl)) return MKR_XML_MUT_OOM;
            /* Detached element: an unknown prefix's namespace is deferred (not an
             * error) by set_attribute, so the raw QName + value carry across. */
            st = mkr_xml_set_attribute(xdoc, el, (const char *)an, (uint32_t)anl,
                                       av != NULL ? (const char *)av : "", (uint32_t)avl, NULL);
            if (st != MKR_XML_MUT_OK) return st;
        }
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
    mkr_xml_mut_status_t st = h2x_make(xdoc, src, &root);
    if (st != MKR_XML_MUT_OK) return st;
    if (root == NULL) return MKR_XML_MUT_TYPE;   /* root node type has no XML counterpart */

    if (deep) {
        mkr_xstack_t stk = { NULL, 0, 0 };
        if (mkr_xstack_push(&stk, src, root) != 0) { free(stk.v); return MKR_XML_MUT_OOM; }
        while (stk.n > 0) {
            mkr_xframe_t f = stk.v[--stk.n];
            lxb_dom_node_t *s = (lxb_dom_node_t *)f.s;
            mkr_xml_node_t *d = (mkr_xml_node_t *)f.d;
            for (lxb_dom_node_t *c = h2x_children_of(s); c != NULL; c = c->next) {
                mkr_xml_node_t *dc = NULL;
                st = h2x_make(xdoc, c, &dc);
                if (st != MKR_XML_MUT_OK) goto done;
                if (dc == NULL) continue;                 /* skipped node type */
                st = mkr_xml_insert_child(xdoc, d, dc);    /* detached parent: ns deferred */
                if (st != MKR_XML_MUT_OK) goto done;
                if (h2x_children_of(c) != NULL && mkr_xstack_push(&stk, c, dc) != 0) {
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
        for (const mkr_xml_node_t *a = s->attrs; a != NULL; a = a->next) {
            lxb_dom_attr_t *at = lxb_dom_element_set_attribute(
                el, (const lxb_char_t *)a->qname, a->qname_len,
                (const lxb_char_t *)(a->value != NULL ? a->value : ""), a->value_len);
            if (at == NULL) return MKR_XML_MUT_OOM;
        }
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
        if (mkr_xstack_push(&stk, (void *)src, root) != 0) { free(stk.v); return MKR_XML_MUT_OOM; }
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
                if (c->first_child != NULL && mkr_xstack_push(&stk, (void *)c, dc) != 0) {
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
