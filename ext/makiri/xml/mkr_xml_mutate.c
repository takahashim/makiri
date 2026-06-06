/* mkr_xml_mutate.c — Ruby-free XML tree mutation primitives.
 *
 * See mkr_xml_mutate.h for the contract. Phase 1 (in-place edits: rename,
 * attribute set/remove, content set, detach) and Phase 2 (building: node
 * factories, deep-copy import, and insertion that resolves the inserted subtree's
 * namespaces against its new context). Every primitive validates first and
 * allocates all arena bytes — and, for insertion, resolves namespaces — BEFORE
 * mutating any node link, so a failure (bad name, bad chars, unbound prefix,
 * cycle, hierarchy, OOM) leaves the tree untouched, even for a move. The
 * namespace resolver mirrors the parser's §7 rules (mkr_xml_tree.c) so a mutated
 * node's ns_uri is identical to what a re-parse would compute.
 */
#include "mkr_xml_mutate.h"
#include "mkr_xml.h"   /* mkr_xml_is_name_start/char, mkr_xml_utf8_decode, mkr_xml_validate_chars */
#include "../core/mkr_core.h"   /* mkr_grow_reserve (the iterative import stack) */

#include <stdlib.h>    /* free (import stack) */
#include <string.h>

#define XML_NS_URI    "http://www.w3.org/XML/1998/namespace"
#define XMLNS_NS_URI  "http://www.w3.org/2000/xmlns/"
#define LIT_LEN(s)    ((uint32_t)(sizeof(s) - 1))

static int
slice_eq(const char *a, uint32_t al, const char *b, uint32_t bl)
{
    return al == bl && (al == 0 || memcmp(a, b, al) == 0);
}

/* Validate that [s, s+len) is a well-formed XML 1.0 QName and split it into
 * prefix:local (pfx_len 0 = no prefix). Mirrors scan_name + split_qname in the
 * tree builder: the whole name is NameStartChar NameChar* (a colon is a NameChar),
 * then at most one colon separates two NCNames. 0 on success, -1 if malformed. */
static int
qname_split(const char *s, uint32_t len, const char **pfx, uint32_t *pl,
            const char **loc, uint32_t *ll)
{
    if (len == 0) return -1;
    const char *p = s, *end = s + len;
    uint32_t cp;
    int bl = mkr_xml_utf8_decode(p, end, &cp);
    if (bl == 0 || !mkr_xml_is_name_start(cp)) return -1;
    p += bl;
    while (p < end) {
        bl = mkr_xml_utf8_decode(p, end, &cp);
        if (bl == 0 || !mkr_xml_is_name_char(cp)) return -1;
        p += bl;
    }
    const char *colon = memchr(s, ':', len);
    if (colon == NULL) { *pfx = s; *pl = 0; *loc = s; *ll = len; return 0; }
    uint32_t prefix_len = (uint32_t)(colon - s);
    const char *ls = colon + 1;
    uint32_t local_len = len - prefix_len - 1;
    if (prefix_len == 0 || local_len == 0) return -1;             /* ":x" or "x:" */
    if (memchr(ls, ':', local_len) != NULL) return -1;           /* a second colon */
    if (mkr_xml_utf8_decode(ls, ls + local_len, &cp) == 0 || !mkr_xml_is_name_start(cp))
        return -1;                                               /* local must be an NCName */
    *pfx = s; *pl = prefix_len; *loc = ls; *ll = local_len;
    return 0;
}

/* Nearest in-scope binding for +prefix+ (pl 0 = the default namespace) at or
 * above +node+, walking the real tree (no scope dictionary is threaded). */
static int
resolve_in_scope(const mkr_xml_node_t *node, const char *prefix, uint32_t pl,
                 const char **uri, uint32_t *ulen)
{
    for (const mkr_xml_node_t *e = node; e != NULL; e = e->parent) {
        if (e->type != MKR_XML_NODE_TYPE_ELEMENT) continue;
        for (mkr_xml_node_t *a = e->attrs; a != NULL; a = a->next) {
            const char *p, *u; uint32_t apl, ul;
            if (mkr_xml_node_xmlns_decl(a, &p, &apl, &u, &ul)
                && apl == pl && (pl == 0 || memcmp(p, prefix, pl) == 0)) {
                *uri = u; *ulen = ul; return 1;
            }
        }
    }
    return 0;
}

/* True if +node+ is part of the live document tree (its topmost ancestor is the
 * document node), as opposed to a detached fragment still being assembled. An
 * unbound prefix is a hard error only once connected (so the live tree stays
 * serializable to well-formed XML); on a detached node it is deferred — left
 * unresolved until the fragment is inserted, when resolution runs again. */
static int
is_connected(const mkr_xml_node_t *node)
{
    const mkr_xml_node_t *top = node;
    while (top->parent != NULL) top = top->parent;
    return top->type == MKR_XML_NODE_TYPE_DOCUMENT;
}

/* Resolve the namespace for the QName [qname,qlen] (already split into pfx/pl)
 * applied at +scope+ (the element whose in-scope declarations apply). +is_attr+
 * selects attribute rules (xmlns* declarations live in the xmlns namespace, an
 * unprefixed attribute is in no namespace). +connected+ decides what an unbound
 * prefix means: an error in the live tree, deferred (unresolved) when detached.
 * Mirrors mkr_xml_tree.c §7. */
