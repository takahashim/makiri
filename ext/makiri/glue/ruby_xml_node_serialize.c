/* ruby_xml_node_serialize.c - turning an XML node back into text.
 *
 * #to_xml / #to_s re-emit the subtree as XML 1.0, #canonicalize emits Canonical
 * XML 1.0, and the HTML serializers are refused rather than answered wrongly.
 * Split from ruby_xml_node.c (see ruby_xml_node_internal.h): this half only
 * READS the tree - it shares nothing at all with the mutation and factory half,
 * in either direction.
 */
#include "glue.h"
#include "ruby_xml_node_internal.h"
#include "../xml/mkr_xml.h"        /* MKR_XML_MAX_DEPTH - the serializer honours the reader's nesting cap */
#include "../xml/mkr_xml_node.h"
#include "../core/mkr_core.h"      /* mkr_buf */

#include <ruby/encoding.h>         /* rb_to_encoding / rb_str_encode (output encoding) */
#include <stdlib.h>                /* qsort / free (C14N attribute sort) */
#include <string.h>

/* ---- XML serialization (#to_xml / #to_s) ---------------------------------
 *
 * Re-emit the parsed tree as XML 1.0 text (UTF-8). The default preserves the
 * content as parsed (no pretty-printing); `pretty: true` indents element-only
 * content (an element with any text/CDATA child stays inline, so character data
 * is never altered). Output is always well-formed and re-parses to the same
 * tree. xmlns declarations ride along as ordinary attribute nodes, so namespaces
 * round-trip without special handling. The tree depth is bounded at parse time
 * (MKR_XML_MAX_DEPTH) and the tree is read-only, so the recursive walk cannot
 * exceed that depth. */

#define MKR_XSER_APPEND(b, p, n) \
    do { if (mkr_buf_append((b), (p), (n)) != MKR_OK) return -1; } while (0)
#define MKR_XSER_LIT(b, lit) MKR_XSER_APPEND((b), (lit), sizeof(lit) - 1)


/* Append [s, s+n), escaping &, <, > (and, in an attribute value, " and the
 * whitespace TAB/LF that attribute-value normalization would otherwise fold).
 * CR is always escaped so line-ending normalization cannot alter it on reparse. */
static int
mkr_xser_escaped(mkr_buf_t *b, const char *s, uint32_t n, int attr)
{
    uint32_t start = 0;
    for (uint32_t i = 0; i < n; i++) {
        const char *rep = NULL;
        size_t replen = 0;
        switch (s[i]) {
        case '&': rep = "&amp;"; replen = 5; break;
        case '<': rep = "&lt;";  replen = 4; break;
        case '>': rep = "&gt;";  replen = 4; break;
        case '"':  if (attr) { rep = "&quot;"; replen = 6; } break;
        case '\t': if (attr) { rep = "&#9;";   replen = 4; } break;
        case '\n': if (attr) { rep = "&#10;";  replen = 5; } break;
        case '\r': rep = "&#13;"; replen = 5; break;
        default: break;
        }
        if (rep != NULL) {
            if (i > start) MKR_XSER_APPEND(b, s + start, i - start);
            MKR_XSER_APPEND(b, rep, replen);
            start = i + 1;
        }
    }
    if (n > start) MKR_XSER_APPEND(b, s + start, n - start);
    return 0;
}

/* ---- namespace declarations -----------------------------------------------
 *
 * A node's ns_uri is its identity, not something derived from the declarations
 * around it (mkr_xml_node.h): a parsed tree carries the URIs its declarations
 * gave it, and a node that has since been moved keeps the URI it already had.
 * So the serializer, not the tree, decides which xmlns declarations the output
 * needs - what browsers do, and the reason to_xml still re-parses to the same
 * tree after arbitrary mutation.
 *
 * The in-scope bindings are a chain threaded down the recursion, ONE LINK PER
 * ELEMENT - no per-attribute array, because an element may carry up to
 * MKR_XML_MAX_ATTRS attributes and nest MKR_XML_MAX_DEPTH deep. A link holds the
 * element (its own xmlns attributes are read straight off it) plus the single
 * declaration the serializer may have had to synthesize for the element's own
 * name. A declaration synthesized for a prefixed ATTRIBUTE is emitted but not
 * chained: a descendant that needs the same prefix simply declares it again,
 * which is more verbose than a browser but still correct.
 */

/* An invented prefix is "ns" + up to five digits, so eight bytes hold any of
 * them with room to spare. */
#define MKR_XSER_PREFIX_CAP 8

typedef struct mkr_xser_ns {
    const struct mkr_xser_ns *up;
    const mkr_xml_node_t *el;            /* its xmlns attributes bind at this level */
    const char *syn_prefix; uint32_t syn_plen;   /* the synthesized one, if any */
    const char *syn_uri;    uint32_t syn_ulen;
    int has_syn;
    /* Storage for syn_prefix when it was invented. It lives HERE, in the link
     * the descendants read, rather than in a buffer the element's own later
     * work could overwrite. */
    char syn_buf[MKR_XSER_PREFIX_CAP];
} mkr_xser_ns_t;

/* The declaration for +prefix+ on +el+ itself, or NULL. */
static const mkr_xml_node_t *
mkr_xser_own_decl(const mkr_xml_node_t *el, const char *prefix, uint32_t plen)
{
    for (const mkr_xml_node_t *a = el->attrs; a != NULL; a = a->next) {
        const char *p; uint32_t pl;
        if (!mkr_xml_xmlns_prefix(a->qname, a->qname_len, &p, &pl)) continue;
        if (mkr_bytes_eq(p, pl, prefix, plen)) return a;
    }
    return NULL;
}

/* What +prefix+ means at this point in the walk: the URI, or NULL when it is
 * bound to nothing. The one traversal of the chain - the three questions the
 * serializer actually asks are the three thin wrappers below it. */
