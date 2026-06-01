#include "glue.h"

#include <lexbor/html/parser.h>

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

/* Parse +rb_html+ as a fragment and build a DOCUMENT_FRAGMENT node owned by
 * +document+ (so its nodes can be spliced into that document). The fragment is
 * parsed in a <body> context; its nodes are deep-imported into the document's
 * arena and attached to the fragment node. Returns the wrapped fragment. */
static VALUE
mkr_build_fragment(VALUE document, VALUE rb_html)
{
    VALUE html = rb_String(rb_html);
    lxb_dom_document_t *doc = mkr_doc_unwrap(document);

    lxb_dom_document_fragment_t *frag = lxb_dom_document_fragment_interface_create(doc);
    if (frag == NULL) {
        rb_raise(mkr_eError, "failed to create document fragment");
    }
    lxb_dom_node_t *frag_node = lxb_dom_interface_node(frag);

    /* Throwaway <body>-context element drives fragment parsing rules. */
    lxb_dom_element_t *ctx =
        lxb_dom_document_create_element(doc, (const lxb_char_t *)"body", 4, NULL);
    lxb_html_parser_t *parser = lxb_html_parser_create();
    if (ctx == NULL || parser == NULL || lxb_html_parser_init(parser) != LXB_STATUS_OK) {
        if (parser != NULL) lxb_html_parser_destroy(parser);
        rb_raise(mkr_eError, "failed to create fragment parser");
    }

    lxb_dom_node_t *root = lxb_html_parse_fragment(
        parser, (lxb_html_element_t *)ctx,
        (const lxb_char_t *)RSTRING_PTR(html), (size_t)RSTRING_LEN(html));
    if (root == NULL) {
        lxb_html_parser_destroy(parser);
        rb_raise(mkr_eError, "failed to parse fragment");
    }

    for (lxb_dom_node_t *f = root->first_child; f != NULL; f = f->next) {
        lxb_dom_node_t *imp = lxb_dom_document_import_node(doc, f, true);
        if (imp != NULL) {
            lxb_dom_node_insert_child(frag_node, imp);
        }
    }

    lxb_html_parser_destroy(parser);
    lxb_dom_node_destroy(lxb_dom_interface_node(ctx));
    return mkr_wrap_node(frag_node, document);
}

/* document.fragment(html) -> DocumentFragment bound to this document. */
static VALUE
mkr_doc_fragment(VALUE self, VALUE rb_html)
{
    return mkr_build_fragment(self, rb_html);
}

/* DocumentFragment.parse(html) -> standalone fragment with its own backing
 * document (kept alive by the fragment's wrapper). */
static VALUE
mkr_frag_s_parse(VALUE klass, VALUE rb_html)
{
    (void)klass;
    static const lxb_char_t shell[] = "<html><body></body></html>";
    mkr_parsed_t *parsed = mkr_parse_html(shell, sizeof(shell) - 1);
    if (parsed == NULL) {
        rb_raise(mkr_eError, "failed to create fragment document");
    }
    VALUE document = mkr_wrap_document(parsed); /* GC now owns parsed */
    return mkr_build_fragment(document, rb_html);
}

/* ------------------------------------------------------------------ */
/* Document.parse                                                     */
/* ------------------------------------------------------------------ */

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

    d->parsed = mkr_parse_html((const lxb_char_t *)RSTRING_PTR(rb_source),
                               (size_t)RSTRING_LEN(rb_source));
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
    rb_define_method(mkr_cDocument, "fragment", mkr_doc_fragment, 1);

    rb_define_singleton_method(mkr_cDocumentFragment, "parse", mkr_frag_s_parse, 1);
}
