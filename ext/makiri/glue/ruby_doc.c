#include "glue.h"

#include <lexbor/html/parser.h>
#include <ruby/thread.h>
#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ */
/* Document wrapper type                                              */
/* ------------------------------------------------------------------ */

static void
mkr_doc_mark(void *ptr)
{
    mkr_doc_data_t *d = (mkr_doc_data_t *)ptr;
    rb_gc_mark(d->errors);
}

static void
mkr_doc_free(void *ptr)
{
    mkr_doc_data_t *d = (mkr_doc_data_t *)ptr;
    if (d->parsed != NULL) {
        mkr_parsed_destroy(d->parsed);
    }
    xfree(d);
}

static size_t
mkr_doc_memsize(const void *ptr)
{
    /* The DOM arena size is not cheaply queryable; report the wrapper only. */
    (void)ptr;
    return sizeof(mkr_doc_data_t);
}

const rb_data_type_t mkr_doc_type = {
    "Makiri::Document",
    { mkr_doc_mark, mkr_doc_free, mkr_doc_memsize, },
    0, 0, RUBY_TYPED_FREE_IMMEDIATELY,
};

lxb_dom_document_t *
mkr_doc_unwrap(VALUE rb_doc)
{
    mkr_doc_data_t *d;
    TypedData_Get_Struct(rb_doc, mkr_doc_data_t, &mkr_doc_type, d);
    return (lxb_dom_document_t *)d->parsed->doc;
}

mkr_parsed_t *
mkr_doc_parsed(VALUE rb_doc)
{
    mkr_doc_data_t *d;
    TypedData_Get_Struct(rb_doc, mkr_doc_data_t, &mkr_doc_type, d);
    return d->parsed;
}

/* Wrap an owned mkr_parsed_t as a Makiri::Document. GC takes ownership of
 * +parsed+ (freed in dfree). Used to back a standalone DocumentFragment. */
VALUE
mkr_wrap_document(mkr_parsed_t *parsed)
{
    mkr_doc_data_t *d;
    VALUE obj = TypedData_Make_Struct(mkr_cDocument, mkr_doc_data_t, &mkr_doc_type, d);
    d->parsed = parsed;
    d->errors = rb_ary_new();
    return obj;
}

/* ------------------------------------------------------------------ */
/* document fragments                                                 */
/* ------------------------------------------------------------------ */

/* Resolve a fragment-parsing context (the element the HTML fragment is parsed
 * "inside of", per the WHATWG fragment-parsing algorithm) into a tag id +
 * namespace. +context+ matches Nokogiri's `context:`:
 *   - nil          -> <body> in the HTML namespace (the default)
 *   - a Makiri node -> that element's tag id + namespace (the only way to reach
 *                      a foreign non-root context such as SVG <desc>)
 *   - a String      -> an HTML-namespace tag by name, except "svg" / "math"
 *                      which name the foreign roots (as in Nokogiri).
 */
static void
mkr_resolve_fragment_context(lxb_dom_document_t *doc, VALUE context,
                             lxb_tag_id_t *out_tag, lxb_ns_id_t *out_ns)
{
    if (NIL_P(context)) {
        *out_tag = LXB_TAG_BODY;
        *out_ns  = LXB_NS_HTML;
        return;
    }

    if (rb_obj_is_kind_of(context, mkr_cNode)) {
        lxb_dom_node_t *cn = mkr_node_unwrap(context);
        if (cn->type != LXB_DOM_NODE_TYPE_ELEMENT) {
            rb_raise(rb_eArgError, "fragment context node must be an element");
        }
        *out_tag = (lxb_tag_id_t)cn->local_name;
        *out_ns  = (lxb_ns_id_t)cn->ns;
        return;
    }

    VALUE s = rb_String(context);
    /* The context: tag name is a programmatic control string, not parsed HTML,
     * so it follows the strict text-input contract (valid UTF-8, no NUL). */
    mkr_check_text(s, "fragment context element");
    const lxb_char_t *p = (const lxb_char_t *)RSTRING_PTR(s);
    size_t n = (size_t)RSTRING_LEN(s);
    if (n == 3 && memcmp(p, "svg", 3) == 0) {
        *out_tag = LXB_TAG_SVG;  *out_ns = LXB_NS_SVG;  return;
    }
    if (n == 4 && memcmp(p, "math", 4) == 0) {
        *out_tag = LXB_TAG_MATH; *out_ns = LXB_NS_MATH; return;
    }
    lxb_tag_id_t tid = lxb_tag_id_by_name(doc->tags, p, n);
    if (tid == LXB_TAG__UNDEF) {
        rb_raise(rb_eArgError, "unknown fragment context element: %" PRIsVALUE, s);
    }
    *out_tag = tid;
    *out_ns  = LXB_NS_HTML;
}