static const char *
mkr_xser_lookup(const mkr_xser_ns_t *scope, const char *prefix, uint32_t plen,
                uint32_t *ulen)
{
    for (const mkr_xser_ns_t *s = scope; s != NULL; s = s->up) {
        const mkr_xml_node_t *d = mkr_xser_own_decl(s->el, prefix, plen);
        if (d != NULL) { *ulen = d->value_len; return d->value ? d->value : ""; }
        if (s->has_syn && mkr_bytes_eq(s->syn_prefix, s->syn_plen, prefix, plen)) {
            *ulen = s->syn_ulen; return s->syn_uri ? s->syn_uri : "";
        }
    }
    *ulen = 0;
    return NULL;
}

/* `xml` is bound by the spec, everywhere, and may not be redeclared - so it is
 * never invented and never declared. */
static int
mkr_xser_is_xml_prefix(const char *prefix, uint32_t plen)
{
    return plen == 3 && mkr_bytes_eq(prefix, plen, "xml", 3);
}

/* True when +prefix+ already means exactly [uri,ulen) here. An undeclared
 * DEFAULT means no namespace, so an unprefixed name in no namespace needs no
 * declaration. */
static int
mkr_xser_bound_to(const mkr_xser_ns_t *scope, const char *prefix, uint32_t plen,
                  const char *uri, uint32_t ulen)
{
    if (mkr_xser_is_xml_prefix(prefix, plen)) return 1;
    uint32_t got_len;
    const char *got = mkr_xser_lookup(scope, prefix, plen, &got_len);
    if (got == NULL) return plen == 0 && ulen == 0;
    return mkr_bytes_eq(got, got_len, uri ? uri : "", ulen);
}

/* Emit `xmlns="uri"` / `xmlns:prefix="uri"`. */
static int
mkr_xser_declare(mkr_buf_t *b, const char *prefix, uint32_t plen,
                 const char *uri, uint32_t ulen)
{
    MKR_XSER_LIT(b, " xmlns");
    if (plen > 0) { MKR_XSER_LIT(b, ":"); MKR_XSER_APPEND(b, prefix, plen); }
    MKR_XSER_LIT(b, "=\"");
    if (mkr_xser_escaped(b, uri ? uri : "", ulen, 1) != 0) return -1;
    MKR_XSER_LIT(b, "\"");
    return 0;
}

/* True when +prefix+ stands for anything at all here - so writing it would
 * SHADOW that meaning for the whole subtree. */
static int
mkr_xser_is_bound(const mkr_xser_ns_t *scope, const char *prefix, uint32_t plen)
{
    if (mkr_xser_is_xml_prefix(prefix, plen)) return 1;
    uint32_t ulen;
    return mkr_xser_lookup(scope, prefix, plen, &ulen) != NULL;
}

/* Invents the prefixes one element needs. `seq` never rewinds, so two names on
 * the same element can never be handed the same prefix; `buf` is scratch, and
 * whoever keeps a result past the next call copies it out. */
typedef struct {
    unsigned seq;
    char buf[MKR_XSER_PREFIX_CAP];
} mkr_xser_gen_t;

/* "ns" + the smallest free number. MKR_XSER_PREFIX_CAP holds "ns" + five digits
 * with room for the one this never writes, so the loop below cannot overrun. */
#define MKR_XSER_GEN_MAX 100000u

/* Invent a prefix for a name whose own prefix already means something else here
 * - the one case where the output cannot reuse the name as written. Browsers do
 * the same ("ns1", "ns2", ...); without it the name would serialize under a
 * prefix bound to the wrong URI and stop round-tripping.
 *
 * The result points into +gen+'s scratch and is valid until the next call.
 * Returns 0, or -1 when every candidate is taken. */
static int
mkr_xser_gen_prefix(const mkr_xser_ns_t *scope, mkr_xser_gen_t *gen,
                    const char **out, uint32_t *outlen)
{
    for (; gen->seq < MKR_XSER_GEN_MAX; gen->seq++) {
        size_t i = 0;
        gen->buf[i++] = 'n'; gen->buf[i++] = 's';
        unsigned v = gen->seq, div = 10000u;
        int started = 0;
        while (div > 0) {
            unsigned d = (v / div) % 10u;
            if (d != 0 || started || div == 1) { gen->buf[i++] = (char)('0' + d); started = 1; }
            div /= 10u;
        }
        if (!mkr_xser_is_bound(scope, gen->buf, (uint32_t)i)) {
            *out = gen->buf; *outlen = (uint32_t)i; gen->seq++;
            return 0;
        }
    }
    return -1;
}

/* The first attribute of +el+ before +stop+ that carries +prefix+, or NULL.
 * Reaching this means the prefix is not already bound to what +stop+ needs, so
 * that earlier attribute is the one that declared it - and its URI says whether
 * this one can ride along on that declaration. */
static const mkr_xml_node_t *
mkr_xser_prefix_seen(const mkr_xml_node_t *el, const mkr_xml_node_t *stop,
                     const char *prefix, uint32_t plen)
{
    for (const mkr_xml_node_t *a = el->attrs; a != NULL && a != stop; a = a->next) {
        if (a->prefix_len == 0) continue;
        if (mkr_xml_xmlns_prefix(a->qname, a->qname_len, NULL, NULL)) continue;
        if (mkr_bytes_eq(a->prefix, a->prefix_len, prefix, plen)) return a;
    }
    return NULL;
}

/* How a name is going to be written. +prefix+/+plen+ is the prefix the output
 * uses, which is the node's own unless one had to be invented. */
typedef struct {
    const char *prefix; uint32_t plen;
    int renamed;    /* the prefix was invented, so the local name must be re-joined */
    int declare;    /* a declaration for it must be emitted */
} mkr_xser_plan_t;

