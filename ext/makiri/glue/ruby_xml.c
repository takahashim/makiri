/* ruby_xml.c - Ruby boundary for the native XML reader (Phase 1).
 *
 * Makiri::XML::Document.parse(source) (and the Makiri::XML(source) convenience
 * that delegates to it): strict-decode the input (§2.1), then run the Ruby-free
 * parser with the GVL released, and return a
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
#include "../xpath/mkr_css.h"   /* mkr_css_compile - CSS selectors over XML via the XPath engine */
#include "../xml/mkr_xml_index.h"   /* element-name index for the //name fast path */

#include <ruby/thread.h>

/* void-typed adapters so the engine's representation-neutral name-index hooks
 * can reach the XML element-name index (mkr_xml_index.c) without the engine
 * knowing its concrete types. */
static void *
mkr_xml_name_index_get_v(void *owner)
{
    return mkr_xml_name_index_get((mkr_xml_doc_t *)owner);
}

static void *const *
mkr_xml_name_index_lookup_v(const void *idx, const char *local, size_t local_len,
                            const char *ns_uri, size_t ns_uri_len, size_t *count)
{
    return (void *const *)mkr_xml_name_index_lookup(
        (const mkr_xml_name_index_t *)idx, local, local_len, ns_uri, ns_uri_len, count);
}

/* Install the document's element-name index hooks on +ctx+ (the engine lazily
 * builds it only when a document-rooted descendant name test runs). */
static void
mkr_xml_install_name_index(mkr_xpath_context_t *ctx, mkr_xml_doc_t *xdoc)
{
    mkr_xpath_context_set_name_index(ctx, (void *)xdoc,
                                     mkr_xml_name_index_get_v,
                                     mkr_xml_name_index_lookup_v);
}

/* ---- GVL-released parse ---- */
typedef struct {
    const char      *src;
    size_t           len;
    mkr_xml_limits_t limits;     /* per-parse budget overrides (0 fields = default) */
    mkr_xml_doc_t   *result;
    mkr_xml_status_t status;
} mkr_xml_parse_nogvl_t;

static void *
mkr_xml_parse_nogvl(void *p)
{
    mkr_xml_parse_nogvl_t *a = (mkr_xml_parse_nogvl_t *)p;
    a->result = mkr_xml_parse_ex(a->src, a->len, &a->limits, &a->status);
    return NULL;
}

/* Read the optional per-parse budget overrides from the keyword hash. Today only
 * max_bytes (the arena memory ceiling) is configurable; rb_get_kwargs rejects any
 * other keyword, and new budgets join the table here as they become runtime-
 * configurable. max_bytes must be a positive Integer (0 / negative / non-Integer
 * raise), and 0 in the struct means "use the compile-time default". */
static mkr_xml_limits_t
mkr_xml_parse_limits(VALUE rb_opts)
{
    mkr_xml_limits_t limits = { 0 };
    if (NIL_P(rb_opts)) return limits;

    static ID kw_ids[1];
    if (kw_ids[0] == 0) kw_ids[0] = rb_intern_const("max_bytes");
    VALUE kw_vals[1];
    rb_get_kwargs(rb_opts, kw_ids, 0, 1, kw_vals);  /* unknown keyword -> ArgumentError */

    if (kw_vals[0] != Qundef) {
        if (!RB_INTEGER_TYPE_P(kw_vals[0])) {
            rb_raise(rb_eTypeError, "max_bytes must be an Integer");
        }
        /* Reject <= 0 BEFORE the unsigned conversion: NUM2SIZET wraps a negative
         * Integer into a huge size_t (an accidental budget bypass), so guard the
         * sign first. A too-large positive still raises RangeError in NUM2SIZET. */
        if (RTEST(rb_funcall(kw_vals[0], rb_intern("<="), 1, INT2FIX(0)))) {
            rb_raise(rb_eArgError, "max_bytes must be positive");
        }
        limits.max_bytes = NUM2SIZET(kw_vals[0]);
    }
    return limits;
}

/* call-seq: Makiri::XML::Document.parse(source, max_bytes: nil) -> Makiri::XML::Document
 *           Makiri::XML(source, max_bytes: nil)              -> Makiri::XML::Document
 * +source+ is a String or any object responding to +#read+ (an IO / File /
 * StringIO); +max_bytes+ overrides the default arena memory ceiling for this
 * parse. Read a non-UTF-8 file in binary mode (File.binread / "rb") so the
 * encoding is autodetected from its BOM / declaration. */