static bool
mkr_is_html_template(const lxb_dom_node_t *n)
{
    return n->type == LXB_DOM_NODE_TYPE_ELEMENT
        && n->local_name == LXB_TAG_TEMPLATE
        && n->ns == LXB_NS_HTML;
}

/* lxb_dom_document_import_node deep-clones the normal child chain but NOT a
 * <template>'s separate "template contents" fragment, so an imported template
 * comes out with empty content. Walk the source subtree and its freshly-imported
 * clone in lockstep (deep import preserves child order 1:1) and, for every
 * matching <template>, import the source content children into the clone's
 * content fragment. Recurses so templates nested inside template content are
 * fixed too. */
static void
mkr_fixup_template_content(lxb_dom_document_t *doc,
                           lxb_dom_node_t *src, lxb_dom_node_t *clone)
{
    if (mkr_is_html_template(src) && mkr_is_html_template(clone)) {
        lxb_dom_document_fragment_t *sc = lxb_html_interface_template(src)->content;
        lxb_dom_document_fragment_t *cc = lxb_html_interface_template(clone)->content;
        if (sc != NULL && cc != NULL) {
            lxb_dom_node_t *cc_node = lxb_dom_interface_node(cc);
            for (lxb_dom_node_t *s = lxb_dom_interface_node(sc)->first_child;
                 s != NULL; s = s->next) {
                lxb_dom_node_t *imp = lxb_dom_document_import_node(doc, s, true);
                if (imp != NULL) {
                    lxb_dom_node_insert_child(cc_node, imp);
                    mkr_fixup_template_content(doc, s, imp);
                }
            }
        }
    }

    lxb_dom_node_t *s = src->first_child;
    lxb_dom_node_t *c = clone->first_child;
    while (s != NULL && c != NULL) {
        mkr_fixup_template_content(doc, s, c);
        s = s->next;
        c = c->next;
    }
}

/* Parse +rb_html+ as a fragment in the given (tag id, namespace) context and
 * build a DOCUMENT_FRAGMENT node owned by +document+ (so its nodes can be
 * spliced into that document). Lexbor's by-tag-id fragment parser implements
 * the full algorithm for the context (tokenizer state for rawtext/rcdata
 * contexts, foreign-content adjustment, the form pointer, ...). The parsed
 * nodes are deep-imported into the document's arena and attached to the
 * fragment node. Returns the wrapped fragment. */
static VALUE
mkr_build_fragment_ctx(VALUE document, VALUE rb_html,
                       lxb_tag_id_t ctx_tag, lxb_ns_id_t ctx_ns)
{
    VALUE html = rb_String(rb_html);
    lxb_dom_document_t *doc = mkr_doc_unwrap(document);

    lxb_dom_document_fragment_t *frag = lxb_dom_document_fragment_interface_create(doc);
    if (frag == NULL) {
        rb_raise(mkr_eError, "failed to create document fragment");
    }
    lxb_dom_node_t *frag_node = lxb_dom_interface_node(frag);

    lxb_html_parser_t *parser = lxb_html_parser_create();
    if (parser == NULL || lxb_html_parser_init(parser) != LXB_STATUS_OK) {
        if (parser != NULL) lxb_html_parser_destroy(parser);
        rb_raise(mkr_eError, "failed to create fragment parser");
    }

    /* Browser-compatible decoding: invalid UTF-8 -> U+FFFD (valid input is used
     * as-is, no copy). Keeps the fragment DOM valid UTF-8 without rejecting. */
    lxb_char_t *clean = NULL;
    size_t      clean_len = 0;
    if (mkr_utf8_sanitize((const lxb_char_t *)RSTRING_PTR(html),
                          (size_t)RSTRING_LEN(html), &clean, &clean_len) != 0) {
        lxb_html_parser_destroy(parser);
        rb_raise(mkr_eError, "out of memory decoding fragment HTML");
    }
    const lxb_char_t *hsrc = (clean != NULL) ? clean
                                             : (const lxb_char_t *)RSTRING_PTR(html);
    size_t            hlen = (clean != NULL) ? clean_len : (size_t)RSTRING_LEN(html);

    lxb_dom_node_t *root = lxb_html_parse_fragment_by_tag_id(
        parser, (lxb_html_document_t *)doc, ctx_tag, ctx_ns, hsrc, hlen);
    free(clean);
    if (root == NULL) {
        lxb_html_parser_destroy(parser);
        rb_raise(mkr_eError, "failed to parse fragment");
    }

    for (lxb_dom_node_t *f = root->first_child; f != NULL;) {
        lxb_dom_node_t *next = f->next;
        lxb_dom_node_t *imp = lxb_dom_document_import_node(doc, f, true);
        if (imp != NULL) {
            lxb_dom_node_insert_child(frag_node, imp);
            mkr_fixup_template_content(doc, f, imp);
        }
        f = next;
    }

    lxb_html_parser_destroy(parser);
    return mkr_wrap_node(frag_node, document);
}