/* Plan the ELEMENT's own name.
 *
 * An element may shadow: a declaration it writes applies to itself and its
 * subtree, and its own name is what it is for. So a prefix an ANCESTOR binds
 * differently is still written as the author had it, with a declaration here to
 * override. The one case that cannot work is the element declaring the prefix
 * itself, as something else - then a second, contradictory xmlns would be a
 * duplicate attribute, and a prefix is invented instead. (An ATTRIBUTE may not
 * shadow at all; mkr_xser_plan_attr has the reason.)
 *
 * An invented prefix is copied into +keep+ - the link's own storage - because a
 * descendant reads it long after +gen+'s scratch has been reused.
 *
 * See mkr_xser_plan_attr for why an ATTRIBUTE may not do the same thing. */
static int
mkr_xser_plan_element(const mkr_xser_ns_t *here, const mkr_xml_node_t *n,
                      mkr_xser_gen_t *gen, char *keep, mkr_xser_plan_t *plan)
{
    plan->prefix = n->prefix; plan->plen = n->prefix_len;
    plan->renamed = 0;
    plan->declare = (n->flags & MKR_XML_NODE_FLAG_DOM_LOOSE_NAME) == 0
                    && !mkr_xser_bound_to(here, plan->prefix, plan->plen,
                                          n->ns_uri, n->ns_uri_len);
    if (plan->declare && mkr_xser_own_decl(n, plan->prefix, plan->plen) != NULL) {
        if (mkr_xser_gen_prefix(here, gen, &plan->prefix, &plan->plen) != 0) return -1;
        memcpy(keep, plan->prefix, plan->plen);
        plan->prefix = keep;
        plan->renamed = 1;
    }
    return 0;
}

/* Plan a prefixed ATTRIBUTE's name.
 *
 * Unlike an element, an attribute may NOT shadow: the declaration it would need
 * sits on the element and would rebind the prefix for every descendant, quietly
 * changing what they stand for. So only a prefix bound NOWHERE is written as the
 * author had it; anything already spoken for takes an invented prefix, which is
 * free by construction and shadows nothing. (Browsers do the same.) This is the
 * one place the two planners differ, and mkr_xser_plan_element says so too:
 * change one rule and look at the other.
 *
 *   already means this URI here          -> as-is, no declaration
 *   an earlier attribute declared it so  -> as-is, no declaration
 *   bound to something else, or an
 *     earlier attribute claimed it       -> invent a prefix, declare that
 *   bound to nothing                     -> as-is, declare it
 */
static int
mkr_xser_plan_attr(const mkr_xser_ns_t *here, const mkr_xml_node_t *el,
                   const mkr_xml_node_t *a, mkr_xser_gen_t *gen,
                   mkr_xser_plan_t *plan)
{
    plan->prefix = a->prefix; plan->plen = a->prefix_len;
    plan->renamed = 0;
    plan->declare = 0;

    /* An unprefixed attribute is in no namespace - the default never applies to
     * one - and a declaration declares itself. */
    if (plan->plen == 0 || mkr_xml_xmlns_prefix(a->qname, a->qname_len, NULL, NULL)) return 0;
    if (mkr_xser_bound_to(here, plan->prefix, plan->plen, a->ns_uri, a->ns_uri_len)) return 0;

    const mkr_xml_node_t *prior = mkr_xser_prefix_seen(el, a, plan->prefix, plan->plen);
    int taken = mkr_xser_is_bound(here, plan->prefix, plan->plen);
    if (!taken && prior != NULL
        && mkr_bytes_eq(prior->ns_uri ? prior->ns_uri : "", prior->ns_uri_len,
                        a->ns_uri ? a->ns_uri : "", a->ns_uri_len)) {
        return 0;   /* that earlier attribute already declared exactly this */
    }
    if (taken || prior != NULL) {
        /* No copy: an attribute's invented prefix is never chained, so gen's
         * scratch outlives every use of it. */
        if (mkr_xser_gen_prefix(here, gen, &plan->prefix, &plan->plen) != 0) return -1;
        plan->renamed = 1;
    }
    plan->declare = 1;
    return 0;
}

/* Write +n+'s name: its qualified name verbatim, or - when the serializer had to
 * invent a prefix for it - that prefix with its local name. */
static int
mkr_xser_name(mkr_buf_t *b, const mkr_xml_node_t *n,
              const char *prefix, uint32_t plen, int renamed)
{
    if (!renamed) { MKR_XSER_APPEND(b, n->qname, n->qname_len); return 0; }
    MKR_XSER_APPEND(b, prefix, plen);
    MKR_XSER_LIT(b, ":");
    MKR_XSER_APPEND(b, n->local, n->local_len);
    return 0;
}

static int
mkr_xser_indent(mkr_buf_t *b, int level, int width)
{
    MKR_XSER_LIT(b, "\n");
    for (int i = 0; i < level * width; i++) MKR_XSER_LIT(b, " ");
    return 0;
}

/* True if any child is character data (TEXT/CDATA): such an element is kept
 * inline even in pretty mode, so its text content is preserved exactly. */
static int
mkr_xser_has_chardata(const mkr_xml_node_t *e)
{
    for (const mkr_xml_node_t *c = e->first_child; c != NULL; c = c->next) {
        if (c->type == MKR_XML_NODE_TYPE_TEXT || c->type == MKR_XML_NODE_TYPE_CDATA_SECTION) {
            return 1;
        }
    }
    return 0;
}

static int
mkr_xml_has_dom_loose_name(const mkr_xml_node_t *root)
{
    for (mkr_xml_node_t *cur = (mkr_xml_node_t *)root;
         cur != NULL;
         cur = mkr_xml_preorder_next(root, cur)) {
        if (cur->type == MKR_XML_NODE_TYPE_ELEMENT
            && (cur->flags & MKR_XML_NODE_FLAG_DOM_LOOSE_NAME) != 0) {
            return 1;
        }
    }
    return 0;
}

