/* mkr_xml_tree.c — minimal XML tokenizer + tree builder (§14 steps 5-6).
 *
 * Ruby-free. Parses a decoded, validated-UTF-8 byte buffer (§2.1) into the
 * custom node arena. MINIMAL subset: elements, attributes, character data, and
 * the well-formedness errors that fall out of that grammar (unclosed / mismatched
 * tags, multiple roots, raw '<' in attribute values, content outside the root).
 * Namespaces (§7), entities (§9.1), comments/CDATA/PI/XML-decl/DOCTYPE (§9) and
 * strict Name-char validation (§9.2b) land in later steps; for now '&' and the
 * names are taken literally and '<!'/'<?' are rejected as unsupported.
 *
 * Budgets (§4): element depth (MKR_XML_MAX_DEPTH) and node/attr counts (enforced
 * in the arena) are fail-closed — a violation aborts with no partial document.
 */
#include "mkr_xml.h"
#include "mkr_xml_node.h"
#include "../core/mkr_core.h"

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* A namespace binding in scope: prefix ("" for the default ns) -> URI. The
 * prefix slice borrows the input (valid for the whole parse); the URI is
 * arena-owned (nodes share it via ns_uri). */
typedef struct {
    const char *pfx; uint32_t pfx_len;
    const char *uri; uint32_t uri_len;   /* uri_len 0 = "no namespace" (xmlns="") */
} ns_binding_t;

/* One attribute collected from a start tag before namespaces are resolved
 * (xmlns declarations on the same element must be processed first). */
typedef struct {
    const char *name; uint32_t name_len;   /* raw QName slice (input) */
    const char *val;  uint32_t val_len;    /* raw value slice (input, pre-expansion) */
} raw_attr_t;

typedef struct {
    const char     *p, *end;
    uint32_t        line, col;       /* 1-based; col in bytes (§5) */
    mkr_xml_doc_t  *doc;
    mkr_xml_status_t status;
    ns_binding_t   *binds; size_t nbind, bind_cap;   /* namespace scope stack */
    raw_attr_t     *ratt;  size_t nratt, ratt_cap;   /* reusable per-tag attr buffer */
} mkr_xml_parser_t;

#define XML_NS_URI    "http://www.w3.org/XML/1998/namespace"
#define XMLNS_NS_URI  "http://www.w3.org/2000/xmlns/"
#define LIT_LEN(s)    ((uint32_t)(sizeof(s) - 1))

static void
set_syntax(mkr_xml_parser_t *P)
{
    if (P->status == MKR_XML_OK) P->status = MKR_XML_ERR_SYNTAX;
}

/* Propagate an arena failure (node/attr/byte budget or OOM) recorded on the doc. */
static int
propagate_oom(mkr_xml_parser_t *P)
{
    if (P->doc->oom != MKR_XML_OK) { P->status = P->doc->oom; return 1; }
    return 0;
}

static void
advance(mkr_xml_parser_t *P)
{
    if (P->p >= P->end) return;
    char c = *P->p++;
    if (c == '\n') { P->line++; P->col = 1; }
    else           { P->col++; }
}

static void
skip_ws(mkr_xml_parser_t *P)
{
    while (P->p < P->end) {
        char c = *P->p;
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r') advance(P);
        else break;
    }
}

/* Minimal Name char set (permissive ASCII; strict NameStartChar/NameChar with
 * Unicode ranges arrives in §9.2b). Good enough to tokenize element/attr names. */
static int is_name_start(unsigned char c) {
    return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || c == '_' || c == ':'
        || c >= 0x80; /* allow UTF-8 lead/continuation bytes; refined in §9.2b */
}
static int is_name_char(unsigned char c) {
    return is_name_start(c) || (c >= '0' && c <= '9') || c == '-' || c == '.';
}

/* Scan a Name into [*out, *out+*len) (a slice of the input). 0 on success. */
static int
scan_name(mkr_xml_parser_t *P, const char **out, uint32_t *len)
{
    const char *start = P->p;
    if (P->p >= P->end || !is_name_start((unsigned char)*P->p)) { set_syntax(P); return -1; }
    advance(P);
    while (P->p < P->end && is_name_char((unsigned char)*P->p)) advance(P);
    size_t n = (size_t)(P->p - start);
    if (n > UINT32_MAX) { P->status = MKR_XML_ERR_LIMIT; return -1; } /* fail closed, never truncate */
    *out = start;
    *len = (uint32_t)n;
    return 0;
}