/* document.fragment(html, context: ...) -> DocumentFragment bound to this
 * document. +context+ defaults to <body>; see mkr_resolve_fragment_context. */
static VALUE
mkr_doc_fragment(int argc, VALUE *argv, VALUE self)
{
    VALUE html, opts;
    rb_scan_args(argc, argv, "1:", &html, &opts);
    VALUE context = NIL_P(opts) ? Qnil
                                : rb_hash_aref(opts, ID2SYM(rb_intern("context")));
    lxb_tag_id_t tag;
    lxb_ns_id_t  ns;
    mkr_resolve_fragment_context(mkr_doc_unwrap(self), context, &tag, &ns);
    return mkr_build_fragment_ctx(self, html, tag, ns);
}

/* DocumentFragment.parse(html, context: ...) -> standalone fragment with its
 * own backing document (kept alive by the fragment's wrapper). */
static VALUE
mkr_frag_s_parse(int argc, VALUE *argv, VALUE klass)
{
    (void)klass;
    VALUE html, opts;
    rb_scan_args(argc, argv, "1:", &html, &opts);
    VALUE context = NIL_P(opts) ? Qnil
                                : rb_hash_aref(opts, ID2SYM(rb_intern("context")));

    static const lxb_char_t shell[] = "<html><body></body></html>";
    mkr_parsed_t *parsed = mkr_parse_html(shell, sizeof(shell) - 1);
    if (parsed == NULL) {
        rb_raise(mkr_eError, "failed to create fragment document");
    }
    VALUE document = mkr_wrap_document(parsed); /* GC now owns parsed */
    lxb_tag_id_t tag;
    lxb_ns_id_t  ns;
    mkr_resolve_fragment_context(mkr_doc_unwrap(document), context, &tag, &ns);
    return mkr_build_fragment_ctx(document, html, tag, ns);
}

/* node.parse(html) -> NodeSet of nodes parsed as a fragment in this element's
 * context (its own tag + namespace). Matches Nokogiri's Node#parse and is the
 * way to reach a foreign (SVG/MathML) fragment context. */
static VALUE
mkr_node_parse(VALUE self, VALUE rb_html)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        rb_raise(rb_eArgError, "Node#parse requires an element context");
    }
    VALUE document = mkr_node_document(self);
    VALUE frag = mkr_build_fragment_ctx(document, rb_html,
                                        (lxb_tag_id_t)node->local_name,
                                        (lxb_ns_id_t)node->ns);
    return rb_funcall(frag, rb_intern("children"), 0);
}

/* ------------------------------------------------------------------ */
/* Document.parse                                                     */
/* ------------------------------------------------------------------ */

/* Arguments for the GVL-released parse. */
typedef struct {
    const lxb_char_t *src;
    size_t            len;
    mkr_parsed_t     *result;
} mkr_parse_nogvl_t;

/* Runs with the GVL released: pure C (Lexbor + libc), touches no Ruby state.
 * mkr_parse_html and all its callees are Ruby-free (verified), so multiple
 * threads can parse concurrently. */
static void *
mkr_parse_nogvl(void *p)
{
    mkr_parse_nogvl_t *a = (mkr_parse_nogvl_t *)p;
    a->result = mkr_parse_html(a->src, a->len);
    return NULL;
}

/*
 * call-seq: _parse(source) -> Document
 *
 * Native entry point. Ruby-level Document.parse coerces +source+ to a String
 * (and reads IO) before calling this. Source locations for Node#line are
 * always tracked.
 */