static VALUE
mkr_xml_s_parse(int argc, VALUE *argv, VALUE self)
{
    (void)self;
    VALUE rb_source, rb_opts;
    rb_scan_args(argc, argv, "1:", &rb_source, &rb_opts);
    mkr_xml_limits_t limits = mkr_xml_parse_limits(rb_opts);  /* validates; may raise */
    size_t budget = limits.max_bytes ? limits.max_bytes : (size_t)MKR_XML_MAX_BYTES;

    /* Read an IO/File-like source (an object responding to #read), like the HTML
     * entry; a String passes straight through. */
    if (rb_respond_to(rb_source, rb_intern("read"))) {
        rb_source = rb_funcall(rb_source, rb_intern("read"), 0);
    }

    /* Strict decode under the GVL: invalid UTF-8 / undecodable byte / NUL all
     * raise Makiri::XML::SyntaxError here (no U+FFFD repair). Passing the budget
     * lets decode reject an over-budget input (LimitExceeded) before its
     * validation copy and the GVL-release copy below - so a hostile oversized
     * document is not materialised twice for a doomed parse. */
    VALUE decoded = mkr_xml_decode_input(rb_String(rb_source), budget);

    /* Copy the decoded bytes into a private C buffer up front - BEFORE allocating
     * any Ruby object (the wrap below) - so there is NO GC point between obtaining
     * +decoded+ and copying it, and the parse can then run with the GVL released
     * without racing GC/compaction on the String's backing store. */
    mkr_owned_bytes_t source = {0};
    if (mkr_ruby_copy_bytes(decoded, &source) != 0) {
        rb_raise(mkr_eError, "out of memory copying XML source");
    }

    /* Build an empty XML handle and wrap it (doc == NULL) so a failure mid-parse
     * frees cleanly via GC (mkr_parsed_destroy -> the XML branch ->
     * mkr_xml_doc_destroy(NULL), a no-op). The source is already copied, so this
     * Ruby allocation cannot disturb it. */
    mkr_parsed_t *parsed = mkr_parsed_new_xml(NULL);
    if (parsed == NULL) {
        mkr_owned_bytes_clear(&source);
        rb_raise(mkr_eError, "out of memory allocating XML document");
    }
    VALUE obj = mkr_wrap_document(parsed); /* GC owns +parsed+ from here */

    mkr_xml_parse_nogvl_t args = { source.ptr, source.len, limits, NULL, MKR_XML_OK };
    rb_thread_call_without_gvl(mkr_xml_parse_nogvl, &args, NULL, NULL);
    mkr_owned_bytes_clear(&source);

    if (args.result == NULL) {
        switch (args.status) {
        case MKR_XML_ERR_SYNTAX:  rb_raise(mkr_eXmlSyntaxError,   "malformed XML"); break;
        case MKR_XML_ERR_LIMIT:   rb_raise(mkr_eXmlLimitExceeded, "XML document budget exceeded"); break;
        case MKR_XML_ERR_VERSION: rb_raise(mkr_eXmlSyntaxError,
                                           "unsupported XML version (only XML 1.0 is supported)"); break;
        default:                  rb_raise(mkr_eError,            "failed to parse XML document"); break;
        }
    }
    mkr_parsed_set_xml_doc(parsed, args.result);
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
    /* mkr_xml_node_unwrap is kind-checked (raises on a non-XML node) and resolves
     * an XML Document to its arena document node; mkr_node_document gives the
     * keepalive Document for either. */
    *out_document = mkr_node_document(self);
    return mkr_xml_node_unwrap(self);
}