static int mkr_xser_doctype(mkr_buf_t *b, const mkr_xml_node_t *dt);

/* +depth+ is the ELEMENT nesting this walk is already inside, capped at
 * MKR_XML_MAX_DEPTH - what the READER counts, so exactly the documents it
 * accepts are the ones that serialize. Counting anything else would refuse
 * documents the reader takes: a character, comment or PI node at the deepest
 * element is not another level of nesting, and neither is a fragment (it has no
 * markup of its own). Only the element case tests and advances it.
 *
 * Two reasons, one of which is the serializer's own contract. A tree built with
 * the factories has no depth limit (only parsing does), so a deeper-than-the-cap
 * document serializes to XML that Makiri itself cannot read back - which breaks
 * "output re-parses to the same tree". And the walk is recursive, so a tree deep
 * enough would exhaust the C stack before it ever got there; the cap turns a
 * crash into a clean Makiri::Error. */
static int
mkr_xser_node(mkr_buf_t *b, const mkr_xml_node_t *n, int level, int width,
              const mkr_xser_ns_t *scope, unsigned depth)
{
    switch (n->type) {
    case MKR_XML_NODE_TYPE_DOCUMENT_TYPE:
        return mkr_xser_doctype(b, n);
    case MKR_XML_NODE_TYPE_ELEMENT: {
        if (depth >= MKR_XML_MAX_DEPTH) return -1;

        /* This element's link in the scope chain: its own xmlns attributes bind
         * here, plus at most one declaration synthesized for its own name. The
         * link owns the storage for an invented prefix, so nothing the element
         * does afterwards can move it out from under a descendant. */
        mkr_xser_ns_t here = { scope, n, NULL, 0, NULL, 0, 0, { 0 } };
        mkr_xser_gen_t gen = { 1, { 0 } };   /* shared by the names on this element */

        /* Decide the name before writing anything: the name comes first in the
         * output, but an invented prefix is only known once a declaration is. */
        mkr_xser_plan_t el;
        if (mkr_xser_plan_element(&here, n, &gen, here.syn_buf, &el) != 0) return -1;

        MKR_XSER_LIT(b, "<");
        if (mkr_xser_name(b, n, el.prefix, el.plen, el.renamed) != 0) return -1;
        if (el.declare) {
            if (mkr_xser_declare(b, el.prefix, el.plen, n->ns_uri, n->ns_uri_len) != 0) return -1;
            here.syn_prefix = el.prefix; here.syn_plen = el.plen;
            here.syn_uri = n->ns_uri;    here.syn_ulen = n->ns_uri_len;
            here.has_syn = 1;
        }

        /* Each attribute, preceded by the declaration it needs - the order a
         * browser emits. An invented attribute prefix is never chained, so its
         * buffer only has to live for this iteration. */
        for (const mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
            mkr_xser_plan_t at;
            if (mkr_xser_plan_attr(&here, n, a, &gen, &at) != 0) return -1;
            if (at.declare
                && mkr_xser_declare(b, at.prefix, at.plen, a->ns_uri, a->ns_uri_len) != 0) return -1;
            MKR_XSER_LIT(b, " ");
            if (mkr_xser_name(b, a, at.prefix, at.plen, at.renamed) != 0) return -1;
            MKR_XSER_LIT(b, "=\"");
            if (mkr_xser_escaped(b, a->value ? a->value : "", a->value_len, 1) != 0) return -1;
            MKR_XSER_LIT(b, "\"");
        }
        if (n->first_child == NULL) { MKR_XSER_LIT(b, "/>"); return 0; }
        MKR_XSER_LIT(b, ">");
        int block = width > 0 && !mkr_xser_has_chardata(n);
        for (const mkr_xml_node_t *c = n->first_child; c != NULL; c = c->next) {
            if (block && mkr_xser_indent(b, level + 1, width) != 0) return -1;
            if (mkr_xser_node(b, c, level + 1, width, &here, depth + 1) != 0) return -1;
        }
        if (block && mkr_xser_indent(b, level, width) != 0) return -1;
        MKR_XSER_LIT(b, "</");
        if (mkr_xser_name(b, n, el.prefix, el.plen, el.renamed) != 0) return -1;
        MKR_XSER_LIT(b, ">");
        return 0;
    }
    case MKR_XML_NODE_TYPE_TEXT:
        return mkr_xser_escaped(b, n->value ? n->value : "", n->value_len, 0);
    case MKR_XML_NODE_TYPE_CDATA_SECTION:
        MKR_XSER_LIT(b, "<![CDATA[");
        MKR_XSER_APPEND(b, n->value ? n->value : "", n->value_len);
        MKR_XSER_LIT(b, "]]>");
        return 0;
    case MKR_XML_NODE_TYPE_COMMENT:
        MKR_XSER_LIT(b, "<!--");
        MKR_XSER_APPEND(b, n->value ? n->value : "", n->value_len);
        MKR_XSER_LIT(b, "-->");
        return 0;
    case MKR_XML_NODE_TYPE_PI:
        MKR_XSER_LIT(b, "<?");
        MKR_XSER_APPEND(b, n->local, n->local_len);
        if (n->value_len) { MKR_XSER_LIT(b, " "); MKR_XSER_APPEND(b, n->value, n->value_len); }
        MKR_XSER_LIT(b, "?>");
        return 0;
    case MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT:
        /* A fragment has no markup of its own: it serializes as its children, in
         * order, spliced together (the same nodes #add_child would insert). */
        for (const mkr_xml_node_t *c = n->first_child; c != NULL; c = c->next) {
            if (mkr_xser_node(b, c, level, width, scope, depth) != 0) return -1;
        }
        return 0;
    default:
        return 0;
    }
}

