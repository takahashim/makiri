/* mkr_xml_tree.c - XML tokenizer + tree builder (§14 steps 5-9).
 *
 * Ruby-free. Parses a decoded, validated-UTF-8 byte buffer (§2.1) into the
 * custom node arena: elements, attributes, character data, namespaces (§7),
 * entity/char-reference expansion (§9.1), comments / CDATA sections / processing
 * instructions / the XML declaration (§9). A '<!DOCTYPE' is RECOGNIZED but its
 * DTD is not processed (§9.4 alternative): the name + external id are kept (in an
 * off-tree DOCUMENT_TYPE node, for Document#internal_subset) and the rest is
 * skipped to its true '>'. NOTHING in the DTD is registered - no entity is
 * defined (a DTD-defined &name; stays an undefined-entity error) and no external
 * subset is fetched (zero I/O), so XXE / billion-laughs remain structurally
 * impossible. Comments and PIs in the prolog/epilog (outside the root) are
 * retained as children of the DOCUMENT node (the XPath data model; like
 * Nokogiri); inter-construct whitespace there is not a text node. Strict
 * NameStartChar/NameChar (§9.2b) and duplicate-attribute rejection (§9.3) apply.
 *
 * Budgets (§4): element depth (MKR_XML_MAX_DEPTH) and node/attr counts (enforced
 * in the arena) are fail-closed - a violation aborts with no partial document.
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
    mkr_span_t      in;              /* bounded input reader (cursor + end; every
                                      * read is bounds-checked by construction) */
    const char     *start;           /* buffer origin (the only place the XML
                                      * declaration is valid; compared, never read) */
    uint32_t        line, col;       /* 1-based; col in bytes (§5) */
    mkr_xml_doc_t  *doc;
    mkr_xml_node_t *fragment;        /* NULL = parse a whole document; else parse a
                                      * fragment INTO this DOCUMENT_FRAGMENT node:
                                      * 0+ top-level nodes (char data, multiple
                                      * elements; no XML decl / DOCTYPE / single-root
                                      * rule) attach to it instead of the doc node */
    mkr_xml_status_t status;
    ns_binding_t   *binds; size_t nbind, bind_cap;   /* namespace scope stack */
    raw_attr_t     *ratt;  size_t nratt, ratt_cap;   /* reusable per-tag attr buffer */
    mkr_xml_node_t **stack; size_t depth, scap;      /* open-element stack */
    size_t          *frame; size_t fcap;             /* parallel: ns-binding count when
                                                      * each open element was entered */
    int             saw_doctype;                     /* at most one DOCTYPE, prolog only */
} mkr_xml_parser_t;

/* small leaf predicates defined later but used by earlier handlers */
static int is_space_byte(int c);
static int lit_ahead(const mkr_xml_parser_t *P, const char *lit, size_t n);

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
    int c = mkr_span_take(&P->in);
    if (c < 0) return;
    if (c == '\n') { P->line++; P->col = 1; }
    else           { P->col++; }
}

/* Advance n bytes, keeping line/col correct (the consumed span may contain LF:
 * comment/CDATA/PI content). Input is line-ending-normalized, so only LF appears. */
static void
advance_n(mkr_xml_parser_t *P, size_t n)
{
    while (n-- > 0 && mkr_span_left(&P->in) > 0) advance(P);
}

static void
skip_ws(mkr_xml_parser_t *P)
{
    for (;;) {
        int c = mkr_span_peek(&P->in);
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r') advance(P);
        else break;
    }
}

/* Scan a Name into [*out, *out+*len) (a slice of the input), codepoint by
 * codepoint against the full XML 1.0 §2.3 NameStartChar / NameChar sets (§9.2b):
 * the first codepoint must be a NameStartChar, the rest NameChar. 0 on success. */
