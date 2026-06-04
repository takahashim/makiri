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
#include "ruby_xpath.h"   /* mkr_xpath_value_to_ruby / mkr_xpath_raise (shared) */
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

/* XPath value -> Ruby and error -> exception are shared with the HTML query glue
 * (mkr_xpath_value_to_ruby / mkr_xpath_raise, ruby_xpath.h): both query entry
 * points run the same engine and return the same public value/error types, so
 * the conversion lives in one place. mkr_node_set_new/push are kind-aware, so the
 * shared value converter wraps these results as Makiri::XML::* nodes. */

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

/* Register a {prefix => uri} Ruby Hash onto +ctx+ for a single query. On any bad
 * entry (non-string-coercible, invalid UTF-8 / embedded NUL, or OOM) the context
 * is freed and an exception is raised — never a partial registration. RSS/Atom
 * live in a default namespace, so a prefix is the strict-mode way to select them
 * (e.g. xpath("//a:entry", "a" => "http://www.w3.org/2005/Atom")). */
static void
mkr_xml_register_query_namespaces(mkr_xpath_context_t *ctx, VALUE rb_ns)
{
    if (NIL_P(rb_ns)) return;
    if (!RB_TYPE_P(rb_ns, T_HASH)) {
        mkr_xpath_context_free(ctx);
        rb_raise(rb_eTypeError, "namespaces must be a Hash of prefix => uri");
    }
    size_t cap = mkr_ctx_limits(ctx)->max_string_bytes;
    VALUE keys = rb_funcall(rb_ns, rb_intern("keys"), 0);
    for (long i = 0; i < RARRAY_LEN(keys); i++) {
        VALUE k  = rb_ary_entry(keys, i);
        VALUE ks = rb_obj_as_string(k);
        VALUE vs = rb_obj_as_string(rb_hash_aref(rb_ns, k));
        mkr_ruby_borrowed_text_t pv, uv;
        const char *bad = mkr_ruby_try_verified_text(ks, cap, &pv);
        if (bad == NULL) bad = mkr_ruby_try_verified_text(vs, cap, &uv);
        if (bad != NULL) {
            mkr_xpath_context_free(ctx);
            rb_raise(mkr_eError, "invalid namespace mapping: %s", bad);
        }
        int rc = mkr_xpath_register_ns(ctx, mkr_verified_text_from_view(pv),
                                       mkr_verified_text_from_view(uv));
        RB_GC_GUARD(ks);
        RB_GC_GUARD(vs);
        if (rc != 0) {
            mkr_xpath_context_free(ctx);
            rb_raise(mkr_eError, "failed to register namespace");
        }
    }
    RB_GC_GUARD(keys);
}

/* Makiri::XML::{Document,*}#xpath(expr, namespaces = nil) / #at_xpath(...):
 * evaluate +expr+ over the XML engine instance, rooted at +self+'s context node,
 * and return a NodeSet (node-set) or scalar. +namespaces+ is an optional
 * {prefix => uri} Hash registered for this query (RSS/Atom default-namespace
 * docs need a prefix under strict matching). Phase 1: no custom-function
 * handler. Makiri::XPathContext is the alternative when many queries share one
 * namespace set (it caches the registrations and the compiled ASTs). */
static VALUE
mkr_xml_doc_xpath_run(VALUE self, VALUE rb_expr, VALUE rb_ns, int first_only)
{
    VALUE document = Qnil;
    mkr_xml_node_t *context = mkr_xml_query_context(self, &document);
    if (context == NULL) {
        return first_only ? Qnil : mkr_node_set_new(document);
    }
    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(document));

    /* The document node is the "document" (the engine's XML namespace services
     * ignore it) and the "/" root for absolute paths; +context+ is the relative
     * context node (the document node for a Document, else the node itself). */
    mkr_xpath_context_t *ctx =
        mkr_xpath_context_new((void *)xdoc->doc_node, (void *)context);
    if (ctx == NULL) {
        rb_raise(mkr_eError, "failed to allocate XPath context");
    }
    mkr_xpath_set_engine_kind(ctx, 1);
    mkr_xml_register_query_namespaces(ctx, rb_ns); /* frees ctx + raises on error */

    /* Mint the borrowed expression view AFTER namespace registration: that step
     * allocates Ruby objects (and may run GC), and the borrowed bytes must not
     * be held live across it. mkr_parse below runs pure C. */
    mkr_ruby_borrowed_text_t ev = mkr_ruby_verified_text(rb_expr, "XPath expression");
    mkr_xpath_error_t error = {0};
    mkr_xpath_limits_t *limits = mkr_ctx_limits(ctx);
    limits->ast_nodes = 0;
    mkr_node_t *ast = mkr_parse(mkr_verified_text_from_view(ev), limits, &error);
    RB_GC_GUARD(ev.value);
    if (ast == NULL) {
        mkr_xpath_context_free(ctx);
        mkr_xpath_raise(&error);
    }

    mkr_xpath_value_t value = {0};
    int rc = first_only ? mkr_xpath_eval_compiled_first(ctx, ast, &value, &error)
                        : mkr_xpath_eval_compiled(ctx, ast, &value, &error);
    mkr_node_free(ast);
    if (rc != 0) {
        mkr_xpath_context_free(ctx);
        mkr_xpath_raise(&error);
    }
    VALUE result = mkr_xpath_value_to_ruby(&value, document); /* converts AND clears value */
    mkr_xpath_context_free(ctx);

    if (first_only && rb_obj_is_kind_of(result, mkr_cNodeSet)) {
        return rb_funcall(result, rb_intern("first"), 0);
    }
    return result;
}

static VALUE
mkr_xml_doc_xpath(int argc, VALUE *argv, VALUE self)
{
    VALUE expr, ns;
    rb_scan_args(argc, argv, "11", &expr, &ns);
    return mkr_xml_doc_xpath_run(self, expr, ns, 0);
}

static VALUE
mkr_xml_doc_at_xpath(int argc, VALUE *argv, VALUE self)
{
    VALUE expr, ns;
    rb_scan_args(argc, argv, "11", &expr, &ns);
    return mkr_xml_doc_xpath_run(self, expr, ns, 1);
}

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
    rb_define_method(mkr_cXmlDocument,   "xpath",    mkr_xml_doc_xpath,    -1);
    rb_define_method(mkr_cXmlDocument,   "at_xpath", mkr_xml_doc_at_xpath, -1);
    rb_define_method(mkr_mXmlNodeMethods, "xpath",    mkr_xml_doc_xpath,    -1);
    rb_define_method(mkr_mXmlNodeMethods, "at_xpath", mkr_xml_doc_at_xpath, -1);

    rb_define_module_function(mkr_mMakiri, "parse_xml", mkr_xml_s_parse, 1);
    /* Makiri::XML(source) — a method named XML on the Makiri module, coexisting
     * with the Makiri::XML constant (the module), as Nokogiri::XML does. */
    rb_define_module_function(mkr_mMakiri, "XML", mkr_xml_s_parse, 1);
}
