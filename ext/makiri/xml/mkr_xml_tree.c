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

typedef struct {
    const char     *p, *end;
    uint32_t        line, col;       /* 1-based; col in bytes (§5) */
    mkr_xml_doc_t  *doc;
    mkr_xml_status_t status;
} mkr_xml_parser_t;

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
    *out = start;
    *len = (uint32_t)(P->p - start);
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

/* Parse the attribute list and the tag close. On '>' pushes the element (caller
 * handles depth); on '/>' leaves it as a leaf. Returns 0 / -1, *pushed set. */
static int
parse_attrs_and_close(mkr_xml_parser_t *P, mkr_xml_node_t *el, int *pushed)
{
    *pushed = 0;
    for (;;) {
        skip_ws(P);
        if (P->p >= P->end) { set_syntax(P); return -1; }       /* unterminated tag */
        char c = *P->p;
        if (c == '>') { advance(P); *pushed = 1; return 0; }
        if (c == '/') {
            advance(P);
            if (P->p >= P->end || *P->p != '>') { set_syntax(P); return -1; }
            advance(P); /* '>' of '/>' */
            return 0;   /* self-closing: not pushed */
        }
        /* attribute: Name S? '=' S? quote value quote */
        const char *an; uint32_t alen;
        if (scan_name(P, &an, &alen) != 0) return -1;
        skip_ws(P);
        if (P->p >= P->end || *P->p != '=') { set_syntax(P); return -1; }
        advance(P);
        skip_ws(P);
        if (P->p >= P->end || (*P->p != '"' && *P->p != '\'')) { set_syntax(P); return -1; }
        char quote = *P->p; advance(P);
        const char *vstart = P->p;
        while (P->p < P->end && *P->p != quote) {
            if (*P->p == '<') { set_syntax(P); return -1; }     /* raw '<' in AttValue */
            advance(P);
        }
        if (P->p >= P->end) { set_syntax(P); return -1; }       /* unterminated value */
        uint32_t vlen = (uint32_t)(P->p - vstart);
        advance(P); /* closing quote */

        mkr_xml_node_t *attr = mkr_xml_arena_node(P->doc, MKR_XN_ATTRIBUTE);
        if (attr == NULL) { propagate_oom(P); return -1; }
        attr->local = own(P, an, alen); attr->local_len = alen;
        attr->value = own(P, vstart, vlen); attr->value_len = vlen;
        if (P->status != MKR_XML_OK) return -1;
        append_attr(el, attr);
    }
}

mkr_xml_doc_t *
mkr_xml_parse(const char *src, size_t len, mkr_xml_status_t *status)
{
    mkr_xml_doc_t *doc = mkr_xml_doc_new();
    if (doc == NULL) { if (status) *status = MKR_XML_ERR_OOM; return NULL; }

    mkr_xml_parser_t P = { src, src + len, 1, 1, doc, MKR_XML_OK };

    mkr_xml_node_t **stack = NULL;
    size_t depth = 0, scap = 0;

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
                mkr_xml_node_t *top = stack[depth - 1];
                if (top->local_len != nl || memcmp(top->local, nm, nl) != 0) {
                    set_syntax(&P); break;                   /* mismatched end tag */
                }
                depth--;
            } else if (c == '!' || c == '?') {   /* comment/CDATA/PI/DOCTYPE/XML-decl: §9 */
                set_syntax(&P); break;           /* unsupported in the minimal subset */
            } else {                              /* start tag */
                const char *nm; uint32_t nl;
                if (scan_name(&P, &nm, &nl) != 0) break;
                mkr_xml_node_t *el = mkr_xml_arena_node(doc, MKR_XN_ELEMENT);
                if (el == NULL) { propagate_oom(&P); break; }
                el->local = own(&P, nm, nl); el->local_len = nl;
                el->line = tl; el->col = tc;
                if (P.status != MKR_XML_OK) break;

                if (depth == 0) {
                    if (doc->root != NULL) { set_syntax(&P); break; } /* multiple roots */
                    doc->root = el;
                } else {
                    append_child(stack[depth - 1], el);
                }

                int pushed = 0;
                if (parse_attrs_and_close(&P, el, &pushed) != 0) break;
                if (pushed) {
                    if (depth + 1 > MKR_XML_MAX_DEPTH) { P.status = MKR_XML_ERR_LIMIT; break; }
                    if (mkr_grow_reserve((void **)&stack, &scap, depth + 1, sizeof(*stack)) != MKR_OK) {
                        P.status = MKR_XML_ERR_OOM; break;
                    }
                    stack[depth++] = el;
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
            uint32_t tlen = (uint32_t)(P.p - tstart);
            if (depth == 0) {
                /* text outside any element: only whitespace (misc) is allowed */
                if (nonspace) { set_syntax(&P); break; }
            } else {
                mkr_xml_node_t *t = mkr_xml_arena_node(doc, MKR_XN_TEXT);
                if (t == NULL) { propagate_oom(&P); break; }
                t->value = own(&P, tstart, tlen); t->value_len = tlen;
                if (P.status != MKR_XML_OK) break;
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
#define NAME_IS(n, lit) nlname((n), (lit), sizeof(lit) - 1)
#define VAL_IS(n, lit)  nlval((n),  (lit), sizeof(lit) - 1)
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

    return 0;
}