static int
scan_name(mkr_xml_parser_t *P, const char **out, uint32_t *len)
{
    const char *start = mkr_span_mark(&P->in);
    uint32_t cp;
    int bl = mkr_utf8_decode1_span(&P->in, &cp);
    if (bl == 0 || !mkr_xml_is_name_start(cp)) { set_syntax(P); return -1; }
    advance_n(P, (size_t)bl);
    for (;;) {
        bl = mkr_utf8_decode1_span(&P->in, &cp);
        if (bl == 0 || !mkr_xml_is_name_char(cp)) break;
        advance_n(P, (size_t)bl);
    }
    size_t n = mkr_span_since(&P->in, start);
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

/* Append a character-data node (TEXT or CDATA) whose value is the arena slice
 * [val, val+len). Adjacent character data of the SAME type is coalesced into the
 * preceding sibling (as libxml2's xmlAddChild does, and per the XPath data
 * model's "as much character data as possible is grouped" rule) - so
 * <![CDATA[a]]><![CDATA[b]]> is one CDATA node, not two. Different types stay
 * separate (text vs CDATA remain distinct DOM nodes). 0 / -1 (P->status set). */
static int
append_chardata(mkr_xml_parser_t *P, mkr_xml_node_t *parent, uint8_t type,
                const char *val, uint32_t len)
{
    mkr_xml_node_t *last = parent->last_child;
    if (last != NULL && last->type == type) {
        size_t total = (size_t)last->value_len + len;
        if (total > UINT32_MAX) { P->status = MKR_XML_ERR_LIMIT; return -1; }
        /* concatenate via the bounded writer: total == the two parts exactly, so
         * it never overflows; the writer just removes the raw-pointer arithmetic. */
        mkr_spanbuf_t b = mkr_xml_arena_spanbuf(P->doc, total);
        mkr_spanbuf_write(&b, last->value, last->value_len);
        mkr_spanbuf_write(&b, val, len);
        const char *buf = mkr_spanbuf_finish(&b);
        if (buf == NULL) { propagate_oom(P); return -1; }   /* alloc failure */
        last->value = buf; last->value_len = (uint32_t)b.pos;
        return 0;
    }
    mkr_xml_node_t *n = mkr_xml_arena_node(P->doc, type);
    if (n == NULL) { propagate_oom(P); return -1; }
    n->value = val; n->value_len = len;
    append_child(parent, n);
    return 0;
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

/* Store an element/attribute's raw "prefix:local" QName as ONE arena copy, with
 * local and prefix slicing into it. Keeping the qualified name contiguous lets
 * Node#name / the XPath name() borrow it without rebuilding it. +loc+/+pl+ come
 * from split_qname (loc points into +name+). 0 on success, -1 on OOM. Thin
 * wrapper over the shared mkr_xml_qname_assign (one arena-copy rule with the
 * mutators), propagating an OOM to P->status. */
static int
set_node_qname(mkr_xml_parser_t *P, mkr_xml_node_t *node, const char *name,
               uint32_t nl, const char *loc, uint32_t ll, uint32_t pl)
{
    mkr_xml_qname_t qn = { name, nl, name, pl, loc, ll };
    if (mkr_xml_qname_assign(P->doc, node, &qn) != 0) { propagate_oom(P); return -1; }
    return 0;
}

static int
slice_eq(const char *a, uint32_t al, const char *b, uint32_t bl)
{
    return mkr_bytes_eq(a, al, b, bl);
}

/* Split a QName into prefix:local, unpacked into loose out-params for the tree
 * builder's callers. 0 on success (no-colon names get prefix_len 0); -1 if
 * malformed (>1 colon, or an empty prefix/local). Thin adapter over the shared
 * mkr_xml_split_scanned_qname (§9.2b NCName rules): the name has already been
 * scan_name'd here, so no re-scan is needed - one splitting rule with the
 * mutation path. */
static int
split_qname(const char *name, uint32_t len, const char **pfx, uint32_t *pfx_len,
            const char **loc, uint32_t *loc_len)
{
    mkr_xml_qname_t qn;
    if (mkr_xml_split_scanned_qname(name, len, &qn) != 0) return -1;
    *pfx = qn.prefix; *pfx_len = qn.prefix_len;
    *loc = qn.local;  *loc_len = qn.local_len;
    return 0;
}

/* Resolve a prefix to its in-scope namespace URI, or NULL when unbound. The
 * predefined 'xml' prefix always resolves; for the empty prefix this returns the
 * default-namespace URI (possibly "" with *uri_len 0 = undeclared) or NULL. */
static const char *
ns_lookup(mkr_xml_parser_t *P, const char *pfx, uint32_t pfx_len, uint32_t *uri_len)
{
    if (slice_eq(pfx, pfx_len, "xml", 3)) { *uri_len = MKR_LIT_LEN(MKR_XML_NS_URI); return MKR_XML_NS_URI; }
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

/* Phase 1: scan all attributes + the tag close into P->ratt (the raw, pre-
 * namespace-resolution buffer). Sets *pushed=1 on '>' (an open tag), 0 on '/>'
 * (self-closing). Returns 0 / -1. */
static int
scan_raw_attrs(mkr_xml_parser_t *P, int *pushed)
{
    *pushed = 0;
    P->nratt = 0;
    for (;;) {
        skip_ws(P);
        int c = mkr_span_peek(&P->in);
        if (c < 0) { set_syntax(P); return -1; }                /* unterminated tag */
        if (c == '>') { advance(P); *pushed = 1; break; }
        if (c == '/') {
            advance(P);
            if (mkr_span_peek(&P->in) != '>') { set_syntax(P); return -1; }
            advance(P); break;                                  /* self-closing */
        }
        const char *an; uint32_t alen;
        if (scan_name(P, &an, &alen) != 0) return -1;
        skip_ws(P);
        if (mkr_span_peek(&P->in) != '=') { set_syntax(P); return -1; }
        advance(P); skip_ws(P);
        int q = mkr_span_peek(&P->in);
        if (q != '"' && q != '\'') { set_syntax(P); return -1; }
        advance(P);
        const char *vs = mkr_span_mark(&P->in);
        for (;;) {
            int vc = mkr_span_peek(&P->in);
            if (vc < 0 || vc == q) break;
            if (vc == '<') { set_syntax(P); return -1; }        /* raw '<' in AttValue */
            advance(P);
        }
        if (mkr_span_left(&P->in) == 0) { set_syntax(P); return -1; } /* unterminated value */
        size_t vraw = mkr_span_since(&P->in, vs);
        if (vraw > UINT32_MAX) { P->status = MKR_XML_ERR_LIMIT; return -1; } /* fail closed */
        uint32_t vlen = (uint32_t)vraw;
        advance(P);                                              /* closing quote */
        /* §3.1: attributes are S-separated - after a value the next byte must be
         * whitespace or the tag close ('>' / '/'), never another name. Rejects
         * `<a x="1"y="2"/>`. */
        int nx = mkr_span_peek(&P->in);
        if (nx >= 0 && nx != '>' && nx != '/' && !is_space_byte(nx)) {
            set_syntax(P); return -1;
        }
        if (P->nratt + 1 > MKR_XML_MAX_ATTRS) { P->status = MKR_XML_ERR_LIMIT; return -1; }
        if (mkr_grow_reserve((void **)&P->ratt, &P->ratt_cap, P->nratt + 1, sizeof(*P->ratt)) != MKR_OK) {
            P->status = MKR_XML_ERR_OOM; return -1;
        }
        P->ratt[P->nratt].name = an; P->ratt[P->nratt].name_len = alen;
        P->ratt[P->nratt].val  = vs; P->ratt[P->nratt].val_len  = vlen;
        P->nratt++;
    }
    return 0;
}

/* Phase 2: process the start tag's xmlns declarations into the scope bindings
 * (§7.1 reserved-prefix / reserved-URI rules). Returns 0 / -1. */
static int
apply_xmlns_bindings(mkr_xml_parser_t *P)
{
    for (size_t i = 0; i < P->nratt; i++) {
        raw_attr_t *r = &P->ratt[i];
        const char *bpfx; uint32_t bpl;
        if (!mkr_xml_xmlns_prefix(r->name, r->name_len, &bpfx, &bpl)) continue;
        if (slice_eq(bpfx, bpl, "xmlns", 5)) { set_syntax(P); return -1; }   /* xmlns:xmlns reserved */

        uint32_t ulen;
        const char *uri = mkr_xml_expand(P->doc, r->val, r->val_len,
                                         MKR_XML_EXPAND_ATTR, &ulen, &P->status);
        if (uri == NULL) return -1;
        if (slice_eq(bpfx, bpl, "xml", 3)) {
            if (!slice_eq(uri, ulen, MKR_XML_NS_URI, MKR_LIT_LEN(MKR_XML_NS_URI))) { set_syntax(P); return -1; }
        } else if (slice_eq(uri, ulen, MKR_XML_NS_URI, MKR_LIT_LEN(MKR_XML_NS_URI))
                || slice_eq(uri, ulen, MKR_XMLNS_NS_URI, MKR_LIT_LEN(MKR_XMLNS_NS_URI))) {
            set_syntax(P); return -1;                  /* reserved URI bound to another prefix */
        }
        if (bpl > 0 && ulen == 0) { set_syntax(P); return -1; } /* xmlns:p="" (XML 1.0) */
        if (push_binding(P, bpfx, bpl, uri, ulen) != 0) return -1;
    }
    return 0;
}

/* Phase 3: resolve the element's own namespace URI from the in-scope bindings.
 * Returns 0 / -1 (unbound prefix). */
static int
resolve_element_ns(mkr_xml_parser_t *P, mkr_xml_node_t *el)
{
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
    return 0;
}

/* Phase 4: create the attribute DOM nodes (xmlns declarations kept, §7.2), then
 * reject duplicates (§9.3). The list is built with an explicit tail so each
 * append is O(1) - walking to the tail per attribute would be O(n^2) over a
 * MKR_XML_MAX_ATTRS-bounded set. Returns 0 / -1. */
static int
build_attr_nodes(mkr_xml_parser_t *P, mkr_xml_node_t *el)
{
    mkr_xml_node_t *attr_tail = NULL;
    for (size_t i = 0; i < P->nratt; i++) {
        raw_attr_t *r = &P->ratt[i];
        const char *pfx, *loc; uint32_t pl, ll;
        if (split_qname(r->name, r->name_len, &pfx, &pl, &loc, &ll) != 0) { set_syntax(P); return -1; }

        mkr_xml_node_t *attr = mkr_xml_arena_node(P->doc, MKR_XML_NODE_TYPE_ATTRIBUTE);
        if (attr == NULL) { propagate_oom(P); return -1; }
        (void)pfx;
        if (set_node_qname(P, attr, r->name, r->name_len, loc, ll, pl) != 0) return -1;

        if (mkr_xml_xmlns_prefix(r->name, r->name_len, NULL, NULL)) {
            attr->ns_uri = MKR_XMLNS_NS_URI; attr->ns_uri_len = MKR_LIT_LEN(MKR_XMLNS_NS_URI);
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
        attr->parent = el;                             /* O(1) tail append */
        if (attr_tail) attr_tail->next = attr; else el->attrs = attr;
        attr_tail = attr;
    }

    /* §9.3: no two attributes may share the same (namespace URI, local name) -
     * compared AFTER resolution, so "a:x" and "b:x" bound to the same URI collide
     * while "xmlns:a"/"xmlns:b" (same xmlns URI, different local) do not. O(n^2)
     * over a per-element set bounded by MKR_XML_MAX_ATTRS. */
    for (mkr_xml_node_t *a = el->attrs; a != NULL; a = a->next) {
        for (mkr_xml_node_t *b = a->next; b != NULL; b = b->next) {
            if (slice_eq(a->local, a->local_len, b->local, b->local_len)
                && slice_eq(a->ns_uri, a->ns_uri_len, b->ns_uri, b->ns_uri_len)) {
                set_syntax(P); return -1;                  /* duplicate attribute */
            }
        }
    }
    return 0;
}

/* Parse a start tag's attributes and close, resolving namespaces (§7), as four
 * ordered phases: scan the raw attributes, process xmlns declarations into scope
 * bindings, resolve the element's own namespace, then create the attribute nodes
 * (xmlns declarations kept as DOM attributes, §7.2). On '>' sets *pushed; on '/>'
 * leaves a leaf. The element's prefix/local are already set by the caller;
 * bindings pushed here are popped by the caller when the element closes. The
 * phase order matters: xmlns must be in scope before the element's and the
 * attributes' prefixes resolve. Returns 0 / -1. */
static int
parse_element_body(mkr_xml_parser_t *P, mkr_xml_node_t *el, int *pushed)
{
    if (scan_raw_attrs(P, pushed) != 0)     return -1;
    if (apply_xmlns_bindings(P) != 0)       return -1;
    if (resolve_element_ns(P, el) != 0)     return -1;
    return build_attr_nodes(P, el);
}

/* True if the remaining input begins with the literal [lit, lit+n). */
static int
lit_ahead(const mkr_xml_parser_t *P, const char *lit, size_t n)
{
    return mkr_span_starts(&P->in, lit, n);
}

/* Takes the int-or-minus-one a span peek returns (-1 / end-of-input is never a
 * space, so a "peek is space" test needs no separate end check). */
static int
is_space_byte(int c)
{
    return c == ' ' || c == '\t' || c == '\n' || c == '\r';
}

/* '<!--' comment (P->p at '!'). XML 1.0: the content is XML Char and may not
 * contain '--' except as part of the closing '-->'. Creates a COMMENT node under
 * +parent+ - its open element, or the DOCUMENT node in the prolog/epilog (XPath
 * data model; like Nokogiri). */
static int
parse_comment(mkr_xml_parser_t *P, mkr_xml_node_t *parent)
{
    advance_n(P, 3);                                  /* consume "!--" */
    const char *cstart = mkr_span_mark(&P->in);
    mkr_span_t s = P->in;                             /* scan copy: looks ahead, consumes nothing */
    for (;;) {
        size_t at;
        if (!mkr_span_find(&s, '-', &at)) { set_syntax(P); return -1; } /* unterminated */
        mkr_span_skip(&s, at);                        /* cursor on the '-' */
        if (mkr_span_at(&s, 1) == '-') {
            if (mkr_span_at(&s, 2) == '>') break;     /* cursor on the closing "-->" */
            set_syntax(P); return -1;                 /* '--' not part of '-->' */
        }
        mkr_span_skip(&s, 1);
    }
    size_t craw = mkr_span_since(&s, cstart);
    if (craw > UINT32_MAX) { P->status = MKR_XML_ERR_LIMIT; return -1; }
    uint32_t clen = (uint32_t)craw;
    if (mkr_xml_validate_chars(cstart, clen) != 0) { set_syntax(P); return -1; }
    if (parent != NULL) {
        mkr_xml_node_t *c = mkr_xml_arena_node(P->doc, MKR_XML_NODE_TYPE_COMMENT);
        if (c == NULL) { propagate_oom(P); return -1; }
        c->value = own(P, cstart, clen); c->value_len = clen;
        if (clen > 0 && c->value == NULL) return -1;
        append_child(parent, c);
    }
    advance_n(P, craw + 3);                           /* content + "-->" */
    return 0;
}

/* '<![CDATA[' section (P->p at '!'). Raw character data (no reference recognition)
 * up to ']]>'. Character data is only well-formed inside an element. */
static int
parse_cdata(mkr_xml_parser_t *P, mkr_xml_node_t *parent)
{
    if (P->depth == 0 && P->fragment == NULL) { set_syntax(P); return -1; } /* CDATA outside the root */
    advance_n(P, 8);                                  /* consume "![CDATA[" */
    const char *cstart = mkr_span_mark(&P->in);
    mkr_span_t s = P->in;                             /* scan copy */
    for (;;) {
        size_t at;
        if (!mkr_span_find(&s, ']', &at)) { set_syntax(P); return -1; } /* unterminated */
        mkr_span_skip(&s, at);                        /* cursor on the ']' */
        if (mkr_span_at(&s, 1) == ']' && mkr_span_at(&s, 2) == '>') break; /* cursor on "]]>" */
        mkr_span_skip(&s, 1);
    }
    size_t craw = mkr_span_since(&s, cstart);
    if (craw > UINT32_MAX) { P->status = MKR_XML_ERR_LIMIT; return -1; }
    uint32_t clen = (uint32_t)craw;
    if (mkr_xml_validate_chars(cstart, clen) != 0) { set_syntax(P); return -1; }
    const char *cval = own(P, cstart, clen);
    if (clen > 0 && cval == NULL) return -1;
    if (append_chardata(P, parent, MKR_XML_NODE_TYPE_CDATA_SECTION, cval, clen) != 0) return -1;
    advance_n(P, craw + 3);                           /* content + "]]>" */
    return 0;
}

/* ---- XML declaration (§2.8 / §4.3.1) strict pseudo-attribute grammar ---- */

/* match the literal keyword [kw, kw+n) at the cursor and consume it; 0/1. */
static int
eat_keyword(mkr_xml_parser_t *P, const char *kw, size_t n)
{
    if (!mkr_span_starts(&P->in, kw, n)) return 0;
    advance_n(P, n);
    return 1;
}

/* Eq ::= S? '=' S?  - 0 on success, -1 if '=' is missing. */
static int
decl_eq(mkr_xml_parser_t *P)
{
    skip_ws(P);
    if (mkr_span_peek(&P->in) != '=') { set_syntax(P); return -1; }
    advance(P);
    skip_ws(P);
    return 0;
}

/* Read a quoted pseudo-attribute value into [*out, *out+*out_len). 0 on success,
 * -1 on a missing/mismatched quote. */
static int
decl_value_get(mkr_xml_parser_t *P, const char **out, uint32_t *out_len)
{
    int qch = mkr_span_peek(&P->in);
    if (qch != '"' && qch != '\'') { set_syntax(P); return -1; }
    advance(P);
    const char *vs = mkr_span_mark(&P->in);
    while (mkr_span_peek(&P->in) >= 0 && mkr_span_peek(&P->in) != qch) advance(P);
    if (mkr_span_left(&P->in) == 0) { set_syntax(P); return -1; } /* unterminated / mismatched quote */
    size_t n = mkr_span_since(&P->in, vs);
    if (n > UINT32_MAX) { P->status = MKR_XML_ERR_LIMIT; return -1; } /* fail closed */
    *out = vs; *out_len = (uint32_t)n;
    advance(P);                                         /* closing quote */
    return 0;
}

/* A quoted pseudo-attribute value, validated whole by +ok+. 0 on success. */
static int
decl_value(mkr_xml_parser_t *P, int (*ok)(const char *, uint32_t))
{
    const char *vs; uint32_t vl;
    if (decl_value_get(P, &vs, &vl) != 0) return -1;
    if (!ok(vs, vl)) { set_syntax(P); return -1; }
    return 0;
}

static int is_version_num(const char *s, uint32_t n) {   /* '1.' [0-9]+ */
    mkr_span_t v = mkr_span(s, n);
    if (n < 3 || !mkr_span_starts(&v, "1.", 2)) return 0;
    for (uint32_t i = 2; i < n; i++) {
        int c = mkr_span_at(&v, i);
        if (c < '0' || c > '9') return 0;
    }
    return 1;
}
static int is_enc_name(const char *s, uint32_t n) {      /* [A-Za-z] ([A-Za-z0-9._] | '-')* */
    mkr_span_t v = mkr_span(s, n);
    int c0 = mkr_span_peek(&v);
    if (!((c0 >= 'A' && c0 <= 'Z') || (c0 >= 'a' && c0 <= 'z'))) return 0;
    for (uint32_t i = 1; i < n; i++) {
        int c = mkr_span_at(&v, i);
        if (!((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')
              || c == '.' || c == '_' || c == '-')) return 0;
    }
    return 1;
}
static int is_yes_no(const char *s, uint32_t n) {
    return mkr_bytes_eq(s, n, "yes", 3) || mkr_bytes_eq(s, n, "no", 2);
}

/* '<?xml' already consumed (cursor just past "xml"). Validate
 *   S 'version' Eq VersionNum (S 'encoding' Eq EncName)? (S 'standalone' Eq ('yes'|'no'))? S? '?>'
 * - pseudo-attributes are lowercase, in this order, each at most once, S-separated.
 * The declaration is not retained as a node. 0 on success / -1 (P->status set). */
static int
parse_xml_decl_body(mkr_xml_parser_t *P)
{
    if (!is_space_byte(mkr_span_peek(&P->in))) { set_syntax(P); return -1; } /* S */
    skip_ws(P);
    if (!eat_keyword(P, "version", 7)) { set_syntax(P); return -1; }
    if (decl_eq(P) != 0) return -1;
    const char *ver; uint32_t ver_len;
    if (decl_value_get(P, &ver, &ver_len) != 0) return -1;
    if (!is_version_num(ver, ver_len)) { set_syntax(P); return -1; }   /* bad VersionNum syntax */
    if (!mkr_bytes_eq(ver, ver_len, "1.0", 3)) {
        /* well-formed, but a version Makiri does not implement (XML 1.1 / 1.x):
         * fail closed rather than silently parse it under XML 1.0 rules. */
        P->status = MKR_XML_ERR_VERSION; return -1;
    }

    int saw_enc = 0, saw_sd = 0;
    for (;;) {
        int had_s = 0;
        while (is_space_byte(mkr_span_peek(&P->in))) { advance(P); had_s = 1; }
        if (lit_ahead(P, "?>", 2)) { advance_n(P, 2); return 0; }  /* end of declaration */
        if (!had_s) { set_syntax(P); return -1; }                 /* attrs need an S separator */
        if (!saw_enc && !saw_sd && eat_keyword(P, "encoding", 8)) {
            saw_enc = 1;                                          /* encoding precedes standalone */
            P->doc->has_encoding_decl = 1;  /* serializer emits encoding="UTF-8" only then */
            if (decl_eq(P) != 0 || decl_value(P, is_enc_name) != 0) return -1;
        } else if (!saw_sd && eat_keyword(P, "standalone", 10)) {
            saw_sd = 1;
            if (decl_eq(P) != 0 || decl_value(P, is_yes_no) != 0) return -1;
        } else {
            set_syntax(P); return -1;   /* unknown / duplicate / out-of-order pseudo-attribute */
        }
    }
}

/* '<?' processing instruction (P->p at '?'). The '<?xml ...?>' declaration (exact
 * lowercase target, document start only) is validated strictly by
 * parse_xml_decl_body and not retained; a target matching "xml" in any OTHER case
 * is a reserved PITarget (§2.6) and rejected. Every other PI is retained as a
 * node - under its open element, or under the DOCUMENT node in the
 * prolog/epilog (XPath data model; like Nokogiri). */
static int
parse_pi(mkr_xml_parser_t *P, mkr_xml_node_t *parent, int at_doc_start)
{
    advance_n(P, 1);                                  /* consume '?' */
    const char *tgt; uint32_t tl;
    if (scan_name(P, &tgt, &tl) != 0) return -1;      /* PITarget (a Name) */
    /* §2.6: PITarget is a Name, NOT an NCName - a colon is permitted (a PI target
     * is not subject to namespace processing). scan_name already validated it as a
     * Name (NameStartChar NameChar*); only the reserved "xml" (any case) below is
     * excluded. */
    mkr_span_t tsp = mkr_span(tgt, tl);
    int ci_xml  = mkr_xml_is_reserved_pi_target(tgt, tl);   /* "xml" in any case */
    int is_decl = (tl == 3 && mkr_span_starts(&tsp, "xml", 3));
    if (is_decl) {
        if (!at_doc_start || P->depth != 0) { set_syntax(P); return -1; }  /* decl: doc start only */
        return parse_xml_decl_body(P);
    }
    if (ci_xml) { set_syntax(P); return -1; }         /* reserved target ("XML"/"xmL"/...) */

    int empty_data = lit_ahead(P, "?>", 2);
    if (!empty_data) {
        if (!is_space_byte(mkr_span_peek(&P->in))) { set_syntax(P); return -1; } /* need S */
        skip_ws(P);
    }
    const char *dstart = mkr_span_mark(&P->in);
    mkr_span_t s = P->in;                             /* scan copy */
    for (;;) {
        size_t at;
        if (!mkr_span_find(&s, '?', &at)) { set_syntax(P); return -1; } /* unterminated */
        mkr_span_skip(&s, at);                        /* cursor on the '?' */
        if (mkr_span_at(&s, 1) == '>') break;         /* cursor on the closing "?>" */
        mkr_span_skip(&s, 1);
    }
    size_t draw = mkr_span_since(&s, dstart);
    if (draw > UINT32_MAX) { P->status = MKR_XML_ERR_LIMIT; return -1; }
    uint32_t dlen = (uint32_t)draw;
    if (mkr_xml_validate_chars(dstart, dlen) != 0) { set_syntax(P); return -1; }

    if (parent != NULL) {
        mkr_xml_node_t *pi = mkr_xml_arena_node(P->doc, MKR_XML_NODE_TYPE_PI);
        if (pi == NULL) { propagate_oom(P); return -1; }
        pi->local = own(P, tgt, tl);     pi->local_len = tl;
        pi->value = own(P, dstart, dlen); pi->value_len = dlen;
        if (pi->local == NULL || (dlen > 0 && pi->value == NULL)) return -1;
        append_child(parent, pi);
    }
    advance_n(P, draw + 2);                           /* data + "?>" */
    return 0;
}

/* ---- per-construct handlers (the tokenizer dispatch calls one per token) ----
 * Each takes only the parser (which now carries the open-element stack), returns
 * 0 on success / -1 on a well-formedness or budget failure (P->status set), and
 * leaves P->p just past the construct. */

/* The parent a node created at the cursor attaches to: the innermost open
 * element, or - at the document level (prolog/epilog) - the DOCUMENT node, so a
 * top-level comment / PI becomes its child (XPath data model; like Nokogiri).
 * The document node exists from the start of the parse. */
static inline mkr_xml_node_t *
cur_parent(const mkr_xml_parser_t *P)
{
    if (P->depth) return P->stack[P->depth - 1];
    return P->fragment ? P->fragment : P->doc->doc_node;
}

/* End tag '</name S? >' (P->p at '/'): must match the innermost open element's
 * raw QName; on match pops it and restores that element's namespace scope. */
static int
parse_end_tag(mkr_xml_parser_t *P)
{
    advance(P);                                       /* '/' */
    const char *nm; uint32_t nl;
    if (scan_name(P, &nm, &nl) != 0) return -1;
    skip_ws(P);
    if (mkr_span_peek(&P->in) != '>') { set_syntax(P); return -1; }
    advance(P);
    if (P->depth == 0) { set_syntax(P); return -1; }  /* end tag with no open element */
    mkr_xml_node_t *top = P->stack[P->depth - 1];
    uint32_t tql = top->prefix_len ? top->prefix_len + 1 + top->local_len : top->local_len;
    int match;
    if (top->prefix_len) {
        mkr_span_t nsp = mkr_span(nm, nl);
        match = (nl == tql && mkr_span_starts(&nsp, top->prefix, top->prefix_len)
                 && mkr_span_at(&nsp, top->prefix_len) == ':');
        if (match) {
            mkr_span_skip(&nsp, (size_t)top->prefix_len + 1);
            match = mkr_span_starts(&nsp, top->local, top->local_len);
        }
    } else {
        match = mkr_bytes_eq(top->local, top->local_len, nm, nl);
    }
    if (!match) { set_syntax(P); return -1; }          /* mismatched end tag */
    P->depth--;
    P->nbind = P->frame[P->depth];                     /* pop this element's namespace scope */
    return 0;
}

/* A quoted literal ("..." or '...'); its content is a slice of the input (no
 * reference recognition). 0 on success, -1 on a missing/unterminated quote. */
static int
parse_quoted(mkr_xml_parser_t *P, const char **out, uint32_t *out_len)
{
    int q = mkr_span_peek(&P->in);
    if (q != '"' && q != '\'') { set_syntax(P); return -1; }
    advance(P);
    const char *s = mkr_span_mark(&P->in);
    while (mkr_span_peek(&P->in) >= 0 && mkr_span_peek(&P->in) != q) advance(P);
    if (mkr_span_left(&P->in) == 0) { set_syntax(P); return -1; }   /* unterminated literal */
    size_t n = mkr_span_since(&P->in, s);
    if (n > UINT32_MAX) { P->status = MKR_XML_ERR_LIMIT; return -1; }
    *out = s; *out_len = (uint32_t)n;
    advance(P);                                          /* closing quote */
    return 0;
}

/* '<!DOCTYPE' (P->p at '!'): the DTD is RECOGNIZED but NOT PROCESSED (§9.4
 * alternative). Valid only in the prolog (before the root element) and at most
 * once. The Name and the ExternalID (SYSTEM/PUBLIC literals) are read for
 * Document#internal_subset, then any internal subset '[ ... ]' is skipped to the
 * true '>' (quote state + bracket depth balance a '>' inside a quoted literal or
 * a markup declaration). NOTHING in the DTD is processed: no entity is defined
 * (so a DTD-defined &name; stays an undefined-entity error per §9.1, keeping XXE
 * and billion-laughs impossible) and no external subset is fetched (zero I/O).
 * The metadata is kept in an off-tree DOCUMENT_TYPE node (doc->doctype). */
static int
parse_doctype(mkr_xml_parser_t *P)
{
    if (P->depth != 0 || P->doc->root != NULL || P->saw_doctype) {
        set_syntax(P); return -1;   /* inside an element, after the root, or a second DOCTYPE */
    }
    P->saw_doctype = 1;
    advance_n(P, MKR_LIT_LEN("!DOCTYPE"));
    if (!is_space_byte(mkr_span_peek(&P->in))) { set_syntax(P); return -1; } /* S required */
    skip_ws(P);
    const char *name; uint32_t name_len;
    if (scan_name(P, &name, &name_len) != 0) return -1; /* the document type name (a Name) */

    const char *pub = NULL, *sys = NULL;
    uint32_t pub_len = 0, sys_len = 0;

    if (is_space_byte(mkr_span_peek(&P->in))) {
        skip_ws(P);
        if (lit_ahead(P, "SYSTEM", 6)) {
            advance_n(P, 6);
            if (!is_space_byte(mkr_span_peek(&P->in))) { set_syntax(P); return -1; }
            skip_ws(P);
            if (parse_quoted(P, &sys, &sys_len) != 0) return -1;
        } else if (lit_ahead(P, "PUBLIC", 6)) {
            advance_n(P, 6);
            if (!is_space_byte(mkr_span_peek(&P->in))) { set_syntax(P); return -1; }
            skip_ws(P);
            if (parse_quoted(P, &pub, &pub_len) != 0) return -1;
            if (!is_space_byte(mkr_span_peek(&P->in))) { set_syntax(P); return -1; }
            skip_ws(P);
            if (parse_quoted(P, &sys, &sys_len) != 0) return -1;
        }
    }

    /* Skip the optional internal subset + trailing whitespace to the true '>'. */
    int quote = 0;
    int depth = 0, closed = 0;
    for (;;) {
        int c = mkr_span_peek(&P->in);
        if (c < 0) break;
        if (quote)                          { if (c == quote) quote = 0; }
        else if (c == '"' || c == '\'')     { quote = c; }
        else if (c == '[')                  { depth++; }
        else if (c == ']')                  { if (depth > 0) depth--; }
        else if (c == '>' && depth == 0)    { advance(P); closed = 1; break; }
        advance(P);
    }
    if (!closed) { set_syntax(P); return -1; }   /* unterminated DOCTYPE */

    /* Retain the metadata off-tree (not a child of any node, so XPath is
     * unaffected) for Document#internal_subset. Fields are repurposed:
     * local/qname = name, prefix = public/external id, value = system id. */
    mkr_xml_node_t *dt = mkr_xml_arena_node(P->doc, MKR_XML_NODE_TYPE_DOCUMENT_TYPE);
    if (dt == NULL) { propagate_oom(P); return -1; }
    dt->local = dt->qname = own(P, name, name_len);
    dt->local_len = dt->qname_len = name_len;
    if (name_len > 0 && dt->local == NULL) return -1;
    if (pub != NULL) {
        dt->prefix = own(P, pub, pub_len); dt->prefix_len = pub_len;
        if (pub_len > 0 && dt->prefix == NULL) return -1;
    }
    if (sys != NULL) {
        dt->value = own(P, sys, sys_len); dt->value_len = sys_len;
        if (sys_len > 0 && dt->value == NULL) return -1;
    }
    P->doc->doctype = dt;
    return 0;
}

/* '<!' markup (P->p at '!'): a comment, a CDATA section, or a DOCTYPE (recognized
 * but not processed, §9.4). Any other markup declaration is rejected. */
static int
parse_markup(mkr_xml_parser_t *P)
{
    mkr_xml_node_t *parent = cur_parent(P);
    if (lit_ahead(P, "!--", 3))      return parse_comment(P, parent);
    if (lit_ahead(P, "![CDATA[", 8)) return parse_cdata(P, parent);
    if (lit_ahead(P, "!DOCTYPE", 8)) {
        if (P->fragment != NULL) { set_syntax(P); return -1; }   /* a fragment has no DOCTYPE */
        return parse_doctype(P);
    }
    set_syntax(P); return -1;
}

/* Start tag (P->p at the QName's first byte; +tl+/+tc+ = the '<' position).
 * Builds the element, resolves its namespaces, links it into the tree, then on
 * '>' pushes it onto the open-element stack or on '/>' leaves it a leaf. */
static int
parse_start_tag(mkr_xml_parser_t *P, uint32_t tl, uint32_t tc)
{
    const char *nm; uint32_t nl;
    if (scan_name(P, &nm, &nl) != 0) return -1;
    const char *epfx, *eloc; uint32_t epl, ell;
    if (split_qname(nm, nl, &epfx, &epl, &eloc, &ell) != 0) { set_syntax(P); return -1; }
    (void)epfx;
    mkr_xml_node_t *el = mkr_xml_arena_node(P->doc, MKR_XML_NODE_TYPE_ELEMENT);
    if (el == NULL) { propagate_oom(P); return -1; }
    if (set_node_qname(P, el, nm, nl, eloc, ell, epl) != 0) return -1;
    el->line = tl; el->col = tc;

    if (P->depth == 0 && P->fragment == NULL) {
        if (P->doc->root != NULL) { set_syntax(P); return -1; }   /* multiple roots */
        P->doc->root = el;
    }   /* a fragment allows multiple top-level elements and sets no doc->root */
    append_child(cur_parent(P), el);   /* depth 0: the DOCUMENT (or fragment) node; else the open element */

    size_t bind_base = P->nbind;       /* xmlns on this element go above here */
    int pushed = 0;
    if (parse_element_body(P, el, &pushed) != 0) return -1;
    if (pushed) {
        if (P->depth + 1 > MKR_XML_MAX_DEPTH) { P->status = MKR_XML_ERR_LIMIT; return -1; }
        if (mkr_grow_reserve((void **)&P->stack, &P->scap, P->depth + 1, sizeof(*P->stack)) != MKR_OK
            || mkr_grow_reserve((void **)&P->frame, &P->fcap, P->depth + 1, sizeof(*P->frame)) != MKR_OK) {
            P->status = MKR_XML_ERR_OOM; return -1;
        }
        P->stack[P->depth] = el; P->frame[P->depth] = bind_base; P->depth++;
    } else {
        P->nbind = bind_base;          /* self-closing: pop its namespace scope now */
    }
    return 0;
}

/* Character data up to the next '<'. Inside an element it becomes a TEXT node
 * (references expanded, XML Char validated); at the document level only white
 * space (misc) is permitted. */
static int
parse_text(mkr_xml_parser_t *P)
{
    const char *tstart = mkr_span_mark(&P->in);
    int nonspace = 0;
    for (;;) {
        int c = mkr_span_peek(&P->in);
        if (c < 0 || c == '<') break;
        /* §2.4: the literal "]]>" MUST NOT appear in character data - only a
         * CDATA section (handled by parse_cdata) may end with it. A lone ']' or
         * "]]" not followed by '>' is fine. */
        if (c == ']' && lit_ahead(P, "]]>", 3)) { set_syntax(P); return -1; }
        if (!(c == ' ' || c == '\t' || c == '\n' || c == '\r')) nonspace = 1;
        advance(P);
    }
    size_t traw = mkr_span_since(&P->in, tstart);
    if (traw > UINT32_MAX) { P->status = MKR_XML_ERR_LIMIT; return -1; }   /* fail closed */
    uint32_t tlen = (uint32_t)traw;
    if (P->depth == 0 && P->fragment == NULL) {
        if (nonspace) { set_syntax(P); return -1; }   /* non-ws text outside any element */
        return 0;                                     /* document-level whitespace (misc): discarded */
    }
    /* expand char/entity references + validate XML Char (§9.1/§9.2). At the fragment
     * top level (depth 0) char data is a child of the fragment node itself. */
    uint32_t tvlen = 0;
    const char *tval = mkr_xml_expand(P->doc, tstart, tlen, MKR_XML_EXPAND_TEXT, &tvlen, &P->status);
    if (tval == NULL) return -1;
    return append_chardata(P, cur_parent(P), MKR_XML_NODE_TYPE_TEXT, tval, tvlen);
}

/* Fold CRLF and a lone CR to LF over the whole input before tokenizing (§9.3b-A),
 * so every later stage (attribute-value normalization, line counting) sees only
 * LF. Only when a CR is actually present is the input copied into a freshly
 * malloc'd scratch buffer (the common no-CR input tokenizes in place, zero copy);
 * it can only shrink. On return, body+blen are the bytes to parse and norm is the
 * scratch buffer the caller must free (NULL if none). Returns 0, or -1 on OOM. */
static int
normalize_newlines(const char *src, size_t len, const char **body, size_t *blen, char **norm)
{
    *body = src; *blen = len; *norm = NULL;
    mkr_span_t s = mkr_span(src, len);
    size_t cr_at;
    if (!mkr_span_find(&s, '\r', &cr_at)) return 0;
    char *buf = mkr_reallocarray(NULL, len == 0 ? 1 : len, 1);
    if (buf == NULL) return -1;
    /* read through the span, write through the bounded writer (the output can
     * only shrink, so `len` capacity always suffices - enforced, not trusted). */
    mkr_spanbuf_t w = mkr_spanbuf(buf, len);
    for (;;) {
        int ch = mkr_span_take(&s);
        if (ch < 0) break;
        if (ch == '\r') {
            mkr_spanbuf_putc(&w, '\n');
            if (mkr_span_peek(&s) == '\n') mkr_span_skip(&s, 1);   /* CRLF -> single LF */
        } else {
            mkr_spanbuf_putc(&w, (char)ch);
        }
    }
    if (mkr_spanbuf_finish(&w) == NULL) { free(buf); return -1; }  /* can't happen; fail closed */
    *body = buf; *blen = w.pos; *norm = buf;
    return 0;
}

/* Tokenizer dispatch: classify the construct at P->p and hand it to its handler.
 * The handlers carry all parse state through P (cursor, namespace scope, AND the
 * open-element stack), so this loop stays a thin classifier. Shared by the
 * document and fragment entries - P->fragment selects the depth-0 rules; the
 * caller does the mode-specific post-checks. */
static void
run_tokenizer(mkr_xml_parser_t *P)
{
    while (mkr_span_left(&P->in) > 0) {
        if (mkr_span_peek(&P->in) != '<') {
            if (parse_text(P) != 0) break;
        } else {
            uint32_t tl = P->line, tc = P->col;     /* the '<' position (start-tag line/col) */
            int at_start = (mkr_span_mark(&P->in) == P->start) && P->fragment == NULL; /* '<?xml ...?>' only in a document prolog */
            advance(P);                              /* '<' */
            int c = mkr_span_peek(&P->in);
            if (c < 0) { set_syntax(P); break; }
            int rc;
            switch (c) {
            case '/': rc = parse_end_tag(P);                          break;
            case '!': rc = parse_markup(P);                           break;  /* comment / CDATA / DTD */
            case '?': rc = parse_pi(P, cur_parent(P), at_start);      break;
            default:  rc = parse_start_tag(P, tl, tc);                break;
            }
            if (rc != 0) break;
        }
        if (P->status != MKR_XML_OK) break;
    }
}

/* Seed a fragment parser's binding scope with the document root's in-scope
 * namespace declarations (its xmlns / xmlns:* DOM attributes ARE the in-scope set:
 * the root is the top, and the xml: prefix is implicit in ns_lookup), so a
 * prefixed (or default-namespaced) name in a Document#fragment resolves against
 * the document, Nokogiri-faithfully. 0, or -1 on a binding-stack failure. */
static int
seed_doc_namespaces(mkr_xml_parser_t *P)
{
    mkr_xml_node_t *root = P->doc->root;
    if (root == NULL) return 0;
    for (mkr_xml_node_t *a = root->attrs; a != NULL; a = a->next) {
        const char *bpfx; uint32_t bpl;
        if (!mkr_xml_xmlns_prefix(a->qname, a->qname_len, &bpfx, &bpl)) continue;
        if (push_binding(P, bpfx, bpl, a->value ? a->value : "", a->value_len) != 0) return -1;
    }
    return 0;
}

/* Free a parser's heap-side scratch arrays (NOT the document or its arena). Safe
 * on a zero-initialised parser - every free(NULL) is a no-op - so it serves every
 * exit path of both parse entries. */
static void
parser_dispose(mkr_xml_parser_t *P)
{
    free(P->stack);
    free(P->frame);
    free(P->binds);
    free(P->ratt);
}

mkr_xml_doc_t *
mkr_xml_parse(const char *src, size_t len, mkr_xml_status_t *status)
{
    return mkr_xml_parse_ex(src, len, NULL, status);
}

mkr_xml_doc_t *
mkr_xml_parse_ex(const char *src, size_t len, const mkr_xml_limits_t *limits,
                 mkr_xml_status_t *status)
{
    mkr_xml_doc_t *doc = mkr_xml_doc_new();
    if (doc == NULL) { if (status) *status = MKR_XML_ERR_OOM; return NULL; }

    /* Apply per-parse budget overrides over the compile-time defaults (a 0 field
     * leaves the default; a NULL +limits+ leaves all defaults). Only max_bytes is
     * runtime-configurable today. */
    if (limits != NULL && limits->max_bytes != 0) doc->max_bytes = limits->max_bytes;

    /* Fail closed on an over-budget input before ANY allocation (src is not yet
     * read): every retained byte is copied into the arena, so an input longer
     * than the arena byte budget can never succeed - reject it now instead of
     * after building most of a doomed document. This also bounds the line-ending
     * scratch below (norm <= len <= max_bytes), so it cannot run a large
     * out-of-budget allocation ahead of the arena's own checks. */
    if (len > doc->max_bytes) {
        if (status) *status = MKR_XML_ERR_LIMIT;
        mkr_xml_doc_destroy(doc);
        return NULL;
    }

    /* Create the DOCUMENT node up front: it is the XPath "/" root, what a Ruby
     * Document wraps, and the parent that top-level comments / PIs and the root
     * element attach to during the parse (cur_parent at depth 0). */
    doc->doc_node = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_DOCUMENT);
    if (doc->doc_node == NULL) {
        if (status) *status = doc->oom;
        mkr_xml_doc_destroy(doc);
        return NULL;
    }

    const char *body; size_t blen; char *norm;
    if (normalize_newlines(src, len, &body, &blen, &norm) != 0) {
        if (status) *status = MKR_XML_ERR_OOM;
        mkr_xml_doc_destroy(doc);
        return NULL;
    }

    mkr_xml_parser_t P = { .in = mkr_span(body, blen), .start = body,
                           .line = 1, .col = 1, .doc = doc, .status = MKR_XML_OK };
    run_tokenizer(&P);

    if (P.status == MKR_XML_OK) {
        if (P.depth != 0)           set_syntax(&P);   /* unclosed element(s) */
        else if (doc->root == NULL) set_syntax(&P);   /* no root element */
    }

    parser_dispose(&P);
    free(norm);                       /* scratch line-ending-normalized buffer (if any) */
    if (P.status != MKR_XML_OK) {
        if (status) *status = P.status;
        mkr_xml_doc_destroy(doc);
        return NULL;
    }
    if (status) *status = MKR_XML_OK;
    return doc;
}

mkr_xml_node_t *
mkr_xml_parse_fragment(mkr_xml_doc_t *doc, const char *src, size_t len,
                       int inherit_doc_ns, mkr_xml_status_t *status)
{
    /* Coarse fast-fail before any allocation; the arena's per-cut checks enforce
     * the remaining budget during the parse (an existing document has already
     * spent some of it). */
    if (len > doc->max_bytes) { if (status) *status = MKR_XML_ERR_LIMIT; return NULL; }

    mkr_xml_node_t *frag = mkr_xml_arena_node(doc, MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT);
    if (frag == NULL) { if (status) *status = doc->oom; return NULL; }

    const char *body; size_t blen; char *norm;
    if (normalize_newlines(src, len, &body, &blen, &norm) != 0) {
        if (status) *status = MKR_XML_ERR_OOM;
        return NULL;   /* frag stays in the arena, detached (never freed) */
    }

    mkr_xml_parser_t P = { .in = mkr_span(body, blen), .start = body, .line = 1,
                           .col = 1, .doc = doc, .fragment = frag, .status = MKR_XML_OK };

    if (inherit_doc_ns && seed_doc_namespaces(&P) != 0) {
        parser_dispose(&P); free(norm);
        if (status) *status = P.status;
        return NULL;
    }

    run_tokenizer(&P);
    if (P.status == MKR_XML_OK && P.depth != 0) set_syntax(&P);   /* unclosed element(s) */

    parser_dispose(&P);
    free(norm);
    if (P.status != MKR_XML_OK) {
        if (status) *status = P.status;
        return NULL;   /* the partial fragment stays detached in the arena */
    }
    if (status) *status = MKR_XML_OK;
    return frag;
}

/* ---- parse self-test (structural; run from Makiri.__c_selftest under ASan) --
 * Returns 0 on success or the 1-based index of the first failing check. Test
 * strings are compile-time literals, so lengths come from sizeof - 1 (no
 * strlen on a non-validated string -> clint clean). */
static int nlname(const mkr_xml_node_t *n, const char *s, size_t l)
{
    return n && mkr_bytes_eq(n->local, n->local_len, s, l);
}
static int nlval(const mkr_xml_node_t *n, const char *s, size_t l)
{
    return n && mkr_bytes_eq(n->value, n->value_len, s, l);
}
static int nlns(const mkr_xml_node_t *n, const char *s, size_t l)
{
    return n && mkr_bytes_eq(n->ns_uri, n->ns_uri_len, s, l);
}
static int nlpfx(const mkr_xml_node_t *n, const char *s, size_t l)
{
    return n && mkr_bytes_eq(n->prefix, n->prefix_len, s, l);
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
    if (!NAME_IS(root, "Feed") || root->type != MKR_XML_NODE_TYPE_ELEMENT
        || root->line != 1 || root->col != 1) { mkr_xml_doc_destroy(d); return i; }

    i++; /* 3: attributes x=1, y=two in order */
    mkr_xml_node_t *a0 = root->attrs;
    mkr_xml_node_t *a1 = a0 ? a0->next : NULL;
    if (!a0 || a0->type != MKR_XML_NODE_TYPE_ATTRIBUTE || !NAME_IS(a0, "x") || !VAL_IS(a0, "1")
        || !a1 || !NAME_IS(a1, "y") || !VAL_IS(a1, "two") || a1->next != NULL) {
        mkr_xml_doc_destroy(d); return i;
    }

    i++; /* 4: children = text "hi", element <b>, text "z" */
    mkr_xml_node_t *c0 = root->first_child;
    mkr_xml_node_t *c1 = c0 ? c0->next : NULL;
    mkr_xml_node_t *c2 = c1 ? c1->next : NULL;
    if (!c0 || c0->type != MKR_XML_NODE_TYPE_TEXT || !VAL_IS(c0, "hi")
        || !c1 || c1->type != MKR_XML_NODE_TYPE_ELEMENT || !NAME_IS(c1, "b") || c1->first_child != NULL
        || !c2 || c2->type != MKR_XML_NODE_TYPE_TEXT || !VAL_IS(c2, "z") || c2->next != NULL) {
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
            || !tx || tx->type != MKR_XML_NODE_TYPE_TEXT || !VAL_IS(tx, "1<2>3&4'5\"6")) {
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

    /* §7: namespaces - prefixed element + attribute, default-ns inheritance,
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
        if (!a || !NAME_IS(a, "a") || !PFX_IS(a, "xmlns") || !NS_IS(a, MKR_XMLNS_NS_URI)) { mkr_xml_doc_destroy(d); return i; }
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

    /* §3.3.3: attribute-value normalization - a LITERAL tab/newline/CR in the
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
            || !tx || tx->type != MKR_XML_NODE_TYPE_TEXT || !VAL_IS(tx, "u\tv\nw")) { /* text unfolded */
            mkr_xml_doc_destroy(d); return i;
        }
    }
    mkr_xml_doc_destroy(d);

    /* §9: comment / CDATA / PI nodes inside an element; the prolog XML declaration
     * is not retained, but a prolog PI / comment IS, as a child of the DOCUMENT
     * node (before the root); '<!DOCTYPE' is fail-closed elsewhere. */
    i++; /* 13 */
    d = PARSE_LIT("<?xml version=\"1.0\"?><?xml-stylesheet href=\"x\"?><!--top-->"
                  "<r><!--c--><![CDATA[a<b]]><?pi dat?></r><?tail t?>", &st);
    if (d == NULL || st != MKR_XML_OK) { if (d) mkr_xml_doc_destroy(d); return i; }
    {
        mkr_xml_node_t *r = d->root;
        if (!NAME_IS(r, "r")) { mkr_xml_doc_destroy(d); return i; }
        mkr_xml_node_t *cm = r->first_child;
        mkr_xml_node_t *cd = cm ? cm->next : NULL;
        mkr_xml_node_t *pi = cd ? cd->next : NULL;
        if (!cm || cm->type != MKR_XML_NODE_TYPE_COMMENT || !VAL_IS(cm, "c")
            || !cd || cd->type != MKR_XML_NODE_TYPE_CDATA_SECTION || !VAL_IS(cd, "a<b") /* raw '<' kept */
            || !pi || pi->type != MKR_XML_NODE_TYPE_PI || !NAME_IS(pi, "pi") || !VAL_IS(pi, "dat")
            || pi->next != NULL) {
            mkr_xml_doc_destroy(d); return i;
        }
        /* the DOCUMENT node's children: prolog PI (xml-stylesheet) + comment,
         * then the root, then the epilog PI (the <?xml?> declaration is NOT a
         * node); inter-construct whitespace is not retained. */
        mkr_xml_node_t *dn = d->doc_node;
        mkr_xml_node_t *p1 = dn ? dn->first_child : NULL;        /* <?xml-stylesheet?> */
        mkr_xml_node_t *p2 = p1 ? p1->next : NULL;               /* <!--top--> */
        mkr_xml_node_t *p3 = p2 ? p2->next : NULL;               /* <r> (root) */
        mkr_xml_node_t *p4 = p3 ? p3->next : NULL;               /* <?tail?> */
        if (!p1 || p1->type != MKR_XML_NODE_TYPE_PI || !NAME_IS(p1, "xml-stylesheet")
            || !p2 || p2->type != MKR_XML_NODE_TYPE_COMMENT || !VAL_IS(p2, "top")
            || p3 != r || r->parent != dn
            || !p4 || p4->type != MKR_XML_NODE_TYPE_PI || !NAME_IS(p4, "tail")
            || p4->next != NULL) {
            mkr_xml_doc_destroy(d); return i;
        }
    }
    mkr_xml_doc_destroy(d);

    i++; /* 14: §9 fail-closed cases */
    st = MKR_XML_OK;
    e = PARSE_LIT("<r/><!DOCTYPE r>",   &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* DOCTYPE after root */
    st = MKR_XML_OK;
    e = PARSE_LIT("<!DOCTYPE r [ <!ENTITY x \"y\"> ]><r>&x;</r>", &st);
    if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* DTD entity not registered -> &x; undefined */
    st = MKR_XML_OK;
    e = PARSE_LIT("<r><!-- a--b --></r>", &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* '--' in comment */
    st = MKR_XML_OK;
    e = PARSE_LIT("<r><!-- c </r>",     &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* unterminated comment */
    st = MKR_XML_OK;
    e = PARSE_LIT(" <?xml version=\"1.0\"?><r/>", &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* decl not at start */
    st = MKR_XML_OK;
    e = PARSE_LIT("<![CDATA[x]]><r/>",  &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* CDATA outside root */

    /* §9.4 alternative: DOCTYPE is recognized but not processed - the external ID
     * and the internal subset (with a '>' inside a markup decl, and '>' inside a
     * quoted literal) are skipped to the true '>', nothing in the DTD is
     * registered, and parsing continues into the root. The name + external ID are
     * retained in an off-tree DOCUMENT_TYPE node (doc->doctype, NOT a child of any
     * node, so the tree/XPath is unaffected); here SYSTEM "a>b" (with a quoted
     * '>') gives system id "a>b" and no public id. */
    i++; /* 14b */
    d = PARSE_LIT("<!DOCTYPE r SYSTEM \"a>b\" [ <!ELEMENT r (#PCDATA)> ]><r>ok</r>", &st);
    if (d == NULL || st != MKR_XML_OK || !NAME_IS(d->root, "r")
        || !VAL_IS(d->root->first_child, "ok")) {
        if (d) mkr_xml_doc_destroy(d); return i;
    }
    {
        mkr_xml_node_t *dt = d->doctype;
        if (dt == NULL || dt->type != MKR_XML_NODE_TYPE_DOCUMENT_TYPE
            || dt->parent != NULL || dt->next != NULL || dt->prev != NULL  /* off-tree */
            || !slice_eq(dt->local, dt->local_len, "r", 1)
            || dt->prefix != NULL                                          /* no PUBLIC id */
            || !slice_eq(dt->value, dt->value_len, "a>b", 3)) {            /* SYSTEM id */
            mkr_xml_doc_destroy(d); return i;
        }
    }
    mkr_xml_doc_destroy(d);

    /* §9.3b-A: line-ending normalization happens before attribute-value
     * normalization, so a literal CRLF inside an attribute collapses to ONE space
     * (CRLF -> LF -> space); in text content CRLF/CR just fold to LF, unfolded. */
    i++; /* 15 */
    d = PARSE_LIT("<a x=\"p\r\nq\r\">m\r\nn\ro</a>", &st);
    if (d == NULL || st != MKR_XML_OK) { if (d) mkr_xml_doc_destroy(d); return i; }
    {
        mkr_xml_node_t *r = d->root;
        mkr_xml_node_t *ax = r->attrs;
        mkr_xml_node_t *tx = r->first_child;
        if (!VAL_IS(ax, "p q ")                 /* CRLF and lone CR each -> one space */
            || !tx || tx->type != MKR_XML_NODE_TYPE_TEXT || !VAL_IS(tx, "m\nn\no")) { /* folded to LF */
            mkr_xml_doc_destroy(d); return i;
        }
    }
    mkr_xml_doc_destroy(d);

    i++; /* 16: §9.2b strict names + §9.3 duplicate attributes fail closed */
    st = MKR_XML_OK;
    e = PARSE_LIT("<1bad/>",            &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* NameStartChar */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a:1b xmlns:a='u'/>", &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* NCName local */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a x='1' x='2'/>",   &st); if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* duplicate attr */
    st = MKR_XML_OK;
    e = PARSE_LIT("<e xmlns:a='u' xmlns:b='u' a:x='1' b:x='2'/>", &st);
    if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; } /* same-URI duplicate */
    st = MKR_XML_OK;
    e = PARSE_LIT("<a>foo]]>bar</a>", &st);  /* §2.4: literal ]]> forbidden in content */
    if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; }
    st = MKR_XML_OK;
    d = PARSE_LIT("<a>1]2]]3</a>", &st);      /* but a lone ] / ]] is fine */
    if (d == NULL || st != MKR_XML_OK || !VAL_IS(d->root->first_child, "1]2]]3")) {
        if (d) mkr_xml_doc_destroy(d); return i;
    }
    mkr_xml_doc_destroy(d);

    i++; /* 17: §2.8 XML declaration grammar + §2.6 reserved/colon PI targets */
    {
        /* a fully-specified valid declaration parses */
        st = MKR_XML_OK;
        d = PARSE_LIT("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><r/>", &st);
        if (d == NULL || st != MKR_XML_OK) { if (d) mkr_xml_doc_destroy(d); return i; }
        mkr_xml_doc_destroy(d);
        static const char *bad[] = {
            "<?xml VERSION=\"1.0\"?><r/>",                 /* keyword case */
            "<?xml version=\"1.0\" standalone=\"YES\"?><r/>", /* value case */
            "<?xml encoding=\"UTF-8\"?><r/>",              /* version required */
            "<?xml version=\"1.0\"encoding=\"UTF-8\"?><r/>",  /* missing S separator */
            "<?xml version=\"1.0\" version=\"1.0\"?><r/>", /* duplicate */
            "<?xml version=\"1.0\" valid=\"no\"?><r/>",    /* unknown pseudo-attr */
            "<?xml version=\"1.0' ?><r/>",                 /* mismatched quotes */
            "<?xml version=\"1.0^\"?><r/>",                /* bad VersionNum char */
            "<?XML version=\"1.0\"?><r/>",                 /* reserved target (wrong case) */
            "<r><?a:b data?></r>",                         /* colon in PITarget (NS) */
            "<a x=\"1\"y=\"2\"/>",                          /* §3.1: missing S between attrs */
            "<a>&#X58;</a>",                               /* §4.1: hex marker must be lowercase x */
        };
        for (size_t k = 0; k < sizeof(bad) / sizeof(bad[0]); k++) {
            st = MKR_XML_OK;
            e = PARSE_LIT(bad[k], &st);
            if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; }
        }
        /* a well-formed but unsupported version (XML 1.1 / 1.x) is fail-closed
         * with its own status, distinct from a malformedness SYNTAX error. */
        st = MKR_XML_OK;
        e = PARSE_LIT("<?xml version=\"1.1\"?><r/>", &st);
        if (e || st != MKR_XML_ERR_VERSION) { if (e) mkr_xml_doc_destroy(e); return i; }
        st = MKR_XML_OK;
        e = PARSE_LIT("<?xml version=\"1.5\"?><r/>", &st);
        if (e || st != MKR_XML_ERR_VERSION) { if (e) mkr_xml_doc_destroy(e); return i; }
        /* version="2.0" is not even a valid VersionNum ('1.' [0-9]+) -> SYNTAX */
        st = MKR_XML_OK;
        e = PARSE_LIT("<?xml version=\"2.0\"?><r/>", &st);
        if (e || st != MKR_XML_ERR_SYNTAX) { if (e) mkr_xml_doc_destroy(e); return i; }
    }

    /* §4 byte-budget entry guard: an input longer than MKR_XML_MAX_BYTES fails
     * closed (MKR_XML_ERR_LIMIT) before any allocation. The guard tests `len`
     * only, so a tiny buffer with a huge claimed length exercises it without
     * allocating - and proves `src` is not dereferenced past the budget. */
    i++; /* 18 */
    st = MKR_XML_OK;
    {
        static const char tiny[] = "<r/>";
        if (mkr_xml_parse(tiny, (size_t)MKR_XML_MAX_BYTES + 1u, &st) != NULL
            || st != MKR_XML_ERR_LIMIT) {
            return i;
        }
        /* and the boundary (== budget) is NOT rejected by the guard (it parses,
         * since this real 4-byte input is well under the budget). */
        st = MKR_XML_OK;
        d = mkr_xml_parse(tiny, sizeof(tiny) - 1, &st);
        if (d == NULL || st != MKR_XML_OK) { if (d) mkr_xml_doc_destroy(d); return i; }
        mkr_xml_doc_destroy(d);
    }

    /* §4 per-parse override (mkr_xml_parse_ex): a tiny max_bytes lowers the
     * effective budget so a document that parses under the default now fails
     * closed, and a NULL/zeroed limits reproduces the default. */
    i++; /* 19 */
    {
        static const char doc_src[] = "<root><a/><b/><c/></root>";
        size_t dlen = sizeof(doc_src) - 1;
        /* a 2-byte budget trips the entry guard (len > max_bytes) */
        st = MKR_XML_OK;
        mkr_xml_limits_t tiny_budget = { .max_bytes = 2 };
        if (mkr_xml_parse_ex(doc_src, dlen, &tiny_budget, &st) != NULL || st != MKR_XML_ERR_LIMIT) {
            return i;
        }
        /* a budget that passes the entry guard but cannot hold the nodes trips the
         * arena allocator instead (one 128-byte node already exceeds 64). */
        st = MKR_XML_OK;
        mkr_xml_limits_t small_budget = { .max_bytes = 64 };
        if (mkr_xml_parse_ex(doc_src, dlen, &small_budget, &st) != NULL || st != MKR_XML_ERR_LIMIT) {
            return i;
        }
        /* a generous override parses the same document; zeroed limits == default */
        st = MKR_XML_OK;
        mkr_xml_limits_t big_budget = { .max_bytes = (size_t)1024u * 1024u };
        d = mkr_xml_parse_ex(doc_src, dlen, &big_budget, &st);
        if (d == NULL || st != MKR_XML_OK || !NAME_IS(d->root, "root")) {
            if (d) mkr_xml_doc_destroy(d); return i;
        }
        mkr_xml_doc_destroy(d);
        st = MKR_XML_OK;
        mkr_xml_limits_t zero = { 0 };
        d = mkr_xml_parse_ex(doc_src, dlen, &zero, &st);
        if (d == NULL || st != MKR_XML_OK) { if (d) mkr_xml_doc_destroy(d); return i; }
        mkr_xml_doc_destroy(d);
    }

    /* 20: fragment - multiple top-level nodes (no single-root rule), char data at
     *     the top level, a self-declared namespace. */
    i++;
    {
        static const char fsrc[] = "<a/>txt<p:b xmlns:p='urn:p'>x</p:b>";
        mkr_xml_doc_t *fd = mkr_xml_doc_new();
        if (fd == NULL) return i;
        st = MKR_XML_OK;
        mkr_xml_node_t *frag = mkr_xml_parse_fragment(fd, fsrc, sizeof(fsrc) - 1, 0, &st);
        if (frag == NULL || st != MKR_XML_OK
            || frag->type != MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT) { mkr_xml_doc_destroy(fd); return i; }
        mkr_xml_node_t *c0 = frag->first_child;
        mkr_xml_node_t *c1 = c0 ? c0->next : NULL;
        mkr_xml_node_t *c2 = c1 ? c1->next : NULL;
        if (!NAME_IS(c0, "a") || c0->type != MKR_XML_NODE_TYPE_ELEMENT
            || c1 == NULL || c1->type != MKR_XML_NODE_TYPE_TEXT || !VAL_IS(c1, "txt")
            || !NAME_IS(c2, "b") || !NS_IS(c2, "urn:p") || c2->next != NULL) {
            mkr_xml_doc_destroy(fd); return i;
        }
        mkr_xml_doc_destroy(fd);
    }

    /* 21: a fragment fails closed on an XML declaration, a DOCTYPE, a stray end
     *     tag, an unclosed element, and (standalone) an unbound prefix. */
    i++;
    {
        mkr_xml_doc_t *fd = mkr_xml_doc_new();
        if (fd == NULL) return i;
#define FRAG_REJECTS(lit) do {                                                  \
            st = MKR_XML_OK;                                                    \
            if (mkr_xml_parse_fragment(fd, (lit), sizeof(lit) - 1, 0, &st) != NULL \
                || st == MKR_XML_OK) { mkr_xml_doc_destroy(fd); return i; }     \
        } while (0)
        FRAG_REJECTS("<?xml version='1.0'?>");
        FRAG_REJECTS("<!DOCTYPE r>");
        FRAG_REJECTS("</x>");
        FRAG_REJECTS("<a>");
        FRAG_REJECTS("<p:a/>");
#undef FRAG_REJECTS
        mkr_xml_doc_destroy(fd);
    }

    /* 22: inherit_doc_ns resolves a prefixed (and default-namespaced) fragment name
     *     against the document's root namespaces (Document#fragment). */
    i++;
    {
        static const char fsrc[] = "<p:a/><plain/>";
        st = MKR_XML_OK;
        mkr_xml_doc_t *fd = PARSE_LIT("<r xmlns:p='urn:p' xmlns='urn:d'/>", &st);
        if (fd == NULL || st != MKR_XML_OK) { if (fd) mkr_xml_doc_destroy(fd); return i; }
        st = MKR_XML_OK;
        mkr_xml_node_t *frag = mkr_xml_parse_fragment(fd, fsrc, sizeof(fsrc) - 1, 1, &st);
        mkr_xml_node_t *a = frag ? frag->first_child : NULL;
        mkr_xml_node_t *plain = a ? a->next : NULL;
        if (frag == NULL || st != MKR_XML_OK || !NS_IS(a, "urn:p") || !NS_IS(plain, "urn:d")) {
            mkr_xml_doc_destroy(fd); return i;
        }
        mkr_xml_doc_destroy(fd);
    }

    return 0;
}