/* <!DOCTYPE name [PUBLIC "pub" "sys" | SYSTEM "sys"]> from the off-tree node. */
static int
mkr_xser_doctype(mkr_buf_t *b, const mkr_xml_node_t *dt)
{
    MKR_XSER_LIT(b, "<!DOCTYPE ");
    MKR_XSER_APPEND(b, dt->local, dt->local_len);
    if (dt->prefix != NULL) {                 /* PUBLIC id present -> "PUBLIC pub sys" */
        MKR_XSER_LIT(b, " PUBLIC \"");
        MKR_XSER_APPEND(b, dt->prefix, dt->prefix_len);
        MKR_XSER_LIT(b, "\" \"");
        MKR_XSER_APPEND(b, dt->value ? dt->value : "", dt->value_len);
        MKR_XSER_LIT(b, "\"");
    } else if (dt->value != NULL) {           /* SYSTEM id only */
        MKR_XSER_LIT(b, " SYSTEM \"");
        MKR_XSER_APPEND(b, dt->value, dt->value_len);
        MKR_XSER_LIT(b, "\"");
    }
    MKR_XSER_LIT(b, ">");
    return 0;
}

/* An upper bound for a serialization buffer, scaled to the document's content
 * (its tracked arena_bytes). The serialized form of any acyclic, depth-bounded
 * document is a small multiple of its arena bytes, so 32x - which covers
 * worst-case escaping and maximal pretty-print indentation for a parsed document
 * (depth <= MKR_XML_MAX_DEPTH) - admits every legitimate serialization, with a
 * 64 KiB floor for tiny documents. A cyclic or pathologically deep CONSTRUCTED
 * tree exceeds the bound, so the serializer fails closed with MKR_ERR_LIMIT
 * (-> Makiri::Error) instead of growing the buffer without limit and exhausting
 * memory. Defence-in-depth: the tree-mutation guards already prevent cycles. */
static size_t
mkr_xml_serialize_cap(VALUE self)
{
    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(mkr_xml_node_document(self)));
    size_t cap = 65536;   /* floor: declaration/DOCTYPE + a small subtree */
    size_t arena = xdoc ? xdoc->arena_bytes : 0;
    if (arena > 0) {
        cap = (arena <= (SIZE_MAX - cap) / 32) ? cap + arena * 32 : SIZE_MAX;
    }
    return cap;
}

/* call-seq: node.to_xml(pretty: false, indent: 2, encoding: "UTF-8") -> String
 * A Document also emits the XML declaration and its DOCTYPE; any other node
 * serializes just its own subtree. +encoding+ (a String or Encoding) transcodes
 * the output - a character the target cannot represent becomes a hexadecimal
 * character reference - and is named in a Document's declaration. */
static VALUE
mkr_xml_node_to_xml(int argc, VALUE *argv, VALUE self)
{
    VALUE opts;
    rb_scan_args(argc, argv, "0:", &opts);
    int width = 0;
    VALUE enc_opt = Qnil;
    if (!NIL_P(opts)) {
        if (RTEST(rb_hash_aref(opts, ID2SYM(rb_intern("pretty"))))) width = 2;
        VALUE iv = rb_hash_aref(opts, ID2SYM(rb_intern("indent")));
        if (!NIL_P(iv)) width = NUM2INT(iv) < 0 ? 0 : NUM2INT(iv);
        enc_opt = rb_hash_aref(opts, ID2SYM(rb_intern("encoding")));
    }
    /* resolve the target encoding (raises on an unknown name) + its declared name */
    rb_encoding *to_enc = NIL_P(enc_opt) ? NULL : rb_to_encoding(enc_opt);
    VALUE enc_name = NIL_P(enc_opt) ? Qnil : rb_obj_as_string(enc_opt);

    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if (mkr_xml_has_dom_loose_name(n)) {
        rb_raise(mkr_eError,
                 "cannot serialize XML containing a DOM-loose element name");
    }
    mkr_buf_t buf;
    mkr_buf_init(&buf, mkr_xml_serialize_cap(self));   /* fail closed past the cap, never OOM */
    int rc = 0;

    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(self));
        /* Emit the encoding pseudo-attribute only when an explicit encoding: was
         * requested or the parsed source declared one; otherwise a built or
         * declaration-less document round-trips to a bare `<?xml version="1.0"?>`,
         * matching Nokogiri (the output is UTF-8 either way). */
        int emit_enc = !NIL_P(enc_name) || (xdoc != NULL && xdoc->has_encoding_decl);
        if (emit_enc) {
            static const char decl_a[] = "<?xml version=\"1.0\" encoding=\"";
            static const char decl_b[] = "\"?>\n";
            VALUE name = NIL_P(enc_name) ? rb_utf8_str_new_cstr("UTF-8") : enc_name;
            mkr_ruby_borrowed_bytes_t nv = mkr_ruby_bytes_view(name);
            rc = (mkr_buf_append(&buf, decl_a, sizeof(decl_a) - 1) == MKR_OK) ? 0 : -1;
            if (rc == 0) rc = (mkr_buf_append(&buf, nv.ptr, nv.len) == MKR_OK) ? 0 : -1;
            if (rc == 0) rc = (mkr_buf_append(&buf, decl_b, sizeof(decl_b) - 1) == MKR_OK) ? 0 : -1;
            RB_GC_GUARD(nv.value);
        } else {
            static const char decl[] = "<?xml version=\"1.0\"?>\n";
            rc = (mkr_buf_append(&buf, decl, sizeof(decl) - 1) == MKR_OK) ? 0 : -1;
        }
        /* The DOCTYPE is a document-node child (linked before the root), so the
         * child walk below serializes it in place - no separate emit. */
        for (mkr_xml_node_t *c = n->first_child; rc == 0 && c != NULL; c = c->next) {
            rc = mkr_xser_node(&buf, c, 0, width, NULL, 0);
            if (rc == 0) rc = (mkr_buf_append(&buf, "\n", 1) == MKR_OK) ? 0 : -1;
        }
    } else {
        rc = mkr_xser_node(&buf, n, 0, width, NULL, 0);
    }

    if (rc != 0) {
        mkr_buf_free(&buf);
        rb_raise(mkr_eError, "failed to serialize XML: output exceeded the size limit or out of memory");
    }
    VALUE str = rb_utf8_str_new(buf.len ? buf.data : "", (long)buf.len);
    mkr_buf_free(&buf);

    /* Transcode to the requested encoding; an unrepresentable character becomes a
     * &#xNN; reference (ECONV_UNDEF_HEX_CHARREF) rather than raising or dropping. */
    if (to_enc != NULL && to_enc != rb_utf8_encoding() && to_enc != rb_usascii_encoding()) {
        str = rb_str_encode(str, rb_enc_from_encoding(to_enc), ECONV_UNDEF_HEX_CHARREF, Qnil);
    }
    return str;
}