static mkr_xml_mut_status_t
resolve_ns(const mkr_xml_node_t *scope, const char *qname, uint32_t qlen,
           const char *pfx, uint32_t pl, int is_attr, int connected,
           const char **uri, uint32_t *ulen)
{
    *uri = NULL; *ulen = 0;
    if (is_attr) {
        int is_default_decl = (qlen == 5 && memcmp(qname, "xmlns", 5) == 0);
        int is_pfx_decl     = (qlen > 6 && memcmp(qname, "xmlns:", 6) == 0);
        if (is_default_decl || is_pfx_decl) {
            *uri = XMLNS_NS_URI; *ulen = LIT_LEN(XMLNS_NS_URI);
            return MKR_XML_MUT_OK;
        }
    }
    if (pl == 0) {
        if (is_attr) return MKR_XML_MUT_OK;       /* unprefixed attribute -> no namespace */
        const char *u; uint32_t ul;               /* unprefixed element -> the default ns */
        if (resolve_in_scope(scope, "", 0, &u, &ul) && ul > 0) { *uri = u; *ulen = ul; }
        return MKR_XML_MUT_OK;
    }
    if (slice_eq(pfx, pl, "xml", 3)) {             /* the predefined xml: prefix */
        *uri = XML_NS_URI; *ulen = LIT_LEN(XML_NS_URI);
        return MKR_XML_MUT_OK;
    }
    if (slice_eq(pfx, pl, "xmlns", 5)) return MKR_XML_MUT_BAD_NAME;  /* xmlns: as a normal name */
    const char *u; uint32_t ul;
    if (!resolve_in_scope(scope, pfx, pl, &u, &ul) || ul == 0) {
        return connected ? MKR_XML_MUT_UNBOUND_NS : MKR_XML_MUT_OK;  /* defer when detached */
    }
    *uri = u; *ulen = ul;
    return MKR_XML_MUT_OK;
}

/* Copy the QName into the arena as ONE contiguous slice and point qname/local/
 * prefix into it (the read API and serializers borrow these). +loc+ points into
 * +name+ (from qname_split). */
static mkr_xml_mut_status_t
assign_qname(mkr_xml_doc_t *doc, mkr_xml_node_t *node, const char *name, uint32_t nl,
             const char *loc, uint32_t ll, uint32_t pl)
{
    const char *q = mkr_xml_arena_bytes(doc, name, nl);
    if (nl > 0 && q == NULL) return MKR_XML_MUT_OOM;
    node->qname  = q;                          node->qname_len  = nl;
    node->local  = q + (size_t)(loc - name);   node->local_len  = ll;
    node->prefix = q;                          node->prefix_len = pl;
    return MKR_XML_MUT_OK;
}

void
mkr_xml_detach(mkr_xml_node_t *node)
{
    mkr_xml_node_t *parent = node->parent;
    if (parent == NULL) return;
    if (node->type == MKR_XML_NODE_TYPE_ATTRIBUTE) {
        mkr_xml_node_t *prev = NULL;
        for (mkr_xml_node_t *a = parent->attrs; a != NULL; prev = a, a = a->next) {
            if (a == node) {
                if (prev) prev->next = a->next; else parent->attrs = a->next;
                break;
            }
        }
        node->next = NULL; node->parent = NULL;
        return;
    }
    if (node->prev) node->prev->next = node->next; else parent->first_child = node->next;
    if (node->next) node->next->prev = node->prev; else parent->last_child  = node->prev;
    node->parent = node->prev = node->next = NULL;
}

mkr_xml_mut_status_t
mkr_xml_rename(mkr_xml_doc_t *doc, mkr_xml_node_t *node, const char *name, uint32_t nlen)
{
    if (node->type != MKR_XML_NODE_TYPE_ELEMENT && node->type != MKR_XML_NODE_TYPE_ATTRIBUTE)
        return MKR_XML_MUT_TYPE;
    const char *pfx, *loc; uint32_t pl, ll;
    if (qname_split(name, nlen, &pfx, &pl, &loc, &ll) != 0) return MKR_XML_MUT_BAD_NAME;

    int is_attr = (node->type == MKR_XML_NODE_TYPE_ATTRIBUTE);
    const mkr_xml_node_t *scope = is_attr ? node->parent : node;
    const char *uri; uint32_t ulen;
    int connected = scope != NULL && is_connected(scope);
    mkr_xml_mut_status_t st = resolve_ns(scope, name, nlen, pfx, pl, is_attr, connected, &uri, &ulen);
    if (st != MKR_XML_MUT_OK) return st;

    /* Copy the new qname BEFORE writing any field, so an OOM leaves node intact. */
    const char *q = mkr_xml_arena_bytes(doc, name, nlen);
    if (nlen > 0 && q == NULL) return MKR_XML_MUT_OOM;
    node->qname  = q;                          node->qname_len  = nlen;
    node->local  = q + (size_t)(loc - name);   node->local_len  = ll;
    node->prefix = q;                          node->prefix_len = pl;
    node->ns_uri = uri;                        node->ns_uri_len = ulen;
    return MKR_XML_MUT_OK;
}