static void
append_child(mkr_xml_node_t *parent, mkr_xml_node_t *child)
{
    child->parent = parent;
    if (parent->last_child) { parent->last_child->next = child; child->prev = parent->last_child; }
    else parent->first_child = child;
    parent->last_child = child;
}

static void
append_attr(mkr_xml_node_t *el, mkr_xml_node_t *attr)
{
    attr->parent = el;
    if (el->attrs == NULL) { el->attrs = attr; return; }
    mkr_xml_node_t *a = el->attrs;
    while (a->next) a = a->next;
    a->next = attr;
}

/* Copy a slice into the arena; on failure record the arena status. NULL return
 * with len>0 means budget/OOM (P->status set); len==0 yields "". */
static const char *
own(mkr_xml_parser_t *P, const char *src, uint32_t len)
{
    const char *p = mkr_xml_arena_bytes(P->doc, src, len);
    if (p == NULL && len > 0) propagate_oom(P);
    return p;
}

static int
slice_eq(const char *a, uint32_t al, const char *b, uint32_t bl)
{
    return al == bl && (al == 0 || memcmp(a, b, al) == 0);
}

/* Split a QName into prefix:local. 0 on success (no-colon names get prefix_len
 * 0); -1 if malformed (>1 colon, or an empty prefix/local). */
static int
split_qname(const char *name, uint32_t len, const char **pfx, uint32_t *pfx_len,
            const char **loc, uint32_t *loc_len)
{
    const char *colon = memchr(name, ':', len);
    if (colon == NULL) { *pfx = name; *pfx_len = 0; *loc = name; *loc_len = len; return 0; }
    uint32_t pl = (uint32_t)(colon - name);
    const char *ls = colon + 1;
    uint32_t ll = len - pl - 1;
    if (pl == 0 || ll == 0) return -1;                  /* ":x" or "x:" */
    if (memchr(ls, ':', ll) != NULL) return -1;         /* second colon */
    *pfx = name; *pfx_len = pl; *loc = ls; *loc_len = ll;
    return 0;
}

/* Resolve a prefix to its in-scope namespace URI, or NULL when unbound. The
 * predefined 'xml' prefix always resolves; for the empty prefix this returns the
 * default-namespace URI (possibly "" with *uri_len 0 = undeclared) or NULL. */
static const char *
ns_lookup(mkr_xml_parser_t *P, const char *pfx, uint32_t pfx_len, uint32_t *uri_len)
{
    if (slice_eq(pfx, pfx_len, "xml", 3)) { *uri_len = LIT_LEN(XML_NS_URI); return XML_NS_URI; }
    for (size_t i = P->nbind; i > 0; i--) {
        ns_binding_t *b = &P->binds[i - 1];
        if (slice_eq(b->pfx, b->pfx_len, pfx, pfx_len)) { *uri_len = b->uri_len; return b->uri; }
    }
    return NULL;
}

static int
push_binding(mkr_xml_parser_t *P, const char *pfx, uint32_t pfx_len,
             const char *uri, uint32_t uri_len)
{
    if (P->nbind + 1 > MKR_XML_MAX_NS) { P->status = MKR_XML_ERR_LIMIT; return -1; }
    if (mkr_grow_reserve((void **)&P->binds, &P->bind_cap, P->nbind + 1, sizeof(*P->binds)) != MKR_OK) {
        P->status = MKR_XML_ERR_OOM; return -1;
    }
    P->binds[P->nbind].pfx = pfx; P->binds[P->nbind].pfx_len = pfx_len;
    P->binds[P->nbind].uri = uri; P->binds[P->nbind].uri_len = uri_len;
    P->nbind++;
    return 0;
}

/* Parse a start tag's attributes and close, resolving namespaces (§7). Collects
 * the attributes, processes xmlns declarations into scope bindings, resolves the
 * element's own namespace, then creates the attribute nodes (xmlns declarations
 * kept as DOM attributes, §7.2). On '>' sets *pushed; on '/>' leaves a leaf.
 * The element's prefix/local are already set by the caller; bindings pushed here
 * are popped by the caller when the element closes. Returns 0 / -1. */