/* Register a {prefix => uri} Ruby Hash onto +ctx+ for a single query. On any bad
 * entry (non-string-coercible, invalid UTF-8 / embedded NUL, or OOM) the context
 * is freed and an exception is raised - never a partial registration. RSS/Atom
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

    /* Verify the expression's text contract BEFORE allocating ctx: the mint below
     * raises on invalid UTF-8 / a NUL, and such a raise after ctx is allocated
     * (but before it is freed) would leak ctx. This check raises with nothing to
     * leak; the real borrow is still minted late (after ns registration) so the
     * bytes are never held across that step's GC points. */
    mkr_verify_text(rb_String(rb_expr), "XPath expression");

    /* The document node is the "document" (the engine's XML namespace services
     * ignore it) and the "/" root for absolute paths; +context+ is the relative
     * context node (the document node for a Document, else the node itself). */
    mkr_xpath_context_t *ctx =
        mkr_xpath_context_new((void *)xdoc->doc_node, (void *)context);
    if (ctx == NULL) {
        rb_raise(mkr_eError, "failed to allocate XPath context");
    }
    mkr_xpath_set_engine_kind(ctx, 1);
    mkr_xml_install_name_index(ctx, xdoc);
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
    /* Free ctx BEFORE converting: the value owns its own data (node-set points
     * at the document's DOM, strings into value's buffers) and never references
     * ctx, so converting after the free is safe - and it means a raise inside
     * mkr_xpath_value_to_ruby (e.g. a node-set cap / OOM building Ruby objects)
     * can no longer leak ctx. */
    mkr_xpath_context_free(ctx);
    VALUE result = mkr_xpath_value_to_ruby(&value, document); /* converts AND clears value */

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

/* ---- CSS selectors over XML (lowered to the native XPath engine) ----
 *
 * Makiri::XML CSS support compiles a selector to the engine's AST (mkr_css.c)
 * and runs it through the SAME evaluator as #xpath, so case-sensitivity,
 * namespaces, budgets and document order are identical. The Ruby wrappers
 * (lib/makiri/xml/node_methods.rb) collect the document's namespaces and pass a
 * normalised {prefix => uri} hash; the default namespace, if any, arrives under
 * the synthetic prefix "xmlns" (Nokogiri's convention), and a bare type selector
 * binds to it. These are the private `_css` / `_at_css` / `_css_matches?`
 * primitives those wrappers call. */

/* The synthetic default-namespace prefix, present iff the (already
 * prefix-normalised) namespace hash carries an "xmlns" key. */
static const char *
mkr_css_default_prefix(VALUE rb_ns)
{
    if (RB_TYPE_P(rb_ns, T_HASH)
        && !NIL_P(rb_hash_aref(rb_ns, rb_str_new_cstr(MKR_CSS_DEFAULT_NS_PREFIX)))) {
        return MKR_CSS_DEFAULT_NS_PREFIX;
    }
    return NULL;
}

/* Compile +rb_selector+ to an AST under +ctx+ (its namespaces already
 * registered). On a CSS syntax error raises Makiri::CSS::SyntaxError; on any
 * other engine error frees ctx and raises via the shared raiser. Returns the AST
 * (caller frees with mkr_node_free). */
static mkr_node_t *
mkr_css_compile_or_raise(mkr_xpath_context_t *ctx, VALUE rb_selector, VALUE rb_ns)
{
    mkr_css_ns_t cns = { mkr_css_default_prefix(rb_ns) };
    mkr_ruby_borrowed_text_t sv = mkr_ruby_verified_text(rb_selector, "CSS selector");
    mkr_xpath_error_t error = {0};
    mkr_xpath_limits_t *limits = mkr_ctx_limits(ctx);
    limits->ast_nodes = 0;
    mkr_node_t *ast = mkr_css_compile(mkr_verified_text_from_view(sv), &cns, limits, &error);
    RB_GC_GUARD(sv.value);
    if (ast == NULL) {
        int syntax = (error.status == MKR_XPATH_ERR_SYNTAX);
        mkr_xpath_context_free(ctx);
        if (syntax) {
            VALUE msg = error.message ? rb_utf8_str_new_cstr(error.message)
                                      : rb_str_new_cstr("invalid CSS selector");
            mkr_xpath_error_clear(&error);
            rb_raise(mkr_eCSSSyntaxError, "%" PRIsVALUE, msg);
        }
        mkr_xpath_raise(&error); /* frees error.message, never returns */
    }
    return ast;
}