/* ---- Canonical XML 1.0 (#canonicalize) -----------------------------------
 *
 * Inclusive Canonical XML 1.0 (https://www.w3.org/TR/xml-c14n), the form used
 * for XML signatures: UTF-8 output, explicit start/end tags (no `<a/>`),
 * attributes sorted by (namespace-uri, local-name), namespace declarations
 * sorted by prefix with superfluous ones removed, CDATA emitted as escaped text,
 * comments omitted unless requested. Exclusive C14N is not implemented. */

/* C14N escaping - text: & < > #xD ; attribute value: & < " #x9 #xA #xD. */
static int
mkr_c14n_escaped(mkr_buf_t *b, const char *s, uint32_t n, int attr)
{
    uint32_t start = 0;
    for (uint32_t i = 0; i < n; i++) {
        const char *rep = NULL;
        size_t replen = 0;
        switch ((unsigned char)s[i]) {
        case '&':  rep = "&amp;"; replen = 5; break;
        case '<':  rep = "&lt;";  replen = 4; break;
        case '>':  if (!attr) { rep = "&gt;";  replen = 4; } break;
        case '"':  if (attr)  { rep = "&quot;"; replen = 6; } break;
        case 0x09: if (attr)  { rep = "&#x9;";  replen = 5; } break;
        case 0x0A: if (attr)  { rep = "&#xA;";  replen = 5; } break;
        case 0x0D: rep = "&#xD;"; replen = 5; break;
        default: break;
        }
        if (rep != NULL) {
            if (i > start) MKR_XSER_APPEND(b, s + start, i - start);
            MKR_XSER_APPEND(b, rep, replen);
            start = i + 1;
        }
    }
    if (n > start) MKR_XSER_APPEND(b, s + start, n - start);
    return 0;
}

/* slice compare (lexicographic; shorter sorts first on a shared prefix). */
static int
mkr_slice_cmp(const char *a, uint32_t al, const char *bb, uint32_t bl)
{
    uint32_t m = al < bl ? al : bl;
    int c = m ? memcmp(a ? a : "", bb ? bb : "", m) : 0;
    if (c != 0) return c;
    return al == bl ? 0 : (al < bl ? -1 : 1);
}

static int
mkr_c14n_attr_cmp(const void *pa, const void *pb)   /* by (namespace-uri, local) */
{
    const mkr_xml_node_t *a = *(const mkr_xml_node_t *const *)pa;
    const mkr_xml_node_t *b = *(const mkr_xml_node_t *const *)pb;
    int c = mkr_slice_cmp(a->ns_uri, a->ns_uri_len, b->ns_uri, b->ns_uri_len);
    return c != 0 ? c : mkr_slice_cmp(a->local, a->local_len, b->local, b->local_len);
}

/* A renderable namespace binding. All pointers are arena slices (stable for the
 * document's lifetime, GC-irrelevant): the C14N walk never builds Ruby objects. */
typedef struct { const char *prefix; uint32_t plen; const char *uri; uint32_t ulen; } mkr_c14n_ns_t;

static int
mkr_c14n_ns_cmp(const void *pa, const void *pb)     /* by prefix (default "" first) */
{
    const mkr_c14n_ns_t *a = pa, *b = pb;
    return mkr_slice_cmp(a->prefix, a->plen, b->prefix, b->plen);
}


/* Nearest in-scope binding for +prefix+ at or above +node+ (walks the real tree;
 * no scope dictionary is threaded). */
static int
mkr_c14n_nearest(const mkr_xml_node_t *node, const char *prefix, uint32_t plen,
                 const char **uri, uint32_t *ulen)
{
    for (const mkr_xml_node_t *e = node; e != NULL; e = e->parent) {
        if (e->type != MKR_XML_NODE_TYPE_ELEMENT) continue;
        for (mkr_xml_node_t *a = e->attrs; a != NULL; a = a->next) {
            const char *p, *u; uint32_t pl, ul;
            if (mkr_xml_node_xmlns_decl(a, &p, &pl, &u, &ul)
                && mkr_bytes_eq(p, pl, prefix, plen)) {
                *uri = u; *ulen = ul; return 1;
            }
        }
    }
    return 0;
}

static int
mkr_c14n_has_prefix(const mkr_c14n_ns_t *arr, size_t n, const char *p, uint32_t pl)
{
    for (size_t i = 0; i < n; i++)
        if (mkr_bytes_eq(arr[i].prefix, arr[i].plen, p, pl)) return 1;
    return 0;
}

/* The namespace declarations to render at +n+ (Inclusive C14N 1.0). The apex
 * renders every in-scope namespace (walking ancestors, nearest binding winning);
 * a descendant renders only its OWN xmlns declarations that change the inherited
 * binding. Writes a heap array (the caller frees) sorted by prefix; returns the
 * count, or SIZE_MAX on OOM. */