static int
parse_element_body(mkr_xml_parser_t *P, mkr_xml_node_t *el, int *pushed)
{
    *pushed = 0;
    P->nratt = 0;

    /* Phase 1: scan all attributes + the tag close into the raw buffer. */
    for (;;) {
        skip_ws(P);
        if (P->p >= P->end) { set_syntax(P); return -1; }       /* unterminated tag */
        char c = *P->p;
        if (c == '>') { advance(P); *pushed = 1; break; }
        if (c == '/') {
            advance(P);
            if (P->p >= P->end || *P->p != '>') { set_syntax(P); return -1; }
            advance(P); break;                                  /* self-closing */
        }
        const char *an; uint32_t alen;
        if (scan_name(P, &an, &alen) != 0) return -1;
        skip_ws(P);
        if (P->p >= P->end || *P->p != '=') { set_syntax(P); return -1; }
        advance(P); skip_ws(P);
        if (P->p >= P->end || (*P->p != '"' && *P->p != '\'')) { set_syntax(P); return -1; }
        char q = *P->p; advance(P);
        const char *vs = P->p;
        while (P->p < P->end && *P->p != q) {
            if (*P->p == '<') { set_syntax(P); return -1; }     /* raw '<' in AttValue */
            advance(P);
        }
        if (P->p >= P->end) { set_syntax(P); return -1; }       /* unterminated value */
        size_t vraw = (size_t)(P->p - vs);
        if (vraw > UINT32_MAX) { P->status = MKR_XML_ERR_LIMIT; return -1; } /* fail closed */
        uint32_t vlen = (uint32_t)vraw;
        advance(P);
        if (P->nratt + 1 > MKR_XML_MAX_ATTRS) { P->status = MKR_XML_ERR_LIMIT; return -1; }
        if (mkr_grow_reserve((void **)&P->ratt, &P->ratt_cap, P->nratt + 1, sizeof(*P->ratt)) != MKR_OK) {
            P->status = MKR_XML_ERR_OOM; return -1;
        }
        P->ratt[P->nratt].name = an; P->ratt[P->nratt].name_len = alen;
        P->ratt[P->nratt].val  = vs; P->ratt[P->nratt].val_len  = vlen;
        P->nratt++;
    }

    /* Phase 2: xmlns declarations -> namespace bindings (§7.1 reserved rules). */
    for (size_t i = 0; i < P->nratt; i++) {
        raw_attr_t *r = &P->ratt[i];
        int is_default  = slice_eq(r->name, r->name_len, "xmlns", 5);
        int is_prefixed = (r->name_len > 6 && memcmp(r->name, "xmlns:", 6) == 0);
        if (!is_default && !is_prefixed) continue;

        const char *bpfx = ""; uint32_t bpl = 0;
        if (is_prefixed) { bpfx = r->name + 6; bpl = r->name_len - 6; }
        if (slice_eq(bpfx, bpl, "xmlns", 5)) { set_syntax(P); return -1; }   /* xmlns:xmlns reserved */

        uint32_t ulen;
        const char *uri = mkr_xml_expand(P->doc, r->val, r->val_len,
                                         MKR_XML_EXPAND_ATTR, &ulen, &P->status);
        if (uri == NULL) return -1;
        if (slice_eq(bpfx, bpl, "xml", 3)) {
            if (!slice_eq(uri, ulen, XML_NS_URI, LIT_LEN(XML_NS_URI))) { set_syntax(P); return -1; }
        } else if (slice_eq(uri, ulen, XML_NS_URI, LIT_LEN(XML_NS_URI))
                || slice_eq(uri, ulen, XMLNS_NS_URI, LIT_LEN(XMLNS_NS_URI))) {
            set_syntax(P); return -1;                  /* reserved URI bound to another prefix */
        }
        if (is_prefixed && ulen == 0) { set_syntax(P); return -1; } /* xmlns:p="" (XML 1.0) */
        if (push_binding(P, bpfx, bpl, uri, ulen) != 0) return -1;
    }

    /* Phase 3: resolve the element's own namespace. */
    if (el->prefix_len > 0) {
        if (slice_eq(el->prefix, el->prefix_len, "xmlns", 5)) { set_syntax(P); return -1; }
        uint32_t ulen;
        const char *uri = ns_lookup(P, el->prefix, el->prefix_len, &ulen);
        if (uri == NULL) { set_syntax(P); return -1; }              /* unbound prefix */
        el->ns_uri = uri; el->ns_uri_len = ulen;
    } else {
        uint32_t ulen;
        const char *uri = ns_lookup(P, "", 0, &ulen);
        if (uri != NULL && ulen > 0) { el->ns_uri = uri; el->ns_uri_len = ulen; }
        /* else: no default namespace -> the element is in no namespace */
    }

    /* Phase 4: create the attribute nodes (xmlns declarations kept, §7.2). */
    for (size_t i = 0; i < P->nratt; i++) {
        raw_attr_t *r = &P->ratt[i];
        const char *pfx, *loc; uint32_t pl, ll;
        if (split_qname(r->name, r->name_len, &pfx, &pl, &loc, &ll) != 0) { set_syntax(P); return -1; }

        mkr_xml_node_t *attr = mkr_xml_arena_node(P->doc, MKR_XN_ATTRIBUTE);
        if (attr == NULL) { propagate_oom(P); return -1; }
        attr->prefix = own(P, pfx, pl); attr->prefix_len = pl;
        attr->local  = own(P, loc, ll); attr->local_len = ll;
        if ((pl > 0 && attr->prefix == NULL) || (ll > 0 && attr->local == NULL)) return -1;

        int is_default  = slice_eq(r->name, r->name_len, "xmlns", 5);
        int is_pfx_decl = (r->name_len > 6 && memcmp(r->name, "xmlns:", 6) == 0);
        if (is_default || is_pfx_decl) {
            attr->ns_uri = XMLNS_NS_URI; attr->ns_uri_len = LIT_LEN(XMLNS_NS_URI);
        } else if (pl > 0) {                           /* prefixed attribute */
            uint32_t ulen;
            const char *uri = ns_lookup(P, pfx, pl, &ulen);
            if (uri == NULL) { set_syntax(P); return -1; }          /* unbound prefix */
            attr->ns_uri = uri; attr->ns_uri_len = ulen;
        }
        /* else: unprefixed attribute -> no namespace (the default ns never applies) */

        attr->value = mkr_xml_expand(P->doc, r->val, r->val_len,
                                     MKR_XML_EXPAND_ATTR, &attr->value_len, &P->status);
        if (attr->value == NULL) return -1;
        append_attr(el, attr);
    }
    return 0;
}

