/* ruby_xml.c — Ruby boundary for the native XML reader (Phase 1).
 *
 * Makiri::XML(source) / Makiri.parse_xml(source): strict-decode the input
 * (§2.1), then run the Ruby-free parser with the GVL released, and return a
 * Makiri::XML::Document. The document is held in a kind=MKR_DOC_XML mkr_parsed_t
 * (the common document handle, §2.3) and wrapped by mkr_wrap_document, which GC
 * frees via mkr_parsed_destroy (the XML branch whole-arena-frees).
 */
#include "../makiri.h"
#include "../core/mkr_core.h"
#include "../xml/mkr_xml.h"
#include "../xml/mkr_xml_node.h"
#include "glue.h"   /* mkr_wrap_document, mkr_parsed_* (via compat.h) */
#include "../xpath/mkr_xpath.h"
#include "../xpath/mkr_xpath_internal.h"

#include <ruby/thread.h>

/* ---- GVL-released parse ---- */
typedef struct {
    const char      *src;
    size_t           len;
    mkr_xml_doc_t   *result;
    mkr_xml_status_t status;
} mkr_xml_parse_nogvl_t;

static void *
mkr_xml_parse_nogvl(void *p)
{
    mkr_xml_parse_nogvl_t *a = (mkr_xml_parse_nogvl_t *)p;
    a->result = mkr_xml_parse(a->src, a->len, &a->status);
    return NULL;
}

/* call-seq: Makiri.parse_xml(source) -> Makiri::XML::Document
 *           Makiri::XML(source)      -> Makiri::XML::Document */
static VALUE
mkr_xml_s_parse(VALUE self, VALUE rb_source)
{
    (void)self;
    /* Strict decode under the GVL: invalid UTF-8 / undecodable byte / NUL all
     * raise Makiri::XML::SyntaxError here (no U+FFFD repair). */
    VALUE decoded = mkr_xml_decode_input(rb_String(rb_source));

    /* Build an empty XML handle and wrap it first (doc == NULL) so a failure
     * mid-parse frees cleanly via GC (mkr_parsed_destroy -> the XML branch ->
     * mkr_xml_doc_destroy(NULL), a no-op). */
    mkr_parsed_t *parsed = mkr_parsed_new_xml(NULL);
    if (parsed == NULL) {
        rb_raise(mkr_eError, "out of memory allocating XML document");
    }
    VALUE obj = mkr_wrap_document(parsed); /* GC owns +parsed+ from here */

    /* Copy the decoded bytes so the parse can run with the GVL released without
     * racing GC/compaction on the String's backing store. */
    mkr_owned_bytes_t source = {0};
    if (mkr_ruby_copy_bytes(decoded, &source) != 0) {
        rb_raise(mkr_eError, "out of memory copying XML source");
    }
    RB_GC_GUARD(decoded);

    mkr_xml_parse_nogvl_t args = { source.ptr, source.len, NULL, MKR_XML_OK };
    rb_thread_call_without_gvl(mkr_xml_parse_nogvl, &args, NULL, NULL);
    mkr_owned_bytes_clear(&source);

    if (args.result == NULL) {
        switch (args.status) {
        case MKR_XML_ERR_SYNTAX: rb_raise(mkr_eXmlSyntaxError,   "malformed XML"); break;
        case MKR_XML_ERR_LIMIT:  rb_raise(mkr_eXmlLimitExceeded, "XML document budget exceeded"); break;
        default:                 rb_raise(mkr_eError,            "failed to parse XML document"); break;
        }
    }
    mkr_parsed_set_xml_doc(parsed, args.result);
    RB_GC_GUARD(obj);
    return obj;
}

/* Move an evaluated XML XPath value into a Ruby value. A node-set becomes a
 * Makiri::NodeSet over the +document+ (its kind-aware wrap yields Makiri::XML::*
 * nodes); otherwise a String / Float / boolean. The value's owned storage (the
 * node array, the string) is released by the caller via mkr_xpath_value_clear. */
static VALUE
mkr_xml_value_to_ruby(mkr_xpath_value_t *v, VALUE document)
{
    switch (v->type) {
    case MKR_XPATH_TYPE_NODESET: {
        VALUE set = mkr_node_set_new(document);
        for (size_t i = 0; i < v->u.nodeset.count; i++) {
            mkr_node_set_push(set, v->u.nodeset.nodes[i]);
        }
        return set;
    }
    case MKR_XPATH_TYPE_STRING:
        return rb_utf8_str_new(v->u.string.ptr ? v->u.string.ptr : "", (long)v->u.string.len);
    case MKR_XPATH_TYPE_NUMBER:
        return DBL2NUM(v->u.number);
    case MKR_XPATH_TYPE_BOOLEAN:
        return v->u.boolean ? Qtrue : Qfalse;
    }
    return Qnil;
}

NORETURN(static void mkr_xml_raise_xpath(mkr_xpath_error_t *err));
static void
mkr_xml_raise_xpath(mkr_xpath_error_t *err)
{
    VALUE klass = (err->status == MKR_XPATH_ERR_SYNTAX)  ? mkr_eXPathSyntaxError
                : (err->status == MKR_XPATH_ERR_LIMIT)   ? mkr_eXPathLimitExceeded
                : mkr_eError;
    char buf[512];
    snprintf(buf, sizeof(buf), "%s", err->message ? err->message : "XPath error");
    mkr_xpath_error_clear(err);
    rb_raise(klass, "%s", buf);
}