static size_t
mkr_c14n_namespaces(const mkr_xml_node_t *n, int is_apex, mkr_c14n_ns_t **out)
{
    *out = NULL;
    size_t cap = 0;
    for (const mkr_xml_node_t *e = n; e != NULL; e = e->parent) {
        if (e->type == MKR_XML_NODE_TYPE_ELEMENT) {
            for (mkr_xml_node_t *a = e->attrs; a != NULL; a = a->next) {
                const char *p, *u; uint32_t pl, ul;
                if (mkr_xml_node_xmlns_decl(a, &p, &pl, &u, &ul)) cap++;
            }
        }
        if (!is_apex) break;   /* a descendant considers only its own declarations */
    }
    if (cap == 0) return 0;
    mkr_c14n_ns_t *arr = mkr_reallocarray(NULL, cap, sizeof(*arr));
    if (arr == NULL) return SIZE_MAX;
    size_t cnt = 0;
    int default_seen = 0;
    for (const mkr_xml_node_t *e = n; e != NULL; e = e->parent) {
        if (e->type == MKR_XML_NODE_TYPE_ELEMENT) {
            for (mkr_xml_node_t *a = e->attrs; a != NULL; a = a->next) {
                const char *p, *u; uint32_t pl, ul;
                if (!mkr_xml_node_xmlns_decl(a, &p, &pl, &u, &ul)) continue;
                if (mkr_bytes_eq(p, pl, "xml", 3)) continue;                   /* implicit xml: */
                if (is_apex) {
                    if (pl == 0) {                /* default: nearest decl wins */
                        if (default_seen) continue;
                        default_seen = 1;
                        if (ul == 0) continue;    /* nearest default is xmlns="" -> not rendered */
                    } else if (mkr_c14n_has_prefix(arr, cnt, p, pl)) {
                        continue;
                    }
                } else {                          /* descendant: only changes to the binding */
                    const char *au; uint32_t aul;
                    int above = mkr_c14n_nearest(e->parent, p, pl, &au, &aul);
                    if (above && mkr_bytes_eq(au, aul, u, ul)) continue; /* superfluous */
                    if (pl == 0 && ul == 0 && !(above && aul > 0)) continue; /* xmlns="" only undeclares */
                }
                arr[cnt].prefix = p; arr[cnt].plen = pl;
                arr[cnt].uri = u;    arr[cnt].ulen = ul;
                cnt++;
            }
        }
        if (!is_apex) break;
    }
    qsort(arr, cnt, sizeof(*arr), mkr_c14n_ns_cmp);
    *out = arr;
    return cnt;
}

static int
mkr_c14n_node(mkr_buf_t *b, const mkr_xml_node_t *n, int is_apex, int comments,
              unsigned depth)
{
    switch (n->type) {
    case MKR_XML_NODE_TYPE_ELEMENT: {
        /* element nesting only, like the reader - see mkr_xser_node */
        if (depth >= MKR_XML_MAX_DEPTH) return -1;
        MKR_XSER_LIT(b, "<");
        MKR_XSER_APPEND(b, n->qname, n->qname_len);

        /* namespace declarations, prefix-sorted (a heap array of arena slices;
         * freed before returning, so no buffer macro may early-return past it). */
        mkr_c14n_ns_t *ns = NULL;
        size_t nn = mkr_c14n_namespaces(n, is_apex, &ns);
        if (nn == (size_t)-1) return -1;
        for (size_t i = 0; i < nn; i++) {
            int ok;
            if (ns[i].plen == 0) {
                ok = mkr_buf_append(b, " xmlns=\"", 8) == MKR_OK;
            } else {
                ok = mkr_buf_append(b, " xmlns:", 7) == MKR_OK
                     && mkr_buf_append(b, ns[i].prefix, ns[i].plen) == MKR_OK
                     && mkr_buf_append(b, "=\"", 2) == MKR_OK;
            }
            if (ok) ok = mkr_c14n_escaped(b, ns[i].uri, ns[i].ulen, 1) == 0;
            if (ok) ok = mkr_buf_append(b, "\"", 1) == MKR_OK;
            if (!ok) { free(ns); return -1; }
        }
        free(ns);

        /* attributes (non-xmlns) sorted by (namespace-uri, local-name) */
        size_t na = 0;
        for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
            const char *p, *u; uint32_t pl, ul;
            if (!mkr_xml_node_xmlns_decl(a, &p, &pl, &u, &ul)) na++;
        }
        if (na > 0) {
            const mkr_xml_node_t **av = mkr_reallocarray(NULL, na, sizeof(*av));
            if (av == NULL) return -1;
            size_t k = 0;
            for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
                const char *p, *u; uint32_t pl, ul;
                if (!mkr_xml_node_xmlns_decl(a, &p, &pl, &u, &ul)) av[k++] = a;
            }
            qsort(av, na, sizeof(*av), mkr_c14n_attr_cmp);
            for (size_t i = 0; i < na; i++) {
                int ok = mkr_buf_append(b, " ", 1) == MKR_OK
                         && mkr_buf_append(b, av[i]->qname, av[i]->qname_len) == MKR_OK
                         && mkr_buf_append(b, "=\"", 2) == MKR_OK;
                if (ok) ok = mkr_c14n_escaped(b, av[i]->value ? av[i]->value : "", av[i]->value_len, 1) == 0;
                if (ok) ok = mkr_buf_append(b, "\"", 1) == MKR_OK;
                if (!ok) { free(av); return -1; }
            }
            free(av);
        }

        MKR_XSER_LIT(b, ">");
        for (mkr_xml_node_t *c = n->first_child; c != NULL; c = c->next) {
            if (mkr_c14n_node(b, c, 0, comments, depth + 1) != 0) return -1;  /* children are not the apex */
        }
        MKR_XSER_LIT(b, "</");
        MKR_XSER_APPEND(b, n->qname, n->qname_len);
        MKR_XSER_LIT(b, ">");
        return 0;
    }
    case MKR_XML_NODE_TYPE_TEXT:
    case MKR_XML_NODE_TYPE_CDATA_SECTION:    /* CDATA canonicalizes to escaped text */
        return mkr_c14n_escaped(b, n->value ? n->value : "", n->value_len, 0);
    case MKR_XML_NODE_TYPE_COMMENT:
        if (comments) {
            MKR_XSER_LIT(b, "<!--");
            MKR_XSER_APPEND(b, n->value ? n->value : "", n->value_len);
            MKR_XSER_LIT(b, "-->");
        }
        return 0;
    case MKR_XML_NODE_TYPE_PI:
        MKR_XSER_LIT(b, "<?");
        MKR_XSER_APPEND(b, n->local, n->local_len);
        if (n->value_len) { MKR_XSER_LIT(b, " "); MKR_XSER_APPEND(b, n->value, n->value_len); }
        MKR_XSER_LIT(b, "?>");
        return 0;
    case MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT:
        for (const mkr_xml_node_t *c = n->first_child; c != NULL; c = c->next) {
            if (mkr_c14n_node(b, c, 0, comments, depth) != 0) return -1;       /* a fragment is not a level */
        }
        return 0;
    default:
        return 0;
    }
}