static VALUE
mkr_xml_doc_css_run(VALUE self, VALUE rb_selector, VALUE rb_ns, int first_only)
{
    VALUE document = Qnil;
    mkr_xml_node_t *context = mkr_xml_query_context(self, &document);
    if (context == NULL) {
        return first_only ? Qnil : mkr_node_set_new(document);
    }
    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(document));

    /* Verify the selector's text contract BEFORE allocating ctx: the mint inside
     * mkr_css_compile_or_raise raises on invalid UTF-8 / a NUL, and such a raise
     * after ctx is allocated (but before it is freed) would leak ctx. This check
     * raises with nothing to leak; the real borrow is still taken late (after ns
     * registration) so the bytes are never held across that step's GC points. */
    mkr_verify_text(rb_String(rb_selector), "CSS selector");

    mkr_xpath_context_t *ctx =
        mkr_xpath_context_new((void *)xdoc->doc_node, (void *)context);
    if (ctx == NULL) {
        rb_raise(mkr_eError, "failed to allocate XPath context");
    }
    mkr_xpath_set_engine_kind(ctx, 1);
    mkr_xml_install_name_index(ctx, xdoc);
    mkr_xml_register_query_namespaces(ctx, rb_ns); /* frees ctx + raises on error */

    mkr_node_t *ast = mkr_css_compile_or_raise(ctx, rb_selector, rb_ns); /* frees ctx + raises on error */

    mkr_xpath_value_t value = {0};
    mkr_xpath_error_t error = {0};
    int rc = first_only ? mkr_xpath_eval_compiled_first(ctx, ast, &value, &error)
                        : mkr_xpath_eval_compiled(ctx, ast, &value, &error);
    mkr_node_free(ast);
    if (rc != 0) {
        mkr_xpath_context_free(ctx);
        mkr_xpath_raise(&error);
    }
    /* Free ctx BEFORE converting: the value owns its own data (node-set points
     * at the document's DOM, strings into value's buffers) and never references
     * ctx, so converting after the free is safe - and it means a raise inside
     * mkr_xpath_value_to_ruby (e.g. a node-set cap / OOM building Ruby objects)
     * can no longer leak ctx. */
    mkr_xpath_context_free(ctx);
    VALUE result = mkr_xpath_value_to_ruby(&value, document); /* converts AND clears value */

    if (first_only && rb_obj_is_kind_of(result, mkr_cNodeSet)) {
        return rb_funcall(result, rb_intern("first"), 0);
    }
    return result;
}

static VALUE
mkr_xml_node_css(VALUE self, VALUE selector, VALUE ns)
{
    return mkr_xml_doc_css_run(self, selector, ns, 0);
}

static VALUE
mkr_xml_node_at_css(VALUE self, VALUE selector, VALUE ns)
{
    return mkr_xml_doc_css_run(self, selector, ns, 1);
}

/* #matches?(selector): does THIS node match the selector? Evaluated by selecting
 * every matching node in the whole document (context = the document node, so the
 * descendant-rooted selector scans the entire tree) and testing membership by
 * node identity - the full-combinator-correct semantics. */
static VALUE
mkr_xml_node_css_matches(VALUE self, VALUE selector, VALUE ns)
{
    VALUE document = Qnil;
    mkr_xml_node_t *node = mkr_xml_query_context(self, &document);
    if (node == NULL) {
        return Qfalse;
    }
    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(document));

    /* Verify the selector BEFORE allocating ctx (see mkr_xml_doc_css_run): a
     * text-contract raise after ctx is allocated but before it is freed leaks it.
     * The real borrow is still taken late, inside mkr_css_compile_or_raise. */
    mkr_verify_text(rb_String(selector), "CSS selector");

    mkr_xpath_context_t *ctx =
        mkr_xpath_context_new((void *)xdoc->doc_node, (void *)xdoc->doc_node);
    if (ctx == NULL) {
        rb_raise(mkr_eError, "failed to allocate XPath context");
    }
    mkr_xpath_set_engine_kind(ctx, 1);
    mkr_xml_install_name_index(ctx, xdoc);
    mkr_xml_register_query_namespaces(ctx, ns); /* frees ctx + raises on error */

    mkr_node_t *ast = mkr_css_compile_or_raise(ctx, selector, ns); /* frees ctx + raises on error */

    mkr_xpath_value_t value = {0};
    mkr_xpath_error_t error = {0};
    int rc = mkr_xpath_eval_compiled(ctx, ast, &value, &error);
    mkr_node_free(ast);
    if (rc != 0) {
        mkr_xpath_context_free(ctx);
        mkr_xpath_raise(&error);
    }

    int found = 0;
    if (value.type == MKR_XPATH_TYPE_NODESET) {
        for (size_t i = 0; i < value.u.nodeset.count; i++) {
            if (value.u.nodeset.nodes[i] == (void *)node) { found = 1; break; }
        }
    }
    mkr_xpath_value_clear(&value);
    mkr_xpath_context_free(ctx);
    return found ? Qtrue : Qfalse;
}