mkr_xml_mut_status_t
mkr_xml_set_attribute(mkr_xml_doc_t *doc, mkr_xml_node_t *el, const char *name, uint32_t nlen,
                      const char *val, uint32_t vlen, mkr_xml_node_t **out)
{
    if (out) *out = NULL;
    if (el->type != MKR_XML_NODE_TYPE_ELEMENT) return MKR_XML_MUT_TYPE;
    const char *pfx, *loc; uint32_t pl, ll;
    if (qname_split(name, nlen, &pfx, &pl, &loc, &ll) != 0) return MKR_XML_MUT_BAD_NAME;
    if (vlen > 0 && mkr_xml_validate_chars(val, vlen) != 0) return MKR_XML_MUT_BAD_CHARS;

    const char *uri; uint32_t ulen;
    mkr_xml_mut_status_t st = resolve_ns(el, name, nlen, pfx, pl, 1, is_connected(el), &uri, &ulen);
    if (st != MKR_XML_MUT_OK) return st;

    /* An existing attribute with the same raw QName -> replace its value. */
    for (mkr_xml_node_t *a = el->attrs; a != NULL; a = a->next) {
        if (slice_eq(a->qname, a->qname_len, name, nlen)) {
            const char *nv = mkr_xml_arena_bytes(doc, val, vlen);
            if (vlen > 0 && nv == NULL) return MKR_XML_MUT_OOM;
            a->value = nv ? nv : ""; a->value_len = vlen;
            a->ns_uri = uri; a->ns_uri_len = ulen;
            if (out) *out = a;
            return MKR_XML_MUT_OK;
        }
    }

    /* New attribute — allocate node + qname + value before linking. */
    mkr_xml_node_t *attr = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_ATTRIBUTE);
    if (attr == NULL) return MKR_XML_MUT_OOM;
    st = assign_qname(doc, attr, name, nlen, loc, ll, pl);
    if (st != MKR_XML_MUT_OK) return st;          /* abandoned in the arena; freed with doc */
    const char *nv = mkr_xml_arena_bytes(doc, val, vlen);
    if (vlen > 0 && nv == NULL) return MKR_XML_MUT_OOM;
    attr->value = nv ? nv : ""; attr->value_len = vlen;
    attr->ns_uri = uri; attr->ns_uri_len = ulen;
    attr->parent = el;

    if (el->attrs == NULL) {
        el->attrs = attr;
    } else {
        mkr_xml_node_t *t = el->attrs;          /* O(attrs) tail find; bounded per element */
        while (t->next != NULL) t = t->next;
        t->next = attr;
    }
    if (out) *out = attr;
    return MKR_XML_MUT_OK;
}

int
mkr_xml_remove_attribute(mkr_xml_node_t *el, const char *name, uint32_t nlen)
{
    if (el->type != MKR_XML_NODE_TYPE_ELEMENT) return 0;
    mkr_xml_node_t *prev = NULL;
    for (mkr_xml_node_t *a = el->attrs; a != NULL; prev = a, a = a->next) {
        if (slice_eq(a->qname, a->qname_len, name, nlen)) {
            if (prev) prev->next = a->next; else el->attrs = a->next;
            a->next = NULL; a->parent = NULL;
            return 1;
        }
    }
    return 0;
}

mkr_xml_mut_status_t
mkr_xml_set_content(mkr_xml_doc_t *doc, mkr_xml_node_t *node, const char *text, uint32_t tlen)
{
    if (tlen > 0 && mkr_xml_validate_chars(text, tlen) != 0) return MKR_XML_MUT_BAD_CHARS;

    switch (node->type) {
    case MKR_XML_NODE_TYPE_TEXT:
    case MKR_XML_NODE_TYPE_CDATA_SECTION:
    case MKR_XML_NODE_TYPE_COMMENT:
    case MKR_XML_NODE_TYPE_PI: {
        const char *nv = mkr_xml_arena_bytes(doc, text, tlen);
        if (tlen > 0 && nv == NULL) return MKR_XML_MUT_OOM;
        node->value = nv ? nv : ""; node->value_len = tlen;
        return MKR_XML_MUT_OK;
    }
    case MKR_XML_NODE_TYPE_ELEMENT: {
        /* Build the replacement TEXT node FIRST, so an OOM leaves the children
         * intact (no half-applied content). */
        mkr_xml_node_t *t = NULL;
        if (tlen > 0) {
            const char *nv = mkr_xml_arena_bytes(doc, text, tlen);
            if (nv == NULL) return MKR_XML_MUT_OOM;
            t = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_TEXT);
            if (t == NULL) return MKR_XML_MUT_OOM;
            t->value = nv; t->value_len = tlen;
        }
        for (mkr_xml_node_t *c = node->first_child; c != NULL;) {
            mkr_xml_node_t *nx = c->next;
            c->parent = c->prev = c->next = NULL;   /* detach each former child */
            c = nx;
        }
        node->first_child = node->last_child = t;   /* t may be NULL (empty element) */
        if (t != NULL) t->parent = node;
        return MKR_XML_MUT_OK;
    }
    default:
        return MKR_XML_MUT_TYPE;
    }
}

/* ============================ Phase 2: building ============================ */

/* True if [s,len) is the reserved PI target "xml" in any case (§2.6). */
static int
is_reserved_pi_target(const char *s, uint32_t len)
{
    return len == 3 && (s[0] | 0x20) == 'x' && (s[1] | 0x20) == 'm' && (s[2] | 0x20) == 'l';
}

mkr_xml_mut_status_t
mkr_xml_new_element(mkr_xml_doc_t *doc, const char *name, uint32_t nlen, mkr_xml_node_t **out)
{
    *out = NULL;
    const char *pfx, *loc; uint32_t pl, ll;
    if (qname_split(name, nlen, &pfx, &pl, &loc, &ll) != 0) return MKR_XML_MUT_BAD_NAME;
    if (slice_eq(pfx, pl, "xmlns", 5)) return MKR_XML_MUT_BAD_NAME;  /* xmlns: is not an element prefix */
    mkr_xml_node_t *el = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_ELEMENT);
    if (el == NULL) return MKR_XML_MUT_OOM;
    mkr_xml_mut_status_t st = assign_qname(doc, el, name, nlen, loc, ll, pl);
    if (st != MKR_XML_MUT_OK) return st;
    *out = el;                              /* ns_uri stays unresolved until insertion */
    return MKR_XML_MUT_OK;
}