mkr_xml_doc_t *
mkr_xml_parse(const char *src, size_t len, mkr_xml_status_t *status)
{
    mkr_xml_doc_t *doc = mkr_xml_doc_new();
    if (doc == NULL) { if (status) *status = MKR_XML_ERR_OOM; return NULL; }

    mkr_xml_parser_t P = { src, src + len, 1, 1, doc, MKR_XML_OK };

    mkr_xml_node_t **stack = NULL;     /* open elements */
    size_t          *frame = NULL;     /* parallel: namespace-binding count when each was opened */
    size_t depth = 0, scap = 0, fcap = 0;

    while (P.p < P.end) {
        if (*P.p == '<') {
            uint32_t tl = P.line, tc = P.col;   /* tag start position */
            advance(&P); /* '<' */
            if (P.p >= P.end) { set_syntax(&P); break; }
            char c = *P.p;

            if (c == '/') {                      /* end tag */
                advance(&P);
                const char *nm; uint32_t nl;
                if (scan_name(&P, &nm, &nl) != 0) break;
                skip_ws(&P);
                if (P.p >= P.end || *P.p != '>') { set_syntax(&P); break; }
                advance(&P);
                if (depth == 0) { set_syntax(&P); break; }   /* end tag with no open element */
                /* the end tag must match the open element's full QName (raw) */
                mkr_xml_node_t *top = stack[depth - 1];
                uint32_t tql = top->prefix_len ? top->prefix_len + 1 + top->local_len : top->local_len;
                int match;
                if (top->prefix_len) {
                    match = (nl == tql && memcmp(nm, top->prefix, top->prefix_len) == 0
                             && nm[top->prefix_len] == ':'
                             && memcmp(nm + top->prefix_len + 1, top->local, top->local_len) == 0);
                } else {
                    match = (top->local_len == nl && memcmp(top->local, nm, nl) == 0);
                }
                if (!match) { set_syntax(&P); break; }        /* mismatched end tag */
                depth--;
                P.nbind = frame[depth];                       /* pop this element's namespace scope */
            } else if (c == '!' || c == '?') {   /* comment/CDATA/PI/DOCTYPE/XML-decl: §9 */
                set_syntax(&P); break;           /* unsupported in the minimal subset */
            } else {                              /* start tag */
                const char *nm; uint32_t nl;
                if (scan_name(&P, &nm, &nl) != 0) break;
                const char *epfx, *eloc; uint32_t epl, ell;
                if (split_qname(nm, nl, &epfx, &epl, &eloc, &ell) != 0) { set_syntax(&P); break; }
                mkr_xml_node_t *el = mkr_xml_arena_node(doc, MKR_XN_ELEMENT);
                if (el == NULL) { propagate_oom(&P); break; }
                el->prefix = own(&P, epfx, epl); el->prefix_len = epl;
                el->local  = own(&P, eloc, ell); el->local_len = ell;
                el->line = tl; el->col = tc;
                if (P.status != MKR_XML_OK) break;

                if (depth == 0) {
                    if (doc->root != NULL) { set_syntax(&P); break; } /* multiple roots */
                    doc->root = el;
                } else {
                    append_child(stack[depth - 1], el);
                }

                size_t bind_base = P.nbind;     /* xmlns on this element go above here */
                int pushed = 0;
                if (parse_element_body(&P, el, &pushed) != 0) break;
                if (pushed) {
                    if (depth + 1 > MKR_XML_MAX_DEPTH) { P.status = MKR_XML_ERR_LIMIT; break; }
                    if (mkr_grow_reserve((void **)&stack, &scap, depth + 1, sizeof(*stack)) != MKR_OK
                        || mkr_grow_reserve((void **)&frame, &fcap, depth + 1, sizeof(*frame)) != MKR_OK) {
                        P.status = MKR_XML_ERR_OOM; break;
                    }
                    stack[depth] = el; frame[depth] = bind_base; depth++;
                } else {
                    P.nbind = bind_base;        /* self-closing: pop its namespace scope now */
                }
            }
        } else {                                  /* character data */
            const char *tstart = P.p;
            int nonspace = 0;
            while (P.p < P.end && *P.p != '<') {
                char c = *P.p;
                if (!(c == ' ' || c == '\t' || c == '\n' || c == '\r')) nonspace = 1;
                advance(&P);
            }
            size_t traw = (size_t)(P.p - tstart);
            if (traw > UINT32_MAX) { P.status = MKR_XML_ERR_LIMIT; break; } /* fail closed */
            uint32_t tlen = (uint32_t)traw;
            if (depth == 0) {
                /* text outside any element: only whitespace (misc) is allowed */
                if (nonspace) { set_syntax(&P); break; }
            } else {
                mkr_xml_node_t *t = mkr_xml_arena_node(doc, MKR_XN_TEXT);
                if (t == NULL) { propagate_oom(&P); break; }
                /* expand char/entity references + validate XML Char (§9.1/§9.2) */
                t->value = mkr_xml_expand(doc, tstart, tlen,
                                          MKR_XML_EXPAND_TEXT, &t->value_len, &P.status);
                if (t->value == NULL) break;
                append_child(stack[depth - 1], t);
            }
        }
        if (P.status != MKR_XML_OK) break;
    }

    if (P.status == MKR_XML_OK) {
        if (depth != 0)        set_syntax(&P);   /* unclosed element(s) */
        else if (doc->root == NULL) set_syntax(&P); /* no root element */
    }

    free(stack);
    free(frame);
    free(P.binds);
    free(P.ratt);
    if (P.status != MKR_XML_OK) {
        if (status) *status = P.status;
        mkr_xml_doc_destroy(doc);
        return NULL;
    }
    if (status) *status = MKR_XML_OK;
    return doc;
}