/* The document's root element. */
static VALUE
mkr_xml_doc_root(VALUE self)
{
    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(self));
    return (xdoc == NULL) ? Qnil : mkr_wrap_xml_node(xdoc->root, self);
}

/* The document's DOCTYPE as a Makiri::XML::DocumentType (aliased
 * Makiri::XML::DTD), or nil if the document had no
 * `<!DOCTYPE ...>`. Mirrors Nokogiri's Document#internal_subset. The DTD's name
 * and external/system identifiers are read; the DTD body is NOT parsed (no
 * entity/element declarations are loaded - &name; stays an undefined-entity
 * error and no external subset is fetched). The doctype node is kept off the
 * tree, so XPath never sees it (XPath 1.0 has no doctype node type). */
static VALUE
mkr_xml_doc_internal_subset(VALUE self)
{
    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(self));
    return (xdoc == NULL || xdoc->doctype == NULL)
               ? Qnil
               : mkr_wrap_xml_node(xdoc->doctype, self);
}

/* Map a fragment-parse status to its Ruby exception (never returns on error). */
NORETURN(static void mkr_xml_raise_fragment_status(mkr_xml_status_t st));
static void
mkr_xml_raise_fragment_status(mkr_xml_status_t st)
{
    switch (st) {
    case MKR_XML_ERR_SYNTAX:  rb_raise(mkr_eXmlSyntaxError,   "malformed XML fragment");
    case MKR_XML_ERR_LIMIT:   rb_raise(mkr_eXmlLimitExceeded, "XML fragment budget exceeded");
    case MKR_XML_ERR_VERSION: rb_raise(mkr_eXmlSyntaxError,
                                       "unsupported XML version (only XML 1.0 is supported)");
    default:                  rb_raise(mkr_eError,            "failed to parse XML fragment");
    }
}

/* Strict-decode +rb_source+ and parse it as a fragment into +xdoc+ (when
 * +inherit_doc_ns+, names resolve against the document's root namespaces). The
 * parse runs under the GVL: a fragment is small, and an existing document's arena
 * must never be mutated with the GVL released. Returns the DOCUMENT_FRAGMENT node;
 * raises on a decode/parse failure. */
static mkr_xml_node_t *
mkr_xml_fragment_into(mkr_xml_doc_t *xdoc, VALUE rb_source, int inherit_doc_ns)
{
    VALUE decoded = mkr_xml_decode_input(rb_String(rb_source), xdoc->max_bytes);
    mkr_owned_bytes_t src = { 0 };
    if (mkr_ruby_copy_bytes(decoded, &src) != 0) {
        rb_raise(mkr_eError, "out of memory copying XML fragment source");
    }

    mkr_xml_status_t st = MKR_XML_OK;
    mkr_xml_node_t *frag = mkr_xml_parse_fragment(xdoc, src.ptr, src.len, inherit_doc_ns, &st);
    mkr_owned_bytes_clear(&src);
    if (frag == NULL) {
        mkr_xml_raise_fragment_status(st);
    }
    return frag;
}

/* A fresh, empty XML Document VALUE: a backing arena holding a DOCUMENT node and
 * no root element. Used by Document.new and as DocumentFragment.parse's backing
 * document. Raises on OOM (with +parsed+ already GC-owned, so it frees cleanly). */
static VALUE
mkr_xml_new_empty_document(void)
{
    mkr_parsed_t *parsed = mkr_parsed_new_xml(NULL);
    if (parsed == NULL) {
        rb_raise(mkr_eError, "out of memory allocating XML document");
    }
    VALUE doc_obj = mkr_wrap_document(parsed);     /* GC owns +parsed+ from here */
    mkr_xml_doc_t *xdoc = mkr_xml_doc_new();
    if (xdoc == NULL) {
        rb_raise(mkr_eError, "out of memory allocating XML document");
    }
    mkr_parsed_set_xml_doc(parsed, xdoc);          /* GC now frees +xdoc+ via +parsed+ */
    xdoc->doc_node = mkr_xml_arena_node(xdoc, MKR_XML_NODE_TYPE_DOCUMENT);
    if (xdoc->doc_node == NULL) {
        rb_raise(mkr_eError, "out of memory allocating XML document");
    }
    return doc_obj;
}

/* call-seq: Makiri::XML::Document.new -> Document
 * A new, empty XML document (no root element) to build up programmatically with
 * #create_element etc. and #add_child / #root=, like Nokogiri. Any arguments
 * (Nokogiri accepts a version / encoding) are accepted and ignored. */