mkr_xml_mut_status_t
mkr_xml_new_chardata(mkr_xml_doc_t *doc, uint8_t type, const char *text, uint32_t tlen,
                     mkr_xml_node_t **out)
{
    *out = NULL;
    if (type != MKR_XML_NODE_TYPE_TEXT && type != MKR_XML_NODE_TYPE_CDATA_SECTION
        && type != MKR_XML_NODE_TYPE_COMMENT) {
        return MKR_XML_MUT_TYPE;
    }
    if (tlen > 0 && mkr_xml_validate_chars(text, tlen) != 0) return MKR_XML_MUT_BAD_CHARS;
    mkr_xml_node_t *n = mkr_xml_arena_node(doc, type);
    if (n == NULL) return MKR_XML_MUT_OOM;
    const char *v = mkr_xml_arena_bytes(doc, text, tlen);
    if (tlen > 0 && v == NULL) return MKR_XML_MUT_OOM;
    n->value = v ? v : ""; n->value_len = tlen;
    *out = n;
    return MKR_XML_MUT_OK;
}

mkr_xml_mut_status_t
mkr_xml_new_pi(mkr_xml_doc_t *doc, const char *target, uint32_t tlen,
               const char *data, uint32_t dlen, mkr_xml_node_t **out)
{
    *out = NULL;
    /* PITarget is an NCName (a QName with no colon) and not "xml" in any case. */
    const char *pfx, *loc; uint32_t pl, ll;
    if (qname_split(target, tlen, &pfx, &pl, &loc, &ll) != 0 || pl != 0) return MKR_XML_MUT_BAD_NAME;
    if (is_reserved_pi_target(target, tlen)) return MKR_XML_MUT_BAD_NAME;
    if (dlen > 0 && mkr_xml_validate_chars(data, dlen) != 0) return MKR_XML_MUT_BAD_CHARS;
    /* The data may not contain "?>", which would close the PI early. */
    for (uint32_t i = 1; i < dlen; i++) {
        if (data[i] == '>' && data[i - 1] == '?') return MKR_XML_MUT_BAD_CHARS;
    }
    mkr_xml_node_t *pi = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_PI);
    if (pi == NULL) return MKR_XML_MUT_OOM;
    const char *t = mkr_xml_arena_bytes(doc, target, tlen);
    if (tlen > 0 && t == NULL) return MKR_XML_MUT_OOM;
    const char *d = mkr_xml_arena_bytes(doc, data, dlen);
    if (dlen > 0 && d == NULL) return MKR_XML_MUT_OOM;
    pi->local = t; pi->local_len = tlen;       /* the PI target (Node#name) */
    pi->value = d ? d : ""; pi->value_len = dlen;
    *out = pi;
    return MKR_XML_MUT_OK;
}

/* Resolve the namespace of every element (and prefixed attribute) in +root+'s
 * subtree against the in-scope declarations reachable from each node — which,
 * because the caller has temporarily pointed root->parent at the prospective
 * context, includes that context and its ancestors. Order-independent (it reads
 * xmlns attribute values, never resolved ns_uri). Iterative pre-order, no
 * recursion. Fails closed on an unbound prefix, having changed no link. */
static mkr_xml_mut_status_t
resolve_node_ns(mkr_xml_node_t *e, int connected)
{
    const char *uri; uint32_t ulen;
    mkr_xml_mut_status_t st =
        resolve_ns(e, e->qname, e->qname_len, e->prefix, e->prefix_len, 0, connected, &uri, &ulen);
    if (st != MKR_XML_MUT_OK) return st;
    e->ns_uri = uri; e->ns_uri_len = ulen;
    for (mkr_xml_node_t *a = e->attrs; a != NULL; a = a->next) {
        st = resolve_ns(e, a->qname, a->qname_len, a->prefix, a->prefix_len, 1, connected, &uri, &ulen);
        if (st != MKR_XML_MUT_OK) return st;
        a->ns_uri = uri; a->ns_uri_len = ulen;
    }
    return MKR_XML_MUT_OK;
}

static mkr_xml_mut_status_t
resolve_subtree(mkr_xml_node_t *root, int connected)
{
    for (mkr_xml_node_t *cur = root; cur != NULL;) {
        if (cur->type == MKR_XML_NODE_TYPE_ELEMENT) {
            mkr_xml_mut_status_t st = resolve_node_ns(cur, connected);
            if (st != MKR_XML_MUT_OK) return st;
        }
        if (cur->first_child != NULL) { cur = cur->first_child; continue; }
        while (cur != root && cur->next == NULL) cur = cur->parent;
        if (cur == root) break;
        cur = cur->next;
    }
    return MKR_XML_MUT_OK;
}

/* Resolve +node+'s subtree as if it were a child of +context+, WITHOUT linking it
 * (so a resolution failure leaves the tree untouched): temporarily borrow
 * node->parent for the ancestor walk, then restore it. An unbound prefix errors
 * only when +context+ is part of the live document (is_connected via the borrowed
 * parent); inserting into a still-detached fragment defers resolution. */