/* Resolve the (document VALUE, context node) an XPath query runs against: for a
 * Makiri::XML::Document the context is the document node, for a node it is that
 * node (and its owning Document). */
static mkr_xml_node_t *
mkr_xml_query_context(VALUE self, VALUE *out_document)
{
    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        *out_document = self;
        mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(self));
        return xdoc ? xdoc->doc_node : NULL;
    }
    mkr_node_data_t *nd;
    TypedData_Get_Struct(self, mkr_node_data_t, &mkr_node_type, nd);
    *out_document = nd->document;
    return (mkr_xml_node_t *)nd->node;
}

/* Makiri::XML::{Document,*}#xpath(expr) / #at_xpath(expr): evaluate +expr+ over
 * the XML engine instance, rooted at +self+'s context node, and return a NodeSet
 * (node-set) or scalar. Phase 1: no custom-function handler and no
 * namespace_matching / per-call namespace registration option yet (Phase 2). */
static VALUE
mkr_xml_doc_xpath_run(VALUE self, VALUE rb_expr, int first_only)
{
    VALUE document = Qnil;
    mkr_xml_node_t *context = mkr_xml_query_context(self, &document);
    if (context == NULL) {
        return first_only ? Qnil : mkr_node_set_new(document);
    }
    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(document));
    mkr_ruby_borrowed_text_t ev = mkr_ruby_verified_text(rb_expr, "XPath expression");

    /* The document node is the "document" (the engine's XML namespace services
     * ignore it) and the "/" root for absolute paths; +context+ is the relative
     * context node (the document node for a Document, else the node itself). */
    mkr_xpath_context_t *ctx =
        mkr_xpath_context_new((void *)xdoc->doc_node, (void *)context);
    if (ctx == NULL) {
        rb_raise(mkr_eError, "failed to allocate XPath context");
    }
    mkr_xpath_set_engine_kind(ctx, 1);

    mkr_xpath_error_t error = {0};
    mkr_xpath_limits_t *limits = mkr_ctx_limits(ctx);
    limits->ast_nodes = 0;
    mkr_node_t *ast = mkr_parse(mkr_verified_text_from_view(ev), limits, &error);
    RB_GC_GUARD(ev.value);
    if (ast == NULL) {
        mkr_xpath_context_free(ctx);
        mkr_xml_raise_xpath(&error);
    }

    mkr_xpath_value_t value = {0};
    int rc = first_only ? mkr_xpath_eval_compiled_first(ctx, ast, &value, &error)
                        : mkr_xpath_eval_compiled(ctx, ast, &value, &error);
    mkr_node_free(ast);
    if (rc != 0) {
        mkr_xpath_context_free(ctx);
        mkr_xml_raise_xpath(&error);
    }
    VALUE result = mkr_xml_value_to_ruby(&value, document);
    mkr_xpath_value_clear(&value);
    mkr_xpath_context_free(ctx);

    if (first_only && rb_obj_is_kind_of(result, mkr_cNodeSet)) {
        return rb_funcall(result, rb_intern("first"), 0);
    }
    return result;
}

static VALUE mkr_xml_doc_xpath(VALUE self, VALUE expr)    { return mkr_xml_doc_xpath_run(self, expr, 0); }
static VALUE mkr_xml_doc_at_xpath(VALUE self, VALUE expr) { return mkr_xml_doc_xpath_run(self, expr, 1); }

/* The document's root element. */
static VALUE
mkr_xml_doc_root(VALUE self)
{
    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(self));
    return (xdoc == NULL) ? Qnil : mkr_wrap_xml_node(xdoc->root, self);
}

void
mkr_init_xml(void)
{
    /* XML::Document is a Makiri::Document leaf (§12): is_a?(Makiri::Document) is
     * true, but it carries no HTML readers (those are on Makiri::HTML, which it
     * does not include) — the read-only XML surface is structural. */
    mkr_cXmlDocument = rb_define_class_under(mkr_mXML, "Document", mkr_cDocument);
    rb_undef_alloc_func(mkr_cXmlDocument); /* created only from C, never .new */
    rb_include_module(mkr_cXmlDocument, mkr_mXmlNodeMethods);

    rb_define_method(mkr_cXmlDocument, "root",     mkr_xml_doc_root,     0);

    /* xpath / at_xpath work on the document and on any XML node (rooted at that
     * node), so they live on the shared XML node behavior module + the document. */
    rb_define_method(mkr_cXmlDocument,   "xpath",    mkr_xml_doc_xpath,    1);
    rb_define_method(mkr_cXmlDocument,   "at_xpath", mkr_xml_doc_at_xpath, 1);
    rb_define_method(mkr_mXmlNodeMethods, "xpath",    mkr_xml_doc_xpath,    1);
    rb_define_method(mkr_mXmlNodeMethods, "at_xpath", mkr_xml_doc_at_xpath, 1);

    rb_define_module_function(mkr_mMakiri, "parse_xml", mkr_xml_s_parse, 1);
    /* Makiri::XML(source) — a method named XML on the Makiri module, coexisting
     * with the Makiri::XML constant (the module), as Nokogiri::XML does. */
    rb_define_module_function(mkr_mMakiri, "XML", mkr_xml_s_parse, 1);
}