static VALUE
mkr_xml_document_s_new(int argc, VALUE *argv, VALUE klass)
{
    (void)argc; (void)argv; (void)klass;
    return mkr_xml_new_empty_document();
}

/* call-seq: Makiri::XML::DocumentFragment.parse(source) -> DocumentFragment
 * Parse +source+ into a standalone fragment with its own (empty) backing
 * document. The fragment is self-contained: a prefixed name must declare its
 * namespace within the fragment itself (use Document#fragment to parse against an
 * existing document's in-scope namespaces). */
static VALUE
mkr_xml_fragment_s_parse(VALUE klass, VALUE rb_source)
{
    (void)klass;
    VALUE doc_obj = mkr_xml_new_empty_document();
    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(doc_obj));
    mkr_xml_node_t *frag = mkr_xml_fragment_into(xdoc, rb_source, 0);
    VALUE result = mkr_wrap_xml_node(frag, doc_obj);
    return result;
}

/* call-seq: doc.fragment(source) -> DocumentFragment
 * Parse +source+ into a fragment bound to this document, resolving names against
 * the document's in-scope (root) namespaces, so the fragment's nodes can be
 * spliced in with Node#add_child and friends. */
static VALUE
mkr_xml_doc_fragment(VALUE self, VALUE rb_source)
{
    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(self));
    if (xdoc == NULL) {
        rb_raise(mkr_eError, "the document has no arena");
    }
    mkr_xml_node_t *frag = mkr_xml_fragment_into(xdoc, rb_source, 1);
    VALUE result = mkr_wrap_xml_node(frag, self);
    return result;
}

void
mkr_init_xml(void)
{
    /* XML::Document is a Makiri::Document leaf (§12): is_a?(Makiri::Document) is
     * true, but it carries no HTML readers (those are on Makiri::HTML, which it
     * does not include) - the read-only XML surface is structural. */
    mkr_cXmlDocument = rb_define_class_under(mkr_mXML, "Document", mkr_cDocument);
    rb_undef_alloc_func(mkr_cXmlDocument); /* created only from C, never .new */
    rb_include_module(mkr_cXmlDocument, mkr_mXmlNodeMethods);

    rb_define_method(mkr_cXmlDocument, "root",     mkr_xml_doc_root,     0);
    rb_define_method(mkr_cXmlDocument, "internal_subset", mkr_xml_doc_internal_subset, 0);
    rb_define_method(mkr_cXmlDocument, "fragment", mkr_xml_doc_fragment, 1);
    rb_define_singleton_method(mkr_cXmlDocument, "new", mkr_xml_document_s_new, -1);
    rb_define_singleton_method(mkr_cXmlDocumentFragment, "parse", mkr_xml_fragment_s_parse, 1);

    /* xpath / at_xpath work on the document and on any XML node (rooted at that
     * node), so they live on the shared XML node behavior module + the document. */
    rb_define_method(mkr_cXmlDocument,   "xpath",    mkr_xml_doc_xpath,    -1);
    rb_define_method(mkr_cXmlDocument,   "at_xpath", mkr_xml_doc_at_xpath, -1);
    rb_define_method(mkr_mXmlNodeMethods, "xpath",    mkr_xml_doc_xpath,    -1);
    rb_define_method(mkr_mXmlNodeMethods, "at_xpath", mkr_xml_doc_at_xpath, -1);

    /* CSS selectors over XML: private primitives called by the Ruby #css /
     * #at_css / #matches? wrappers (which collect the document namespaces). The
     * selector is lowered to the native XPath engine (mkr_css.c). */
    rb_define_private_method(mkr_mXmlNodeMethods, "_css",        mkr_xml_node_css,         2);
    rb_define_private_method(mkr_mXmlNodeMethods, "_at_css",     mkr_xml_node_at_css,      2);
    rb_define_private_method(mkr_mXmlNodeMethods, "_css_matches", mkr_xml_node_css_matches, 2);

    /* The native XML parser, exposed as XML::Document.parse, mirroring HTML
     * (HTML::Document.parse). The Makiri::XML(source) convenience delegates to it
     * in Ruby (lib/makiri.rb), as Makiri.HTML does for HTML::Document.parse. */
    rb_define_singleton_method(mkr_cXmlDocument, "parse", mkr_xml_s_parse, -1);
}