static mkr_xml_mut_status_t
resolve_into(mkr_xml_node_t *node, mkr_xml_node_t *context)
{
    mkr_xml_node_t *saved = node->parent;
    node->parent = context;
    mkr_xml_mut_status_t st = resolve_subtree(node, is_connected(node));
    node->parent = saved;
    return st;
}

/* A single arena copy of +src+ (its own fields + attributes, NOT its children).
 * Namespaces are left unresolved. Returns NULL on OOM. */
static mkr_xml_node_t *
copy_one(mkr_xml_doc_t *doc, const mkr_xml_node_t *src)
{
    mkr_xml_node_t *n = mkr_xml_arena_node(doc, src->type);
    if (n == NULL) return NULL;
    if (src->qname != NULL && src->qname_len > 0) {       /* element / attribute QName */
        if (assign_qname(doc, n, src->qname, src->qname_len,
                         src->qname + (size_t)(src->local - src->qname),
                         src->local_len, src->prefix_len) != MKR_XML_MUT_OK) {
            return NULL;
        }
    } else if (src->local != NULL && src->local_len > 0) { /* PI target */
        const char *t = mkr_xml_arena_bytes(doc, src->local, src->local_len);
        if (t == NULL) return NULL;
        n->local = t; n->local_len = src->local_len;
    }
    if (src->value_len > 0) {
        const char *v = mkr_xml_arena_bytes(doc, src->value, src->value_len);
        if (v == NULL) return NULL;
        n->value = v; n->value_len = src->value_len;
    } else if (src->value != NULL) {
        n->value = "";
    }
    /* copy attributes (each an arena node), preserving order */
    mkr_xml_node_t *tail = NULL;
    for (mkr_xml_node_t *a = src->attrs; a != NULL; a = a->next) {
        mkr_xml_node_t *ca = copy_one(doc, a);     /* an attribute has no children/attrs */
        if (ca == NULL) return NULL;
        ca->parent = n;
        if (tail) tail->next = ca; else n->attrs = ca;
        tail = ca;
    }
    return n;
}

mkr_xml_mut_status_t
mkr_xml_import_subtree(mkr_xml_doc_t *doc, const mkr_xml_node_t *src, mkr_xml_node_t **out)
{
    *out = NULL;
    mkr_xml_node_t *root = copy_one(doc, src);
    if (root == NULL) return MKR_XML_MUT_OOM;

    /* Iterative pre-order: an explicit heap stack of (src, dst) pairs copies each
     * src node's children under its dst copy. No C recursion -> no stack DoS on a
     * deep constructed tree. */
    typedef struct { const mkr_xml_node_t *s; mkr_xml_node_t *d; } frame_t;
    frame_t *stack = NULL; size_t cap = 0, top = 0;
    mkr_xml_mut_status_t st = MKR_XML_MUT_OK;

    if (mkr_grow_reserve((void **)&stack, &cap, 1, sizeof(*stack)) != MKR_OK) {
        return MKR_XML_MUT_OOM;
    }
    stack[top].s = src; stack[top].d = root; top++;
    while (top > 0) {
        frame_t f = stack[--top];
        mkr_xml_node_t *dtail = NULL;
        for (const mkr_xml_node_t *sc = f.s->first_child; sc != NULL; sc = sc->next) {
            mkr_xml_node_t *dc = copy_one(doc, sc);
            if (dc == NULL) { st = MKR_XML_MUT_OOM; goto done; }
            dc->parent = f.d;
            if (dtail) { dtail->next = dc; dc->prev = dtail; }
            else f.d->first_child = dc;
            f.d->last_child = dc;
            dtail = dc;
            if (sc->first_child != NULL) {
                if (mkr_grow_reserve((void **)&stack, &cap, top + 1, sizeof(*stack)) != MKR_OK) {
                    st = MKR_XML_MUT_OOM; goto done;
                }
                stack[top].s = sc; stack[top].d = dc; top++;
            }
        }
    }
done:
    free(stack);
    if (st != MKR_XML_MUT_OK) return st;       /* partial copy abandoned in the arena */
    *out = root;
    return MKR_XML_MUT_OK;
}

/* ---- insertion ---- */

/* +node+ may be placed in the tree at all (not an attribute / document / doctype,
 * which are never element-level children). */
static int
is_insertable(const mkr_xml_node_t *node)
{
    switch (node->type) {
    case MKR_XML_NODE_TYPE_ELEMENT:
    case MKR_XML_NODE_TYPE_TEXT:
    case MKR_XML_NODE_TYPE_CDATA_SECTION:
    case MKR_XML_NODE_TYPE_COMMENT:
    case MKR_XML_NODE_TYPE_PI:
        return 1;
    default:
        return 0;
    }
}

/* +node+ must not be an inclusive ancestor of +container+ (else a cycle). */
static int
would_cycle(const mkr_xml_node_t *container, const mkr_xml_node_t *node)
{
    for (const mkr_xml_node_t *p = container; p != NULL; p = p->parent) {
        if (p == node) return 1;
    }
    return 0;
}

/* The XML single-root rule: the document node holds at most one ELEMENT child.
 * True if linking +node+ under document +container+ is allowed (i.e. node is not
 * an element, or there is no existing element child other than +exclude+, which
 * is the node being replaced). */
static int
doc_root_ok(const mkr_xml_node_t *container, const mkr_xml_node_t *node,
            const mkr_xml_node_t *exclude)
{
    if (container->type != MKR_XML_NODE_TYPE_DOCUMENT) return 1;
    if (node->type != MKR_XML_NODE_TYPE_ELEMENT) return 1;   /* comments/PIs are fine at doc level */
    for (const mkr_xml_node_t *c = container->first_child; c != NULL; c = c->next) {
        if (c != exclude && c != node && c->type == MKR_XML_NODE_TYPE_ELEMENT) return 0;
    }
    return 1;
}