/* ---- parse self-test (structural; run from Makiri.__c_selftest under ASan) --
 * Returns 0 on success or the 1-based index of the first failing check. Test
 * strings are compile-time literals, so lengths come from sizeof - 1 (no
 * strlen on a non-validated string -> clint clean). */
static int nlname(const mkr_xml_node_t *n, const char *s, size_t l)
{
    return n && n->local_len == l && memcmp(n->local, s, l) == 0;
}
static int nlval(const mkr_xml_node_t *n, const char *s, size_t l)
{
    return n && n->value_len == l && memcmp(n->value, s, l) == 0;
}
static int nlns(const mkr_xml_node_t *n, const char *s, size_t l)
{
    return n && n->ns_uri_len == l && (l == 0 || memcmp(n->ns_uri, s, l) == 0);
}
static int nlpfx(const mkr_xml_node_t *n, const char *s, size_t l)
{
    return n && n->prefix_len == l && (l == 0 || memcmp(n->prefix, s, l) == 0);
}
#define NAME_IS(n, lit) nlname((n), (lit), sizeof(lit) - 1)
#define VAL_IS(n, lit)  nlval((n),  (lit), sizeof(lit) - 1)
#define NS_IS(n, lit)   nlns((n),   (lit), sizeof(lit) - 1)
#define PFX_IS(n, lit)  nlpfx((n),  (lit), sizeof(lit) - 1)
#define NS_NONE(n)      ((n) && (n)->ns_uri_len == 0)
#define PARSE_LIT(lit, stp) mkr_xml_parse((lit), sizeof(lit) - 1, (stp))