static VALUE
mkr_doc_s_parse(VALUE klass, VALUE rb_source)
{
    StringValue(rb_source);

    /* Allocate the wrapper first (with parsed == NULL) so that if parsing
     * fails the GC-managed object frees cleanly. */
    mkr_doc_data_t *d;
    VALUE obj = TypedData_Make_Struct(klass, mkr_doc_data_t, &mkr_doc_type, d);
    d->parsed = NULL;
    d->errors = rb_ary_new();

    /* Copy the source into a C buffer so the parse can run with the GVL
     * released without racing GC/compaction on the Ruby String's backing
     * store. The source is not retained past the parse (Lexbor copies what it
     * needs into the arena and the line table is built up front), so the
     * buffer is freed immediately after. */
    long len = RSTRING_LEN(rb_source);
    char *buf = malloc(len > 0 ? (size_t)len : 1);
    if (buf == NULL) {
        rb_raise(mkr_eError, "out of memory copying source");
    }
    if (len > 0) {
        memcpy(buf, RSTRING_PTR(rb_source), (size_t)len);
    }
    RB_GC_GUARD(rb_source);

    mkr_parse_nogvl_t args = { (const lxb_char_t *)buf, (size_t)len, NULL };
    rb_thread_call_without_gvl(mkr_parse_nogvl, &args, NULL, NULL);
    free(buf);

    d->parsed = args.result;
    if (d->parsed == NULL) {
        rb_raise(mkr_eError, "failed to parse HTML document");
    }

    return obj;
}

/* ------------------------------------------------------------------ */
/* Read-only document accessors                                       */
/* ------------------------------------------------------------------ */

/* Get the root element (<html>) of the document, or nil. */
static VALUE
mkr_doc_root(VALUE self)
{
    lxb_dom_document_t *doc = mkr_doc_unwrap(self);
    return mkr_wrap_node(lxb_dom_document_root(doc), self);
}

/* Get the document <title>, or "" if absent. */
static VALUE
mkr_doc_title(VALUE self)
{
    size_t len = 0;
    const lxb_char_t *str =
        lxb_html_document_title((lxb_html_document_t *)mkr_doc_unwrap(self), &len);
    return (str == NULL) ? rb_utf8_str_new("", 0)
                         : rb_utf8_str_new((const char *)str, len);
}

/* The document's DocumentType node (`<!DOCTYPE ...>`), or nil if absent.
 * Mirrors Nokogiri's Document#internal_subset. The doctype is a child of the
 * document node (typically first), so a short scan of the children finds it. */
static VALUE
mkr_doc_internal_subset(VALUE self)
{
    lxb_dom_node_t *doc = (lxb_dom_node_t *)mkr_doc_unwrap(self);
    for (lxb_dom_node_t *c = doc->first_child; c != NULL; c = c->next) {
        if (c->type == LXB_DOM_NODE_TYPE_DOCUMENT_TYPE) {
            return mkr_wrap_node(c, self);
        }
    }
    return Qnil;
}

/* The document's quirks mode as an Integer matching Lexbor's
 * lxb_dom_document_cmode_t (and Gumbo/Nokogiri): 0 = no-quirks, 1 = quirks,
 * 2 = limited-quirks. Set by the parser from the doctype. */
static VALUE
mkr_doc_quirks_mode(VALUE self)
{
    return INT2NUM((int)mkr_doc_unwrap(self)->compat_mode);
}

/* Parse warnings. Reserved; currently always empty. */
static VALUE
mkr_doc_errors(VALUE self)
{
    mkr_doc_data_t *d;
    TypedData_Get_Struct(self, mkr_doc_data_t, &mkr_doc_type, d);
    return d->errors;
}

void
mkr_init_document(void)
{
    rb_define_singleton_method(mkr_cDocument, "_parse", mkr_doc_s_parse, 1);
    rb_define_method(mkr_cDocument, "root",     mkr_doc_root,     0);
    rb_define_method(mkr_cDocument, "title",    mkr_doc_title,    0);
    rb_define_method(mkr_cDocument, "errors",   mkr_doc_errors,   0);
    rb_define_method(mkr_cDocument, "internal_subset", mkr_doc_internal_subset, 0);
    rb_define_method(mkr_cDocument, "quirks_mode", mkr_doc_quirks_mode, 0);
    rb_define_method(mkr_cDocument, "fragment", mkr_doc_fragment, -1);

    rb_define_singleton_method(mkr_cDocumentFragment, "parse", mkr_frag_s_parse, -1);

    /* Node#parse(html): fragment-parse in this element's context (Nokogiri
     * compatible). Defined here, next to the fragment machinery it reuses. */
    rb_define_method(mkr_cNode, "parse", mkr_node_parse, 1);
}