/* After a structural change whose container is the document node, point doc->root
 * at the (single) element child, or NULL. */
static void
sync_doc_root(mkr_xml_doc_t *doc, const mkr_xml_node_t *container)
{
    if (container != doc->doc_node) return;
    doc->root = NULL;
    for (mkr_xml_node_t *c = doc->doc_node->first_child; c != NULL; c = c->next) {
        if (c->type == MKR_XML_NODE_TYPE_ELEMENT) { doc->root = c; break; }
    }
}

/* Shared validation + namespace resolution for an insertion of +node+ under
 * +container+ (the future parent), replacing +exclude+ (or NULL). Performs NO
 * structural change; on success the caller commits the link. */
static mkr_xml_mut_status_t
prepare_insert(mkr_xml_node_t *container, mkr_xml_node_t *node, const mkr_xml_node_t *exclude)
{
    if (!is_insertable(node)) return MKR_XML_MUT_HIERARCHY;
    if (container->type != MKR_XML_NODE_TYPE_ELEMENT
        && container->type != MKR_XML_NODE_TYPE_DOCUMENT) {
        return MKR_XML_MUT_HIERARCHY;                  /* only elements / the document hold children */
    }
    if (would_cycle(container, node)) return MKR_XML_MUT_CYCLE;
    if (!doc_root_ok(container, node, exclude)) return MKR_XML_MUT_HIERARCHY;
    return resolve_into(node, container);              /* fail-closed: no link changed yet */
}

mkr_xml_mut_status_t
mkr_xml_insert_child(mkr_xml_doc_t *doc, mkr_xml_node_t *parent, mkr_xml_node_t *node)
{
    mkr_xml_mut_status_t st = prepare_insert(parent, node, NULL);
    if (st != MKR_XML_MUT_OK) return st;
    mkr_xml_detach(node);                              /* move semantics */
    node->parent = parent;
    node->prev = parent->last_child;
    node->next = NULL;
    if (parent->last_child) parent->last_child->next = node; else parent->first_child = node;
    parent->last_child = node;
    sync_doc_root(doc, parent);
    return MKR_XML_MUT_OK;
}

mkr_xml_mut_status_t
mkr_xml_insert_before(mkr_xml_doc_t *doc, mkr_xml_node_t *ref, mkr_xml_node_t *node)
{
    mkr_xml_node_t *container = ref->parent;
    if (container == NULL) return MKR_XML_MUT_HIERARCHY;   /* a parentless ref has no sibling slot */
    mkr_xml_mut_status_t st = prepare_insert(container, node, NULL);
    if (st != MKR_XML_MUT_OK) return st;
    mkr_xml_detach(node);
    node->parent = container;
    node->next = ref;
    node->prev = ref->prev;
    if (ref->prev) ref->prev->next = node; else container->first_child = node;
    ref->prev = node;
    sync_doc_root(doc, container);
    return MKR_XML_MUT_OK;
}

mkr_xml_mut_status_t
mkr_xml_insert_after(mkr_xml_doc_t *doc, mkr_xml_node_t *ref, mkr_xml_node_t *node)
{
    mkr_xml_node_t *container = ref->parent;
    if (container == NULL) return MKR_XML_MUT_HIERARCHY;
    mkr_xml_mut_status_t st = prepare_insert(container, node, NULL);
    if (st != MKR_XML_MUT_OK) return st;
    mkr_xml_detach(node);
    node->parent = container;
    node->prev = ref;
    node->next = ref->next;
    if (ref->next) ref->next->prev = node; else container->last_child = node;
    ref->next = node;
    sync_doc_root(doc, container);
    return MKR_XML_MUT_OK;
}

mkr_xml_mut_status_t
mkr_xml_replace_node(mkr_xml_doc_t *doc, mkr_xml_node_t *ref, mkr_xml_node_t *node)
{
    mkr_xml_node_t *container = ref->parent;
    if (container == NULL) return MKR_XML_MUT_HIERARCHY;   /* a parentless / document ref */
    if (node == ref) return MKR_XML_MUT_OK;
    mkr_xml_mut_status_t st = prepare_insert(container, node, ref);
    if (st != MKR_XML_MUT_OK) return st;
    mkr_xml_detach(node);
    node->parent = container;
    node->prev = ref->prev;
    node->next = ref->next;
    if (ref->prev) ref->prev->next = node; else container->first_child = node;
    if (ref->next) ref->next->prev = node; else container->last_child = node;
    ref->parent = ref->prev = ref->next = NULL;           /* detach the replaced node */
    sync_doc_root(doc, container);
    return MKR_XML_MUT_OK;
}