/* call-seq: node.canonicalize(comments: false) -> String
 * Inclusive Canonical XML 1.0 (UTF-8). A Document canonicalizes its element +
 * top-level PIs (and comments when requested); any other node canonicalizes its
 * subtree, with the apex inheriting the ancestors' in-scope namespaces. */
static VALUE
mkr_xml_node_canonicalize(int argc, VALUE *argv, VALUE self)
{
    VALUE opts;
    rb_scan_args(argc, argv, "0:", &opts);
    int comments = !NIL_P(opts) && RTEST(rb_hash_aref(opts, ID2SYM(rb_intern("comments"))));

    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if (mkr_xml_has_dom_loose_name(n)) {
        rb_raise(mkr_eError,
                 "cannot canonicalize XML containing a DOM-loose element name");
    }
    mkr_buf_t buf;
    mkr_buf_init(&buf, mkr_xml_serialize_cap(self));   /* fail closed past the cap, never OOM */
    int rc = 0;

    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        /* §2.4: a PI/comment before the document element is followed by #xA, one
         * after is preceded by #xA; the element itself has no surrounding line. */
        int seen_root = 0;
        for (mkr_xml_node_t *c = n->first_child; rc == 0 && c != NULL; c = c->next) {
            if (c->type == MKR_XML_NODE_TYPE_ELEMENT) {
                rc = mkr_c14n_node(&buf, c, 1, comments, 0);   /* the root element is the apex */
                seen_root = 1;
            } else if (c->type == MKR_XML_NODE_TYPE_PI ||
                       (c->type == MKR_XML_NODE_TYPE_COMMENT && comments)) {
                if (seen_root && rc == 0) rc = (mkr_buf_append(&buf, "\n", 1) == MKR_OK) ? 0 : -1;
                if (rc == 0) rc = mkr_c14n_node(&buf, c, 0, comments, 0);
                if (!seen_root && rc == 0) rc = (mkr_buf_append(&buf, "\n", 1) == MKR_OK) ? 0 : -1;
            }
        }
    } else {
        rc = mkr_c14n_node(&buf, n, 1, comments, 0);  /* the node is the apex (inherits ancestors' ns) */
    }

    if (rc != 0) {
        mkr_buf_free(&buf);
        rb_raise(mkr_eError, "failed to canonicalize XML: output exceeded the size limit or out of memory");
    }
    VALUE str = rb_utf8_str_new(buf.len ? buf.data : "", (long)buf.len);
    mkr_buf_free(&buf);
    return str;
}

/* ---- fail-closed guard for the unsupported serialization surface ----
 *
 * HTML serialization (to_html/inner_html/outer_html) would silently misbehave on
 * XML (escaping / CDATA / void elements differ), so it is an explicit
 * NotImplementedError rather than a wrong result. Use #to_xml. (CSS selectors,
 * once unsupported here, are now lowered to the native XPath engine - see
 * ruby_xml.c.) */
static VALUE
mkr_xml_node_no_serialize(int argc, VALUE *argv, VALUE self)
{
    (void)argc; (void)argv; (void)self;
    rb_raise(rb_eNotImpError,
             "Makiri::XML does not HTML-serialize (to_html / inner_html / "
             "outer_html); use #to_xml for XML output.");
}

void
mkr_init_xml_node_serialize(void)
{
    /* XML serialization: #to_xml / #to_s re-emit the subtree (a Document also
     * emits the declaration + DOCTYPE). Also defined on XML::Document below. */
    rb_define_method(mkr_mXmlNodeMethods, "to_xml",     mkr_xml_node_to_xml, -1);
    rb_define_method(mkr_mXmlNodeMethods, "to_s",       mkr_xml_node_to_xml, -1);
    rb_define_method(mkr_mXmlNodeMethods, "canonicalize", mkr_xml_node_canonicalize, -1);

    /* CSS selectors (#css / #at_css / #matches?) are supported on XML via the
     * native XPath engine - registered in ruby_xml.c. Fail-closed: HTML
     * serialization is unsupported on XML; raise rather than emit wrong markup. */
    rb_define_method(mkr_mXmlNodeMethods, "to_html",    mkr_xml_node_no_serialize, -1);
    rb_define_method(mkr_mXmlNodeMethods, "inner_html", mkr_xml_node_no_serialize, -1);
    rb_define_method(mkr_mXmlNodeMethods, "outer_html", mkr_xml_node_no_serialize, -1);
}