int
mkr_xml_parse_selftest(void)
{
    mkr_xml_status_t st = MKR_XML_OK;
    int i = 0;

    /* happy path: structure, attributes, mixed content, case preservation */
    i++; /* 1 */
    mkr_xml_doc_t *d = PARSE_LIT("<Feed x='1' y='two'>hi<b/>z</Feed>", &st);
    if (d == NULL || st != MKR_XML_OK) { if (d) mkr_xml_doc_destroy(d); return i; }

    i++; /* 2: root element, case-preserved name, source position recorded */
    mkr_xml_node_t *root = d->root;
    if (!NAME_IS(root, "Feed") || root->type != MKR_XN_ELEMENT
        || root->line != 1 || root->col != 1) { mkr_xml_doc_destroy(d); return i; }

    i++; /* 3: attributes x=1, y=two in order */
    mkr_xml_node_t *a0 = root->attrs;
    mkr_xml_node_t *a1 = a0 ? a0->next : NULL;
    if (!a0 || a0->type != MKR_XN_ATTRIBUTE || !NAME_IS(a0, "x") || !VAL_IS(a0, "1")
        || !a1 || !NAME_IS(a1, "y") || !VAL_IS(a1, "two") || a1->next != NULL) {
        mkr_xml_doc_destroy(d); return i;
    }

    i++; /* 4: children = text "hi", element <b>, text "z" */
    mkr_xml_node_t *c0 = root->first_child;
    mkr_xml_node_t *c1 = c0 ? c0->next : NULL;
    mkr_xml_node_t *c2 = c1 ? c1->next : NULL;
    if (!c0 || c0->type != MKR_XN_TEXT || !VAL_IS(c0, "hi")
        || !c1 || c1->type != MKR_XN_ELEMENT || !NAME_IS(c1, "b") || c1->first_child != NULL
        || !c2 || c2->type != MKR_XN_TEXT || !VAL_IS(c2, "z") || c2->next != NULL) {
        mkr_xml_doc_destroy(d); return i;
    }
    i++; /* 5: parent/sibling links */
    if (c1->parent != root || c1->prev != c0 || c2->prev != c1) { mkr_xml_doc_destroy(d); return i; }
    mkr_xml_doc_destroy(d);

    /* case sensitivity: <X> and <x> are distinct */
    i++; /* 6 */
    d = PARSE_LIT("<X><x/></X>", &st);
    if (d == NULL || !NAME_IS(d->root, "X") || !NAME_IS(d->root->first_child, "x")) {
        if (d) mkr_xml_doc_destroy(d); return i;
    }
    mkr_xml_doc_destroy(d);

    /* well-formedness errors fail closed (NULL + SYNTAX, no leak) */
    i++; /* 7: one literal per case keeps sizeof-based lengths */
    st = MKR_XML_OK;
    mkr_xml_doc_t *e;
    e = PARSE_LIT("<a>",       &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; }
    st = MKR_XML_OK;
    e = PARSE_LIT("<a></b>",   &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; }
    st = MKR_XML_OK;
    e = PARSE_LIT("<a/><b/>",  &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; }
    st = MKR_XML_OK;
    e = PARSE_LIT("x<a/>",     &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; }
    st = MKR_XML_OK;
    e = PARSE_LIT("<a x=>",    &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; }
    st = MKR_XML_OK;
    e = PARSE_LIT("<a y='<'>", &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; }

    /* §9.1: predefined + numeric references expand; text and attribute values */
    i++; /* 8 */
    d = PARSE_LIT("<a x='p&amp;q' y='&#65;&#x42;'>1&lt;2&gt;3&amp;4&apos;5&quot;6</a>", &st);
    if (d == NULL || st != MKR_XML_OK) { if (d) mkr_xml_doc_destroy(d); return i; }
    {
        mkr_xml_node_t *r = d->root;
        mkr_xml_node_t *ax = r->attrs, *ay = ax ? ax->next : NULL;
        mkr_xml_node_t *tx = r->first_child;
        if (!VAL_IS(ax, "p&q") || !VAL_IS(ay, "AB")
            || !tx || tx->type != MKR_XN_TEXT || !VAL_IS(tx, "1<2>3&4'5\"6")) {
            mkr_xml_doc_destroy(d); return i;
        }
    }
    mkr_xml_doc_destroy(d);

    /* §9.1/§9.2: undefined entity, bad/empty reference, and non-XML-Char fail closed */
    i++; /* 9 */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a>&nbsp;</a>", &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; }
    st = MKR_XML_OK;
    e = PARSE_LIT("<a>x & y</a>",  &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* bare & */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a>&#0;</a>",   &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* NUL not XML Char */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a>&#xD800;</a>", &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* surrogate */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a>&#;</a>",    &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* no digits */

    /* §7: namespaces — prefixed element + attribute, default-ns inheritance,
     * unprefixed attribute has no namespace, xmlns kept as a DOM attribute */
    i++; /* 10 */
    d = PARSE_LIT("<a:e xmlns:a='urn:a' xmlns='urn:d' a:x='1' y='2'><c/></a:e>", &st);
    if (d == NULL || st != MKR_XML_OK) { if (d) mkr_xml_doc_destroy(d); return i; }
    {
        mkr_xml_node_t *r = d->root;
        if (!NAME_IS(r, "e") || !PFX_IS(r, "a") || !NS_IS(r, "urn:a")) { mkr_xml_doc_destroy(d); return i; }
        mkr_xml_node_t *c = r->first_child;
        if (!NAME_IS(c, "c") || !NS_IS(c, "urn:d")) { mkr_xml_doc_destroy(d); return i; } /* default ns inherited */
        /* attrs in order: xmlns:a, xmlns, a:x, y */
        mkr_xml_node_t *a = r->attrs;
        if (!a || !NAME_IS(a, "a") || !PFX_IS(a, "xmlns") || !NS_IS(a, XMLNS_NS_URI)) { mkr_xml_doc_destroy(d); return i; }
        a = a->next; /* xmlns */
        if (!a || !NAME_IS(a, "xmlns") || a->prefix_len != 0) { mkr_xml_doc_destroy(d); return i; }
        a = a->next; /* a:x */
        if (!a || !NAME_IS(a, "x") || !PFX_IS(a, "a") || !NS_IS(a, "urn:a") || !VAL_IS(a, "1")) { mkr_xml_doc_destroy(d); return i; }
        a = a->next; /* y -> NO namespace (default ns never applies to attributes) */
        if (!a || !NAME_IS(a, "y") || a->prefix_len != 0 || !NS_NONE(a) || !VAL_IS(a, "2")) { mkr_xml_doc_destroy(d); return i; }
    }
    mkr_xml_doc_destroy(d);

    /* §7.1: namespace errors fail closed */
    i++; /* 11 */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a:b/>",                 &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* unbound element prefix */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a x:y='1'/>",           &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* unbound attr prefix */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a xmlns:xml='wrong'/>", &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* rebinding xml */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a:b xmlns:a=''/>",      &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* xmlns:p="" */

    /* §3.3.3: attribute-value normalization — a LITERAL tab/newline/CR in the
     * source folds to a space, while a tab/LF from a numeric reference is kept.
     * Text content is NOT folded (the literal newline survives verbatim). */
    i++; /* 12 */
    d = PARSE_LIT("<a x=\"p\tq\nr\" y=\"p&#9;q&#10;r\">u\tv\nw</a>", &st);
    if (d == NULL || st != MKR_XML_OK) { if (d) mkr_xml_doc_destroy(d); return i; }
    {
        mkr_xml_node_t *r = d->root;
        mkr_xml_node_t *ax = r->attrs, *ay = ax ? ax->next : NULL;
        mkr_xml_node_t *tx = r->first_child;
        if (!VAL_IS(ax, "p q r")            /* literal whitespace -> spaces */
            || !VAL_IS(ay, "p\tq\nr")       /* reference-derived whitespace preserved */
            || !tx || tx->type != MKR_XN_TEXT || !VAL_IS(tx, "u\tv\nw")) { /* text unfolded */
            mkr_xml_doc_destroy(d); return i;
        }
    }
    mkr_xml_doc_destroy(d);

    return 0;
}