/* ---- self-test (Makiri.__c_selftest) ---- */
int
mkr_xml_mutate_selftest(void)
{
    mkr_xml_doc_t *doc = mkr_xml_doc_new();
    if (doc == NULL) return 1;
    int rc = 0;
    const char *pfx, *loc; uint32_t pl, ll;

    /* 1. QName validation + split */
    if (qname_split("a:b", 3, &pfx, &pl, &loc, &ll) != 0 || pl != 1 || ll != 1) { rc = 2;  goto done; }
    if (qname_split("1bad", 4, &pfx, &pl, &loc, &ll) == 0)  { rc = 3;  goto done; } /* bad NameStartChar */
    if (qname_split("a:b:c", 5, &pfx, &pl, &loc, &ll) == 0) { rc = 4;  goto done; } /* two colons */
    if (qname_split(":x", 2, &pfx, &pl, &loc, &ll) == 0)    { rc = 5;  goto done; } /* empty prefix */
    if (qname_split("x:", 2, &pfx, &pl, &loc, &ll) == 0)    { rc = 6;  goto done; } /* empty local */

    /* 2. build a root element, exercise attribute set/replace */
    mkr_xml_node_t *r = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_ELEMENT);
    if (r == NULL) { rc = 7; goto done; }
    if (assign_qname(doc, r, "r", 1, "r", 1, 0) != MKR_XML_MUT_OK) { rc = 8; goto done; }

    mkr_xml_node_t *at = NULL;
    if (mkr_xml_set_attribute(doc, r, "id", 2, "x", 1, &at) != MKR_XML_MUT_OK
        || at == NULL || at->value_len != 1 || at->value[0] != 'x' || r->attrs != at) { rc = 9; goto done; }
    if (mkr_xml_set_attribute(doc, r, "id", 2, "yy", 2, &at) != MKR_XML_MUT_OK
        || at->value_len != 2 || r->attrs->next != NULL) { rc = 10; goto done; } /* replaced, still one */

    /* 3. fail-closed: non-XML-Char value. An unbound prefix on a DETACHED element
     *    defers (left unresolved), not an error — the live-tree error is exercised
     *    in the Phase 2 checks below. */
    if (mkr_xml_set_attribute(doc, r, "k", 1, "\x01", 1, NULL) != MKR_XML_MUT_BAD_CHARS) { rc = 11; goto done; }
    mkr_xml_node_t *det = NULL;
    if (mkr_xml_new_element(doc, "det", 3, &det) != MKR_XML_MUT_OK
        || mkr_xml_set_attribute(doc, det, "p:k", 3, "v", 1, &at) != MKR_XML_MUT_OK
        || at->ns_uri_len != 0) { rc = 12; goto done; }

    /* 4. the predefined xml: prefix resolves with no declaration */
    if (mkr_xml_set_attribute(doc, r, "xml:lang", 8, "en", 2, &at) != MKR_XML_MUT_OK
        || !slice_eq(at->ns_uri, at->ns_uri_len, XML_NS_URI, LIT_LEN(XML_NS_URI))) { rc = 13; goto done; }

    /* 5. adding an xmlns:* declaration lands in the xmlns namespace and then binds
     *    a previously-unbound prefix */
    if (mkr_xml_set_attribute(doc, r, "xmlns:p", 7, "urn:p", 5, &at) != MKR_XML_MUT_OK
        || !slice_eq(at->ns_uri, at->ns_uri_len, XMLNS_NS_URI, LIT_LEN(XMLNS_NS_URI))) { rc = 14; goto done; }
    if (mkr_xml_set_attribute(doc, r, "p:k", 3, "v", 1, &at) != MKR_XML_MUT_OK
        || !slice_eq(at->ns_uri, at->ns_uri_len, "urn:p", 5)) { rc = 15; goto done; }

    /* 6. remove by name (idempotent) */
    if (mkr_xml_remove_attribute(r, "id", 2) != 1) { rc = 16; goto done; }
    if (mkr_xml_remove_attribute(r, "id", 2) != 0) { rc = 17; goto done; }

    /* 7. rename the element + re-resolve namespace */
    if (mkr_xml_rename(doc, r, "q", 1) != MKR_XML_MUT_OK
        || r->qname_len != 1 || r->qname[0] != 'q' || r->ns_uri_len != 0) { rc = 18; goto done; }

    /* 8. content: replace children with text, then empty it */
    mkr_xml_node_t *c1 = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_ELEMENT);
    if (c1 == NULL) { rc = 19; goto done; }
    c1->parent = r; r->first_child = r->last_child = c1;
    if (mkr_xml_set_content(doc, r, "hi", 2) != MKR_XML_MUT_OK) { rc = 20; goto done; }
    if (r->first_child == NULL || r->first_child->type != MKR_XML_NODE_TYPE_TEXT
        || r->first_child->value_len != 2 || r->last_child != r->first_child
        || c1->parent != NULL) { rc = 21; goto done; }
    if (mkr_xml_set_content(doc, r, "", 0) != MKR_XML_MUT_OK || r->first_child != NULL
        || r->last_child != NULL) { rc = 22; goto done; }

    /* 9. detach a middle/edge tree child, sibling links stay consistent */
    mkr_xml_node_t *a1 = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_ELEMENT);
    mkr_xml_node_t *a2 = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_ELEMENT);
    if (a1 == NULL || a2 == NULL) { rc = 23; goto done; }
    a1->parent = r; a2->parent = r; a1->next = a2; a2->prev = a1;
    r->first_child = a1; r->last_child = a2;
    mkr_xml_detach(a1);
    if (r->first_child != a2 || a2->prev != NULL || a1->parent != NULL) { rc = 24; goto done; }
    mkr_xml_detach(a2);
    if (r->first_child != NULL || r->last_child != NULL || a2->parent != NULL) { rc = 25; goto done; }

    /* 10. Phase 2 — a live document node + a connected root carrying xmlns:p */
    mkr_xml_node_t *docn = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_DOCUMENT);
    if (docn == NULL) { rc = 26; goto done; }
    doc->doc_node = docn;
    mkr_xml_node_t *pr = NULL, *ne = NULL, *tx = NULL;
    if (mkr_xml_new_element(doc, "pr", 2, &pr) != MKR_XML_MUT_OK || pr == NULL
        || pr->parent != NULL || pr->ns_uri_len != 0) { rc = 27; goto done; }
    if (mkr_xml_set_attribute(doc, pr, "xmlns:p", 7, "urn:p", 5, NULL) != MKR_XML_MUT_OK) { rc = 28; goto done; }
    if (mkr_xml_insert_child(doc, docn, pr) != MKR_XML_MUT_OK || doc->root != pr
        || !is_connected(pr)) { rc = 29; goto done; }
    if (mkr_xml_new_chardata(doc, MKR_XML_NODE_TYPE_TEXT, "hi", 2, &tx) != MKR_XML_MUT_OK
        || tx->value_len != 2) { rc = 30; goto done; }

    /* 11. insert_child links + resolves the inserted subtree's namespace (p bound on pr) */
    if (mkr_xml_new_element(doc, "p:c", 3, &ne) != MKR_XML_MUT_OK) { rc = 31; goto done; }
    if (mkr_xml_insert_child(doc, pr, ne) != MKR_XML_MUT_OK
        || pr->first_child != ne || ne->parent != pr
        || !slice_eq(ne->ns_uri, ne->ns_uri_len, "urn:p", 5)) { rc = 32; goto done; }
    if (mkr_xml_insert_child(doc, ne, tx) != MKR_XML_MUT_OK || ne->first_child != tx) { rc = 33; goto done; }

    /* 12. an unbound prefix is a hard error in the live tree, leaving it unchanged */
    mkr_xml_node_t *ub = NULL;
    if (mkr_xml_new_element(doc, "z:c", 3, &ub) != MKR_XML_MUT_OK) { rc = 34; goto done; }
    if (mkr_xml_insert_child(doc, pr, ub) != MKR_XML_MUT_UNBOUND_NS
        || ub->parent != NULL || pr->last_child != ne) { rc = 35; goto done; }

    /* 13. deferred resolution: build a detached fragment whose prefix is unbound
     *     there, then connect it — resolution succeeds against the live context */
    mkr_xml_node_t *wrap = NULL, *inner = NULL;
    if (mkr_xml_new_element(doc, "p:wrap", 6, &wrap) != MKR_XML_MUT_OK
        || mkr_xml_new_element(doc, "p:inner", 7, &inner) != MKR_XML_MUT_OK) { rc = 36; goto done; }
    if (mkr_xml_insert_child(doc, wrap, inner) != MKR_XML_MUT_OK
        || inner->ns_uri_len != 0) { rc = 37; goto done; }   /* deferred while detached */
    if (mkr_xml_insert_child(doc, pr, wrap) != MKR_XML_MUT_OK
        || !slice_eq(wrap->ns_uri, wrap->ns_uri_len, "urn:p", 5)
        || !slice_eq(inner->ns_uri, inner->ns_uri_len, "urn:p", 5)) { rc = 38; goto done; }

    /* 14. cycle rejection: a node cannot be inserted under its own descendant */
    if (mkr_xml_insert_child(doc, ne, pr) != MKR_XML_MUT_CYCLE) { rc = 39; goto done; }

    /* 15. sibling order: before / after place correctly */
    mkr_xml_node_t *b1 = NULL, *b2 = NULL;
    if (mkr_xml_new_element(doc, "b1", 2, &b1) != MKR_XML_MUT_OK
        || mkr_xml_new_element(doc, "b2", 2, &b2) != MKR_XML_MUT_OK) { rc = 40; goto done; }
    if (mkr_xml_insert_before(doc, ne, b1) != MKR_XML_MUT_OK || pr->first_child != b1
        || b1->next != ne) { rc = 41; goto done; }
    if (mkr_xml_insert_after(doc, ne, b2) != MKR_XML_MUT_OK || ne->next != b2) { rc = 42; goto done; }

    /* 16. replace swaps a node in place (detach-never-destroy of the old node) */
    mkr_xml_node_t *rep = NULL;
    if (mkr_xml_new_element(doc, "rep", 3, &rep) != MKR_XML_MUT_OK) { rc = 43; goto done; }
    if (mkr_xml_replace_node(doc, ne, rep) != MKR_XML_MUT_OK
        || ne->parent != NULL || rep->parent != pr || b1->next != rep) { rc = 44; goto done; }

    /* 17. cross-document import: a deep, independent copy in another arena */
    mkr_xml_doc_t *doc2 = mkr_xml_doc_new();
    if (doc2 == NULL) { rc = 45; goto done; }
    mkr_xml_node_t *imp = NULL;
    int irc = mkr_xml_import_subtree(doc2, pr, &imp);
    if (irc != MKR_XML_MUT_OK || imp == NULL || imp == pr
        || imp->qname_len != 2 || memcmp(imp->qname, "pr", 2) != 0
        || imp->first_child == NULL || imp->first_child == pr->first_child) { rc = 46; }
    mkr_xml_doc_destroy(doc2);
    if (rc != 0) goto done;

    /* 18. single-root rule at the document level (pr is already the root) */
    mkr_xml_node_t *root2 = NULL;
    if (mkr_xml_new_element(doc, "root2", 5, &root2) != MKR_XML_MUT_OK) { rc = 47; goto done; }
    if (mkr_xml_insert_child(doc, docn, root2) != MKR_XML_MUT_HIERARCHY) { rc = 48; goto done; }

done:
    mkr_xml_doc_destroy(doc);
    return rc;
}
