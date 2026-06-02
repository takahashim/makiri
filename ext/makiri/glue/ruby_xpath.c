#include "glue.h"
#include "../xpath/mkr_xpath.h"
#include "../xpath/mkr_xpath_internal.h" /* mkr_val_t / nodeset for the handler bridge */
#include "../core/mkr_safe.h"

#include <stdlib.h>
#include <string.h>

/*
 * Ruby <-> native XPath engine bridge.
 *
 *   Makiri::XPathContext.new(node)              -> context bound to a document
 *   ctx#evaluate(expr, handler = nil)           -> NodeSet | String | Float | bool
 *   ctx#register_namespace(prefix, uri)
 *   ctx#register_variable(name, value)
 *
 *   Node#xpath(expr, handler = nil) / Node#at_xpath(expr, handler = nil)
 *
 * The engine owns its result buffers (node array / string); we copy results
 * into Ruby objects and immediately clear the native value.
 *
 * Custom functions: when an expression calls a function that is not a built-in,
 * the engine delegates to a resolver. We install one that dispatches to a Ruby
 * +handler+ object (method name = XPath local name with '-' mapped to '_'),
 * converting arguments and the return value between engine and Ruby values. The
 * handler is invoked under rb_protect so a Ruby exception becomes a clean engine
 * error instead of long-jumping through the evaluator's C stack.
 */

/* ------------------------------------------------------------------ */
/* XPathContext wrapper                                               */
/* ------------------------------------------------------------------ */

/* Per-context compiled-AST cache: an XPathContext is typically reused to run
 * the same handful of expressions many times, so we parse each expression once
 * and re-evaluate the cached AST (mkr_xpath_eval_compiled resets the per-eval
 * counters / memo slots, so a cached AST is safely reusable). Bounded so a
 * context fed unbounded distinct expressions doesn't grow without limit. */
#define MKR_AST_CACHE_MAX 1024

typedef struct {
    char      *expr;
    mkr_node_t *ast;
} mkr_ast_cache_entry_t;

typedef struct {
    mkr_xpath_context_t   *ctx;
    VALUE                  document; /* keepalive: owns the DOM arena */
    VALUE                  node;     /* keepalive: the context node wrapper  */
    mkr_ast_cache_entry_t *cache;
    size_t                 cache_count;
    size_t                 cache_cap;
} mkr_xpath_ctx_data_t;

static void
mkr_xpath_ctx_mark(void *ptr)
{
    mkr_xpath_ctx_data_t *d = (mkr_xpath_ctx_data_t *)ptr;
    rb_gc_mark(d->document);
    rb_gc_mark(d->node);
}

static void
mkr_xpath_ctx_free(void *ptr)
{
    mkr_xpath_ctx_data_t *d = (mkr_xpath_ctx_data_t *)ptr;
    for (size_t i = 0; i < d->cache_count; i++) {
        free(d->cache[i].expr);
        mkr_node_free(d->cache[i].ast);
    }
    free(d->cache);
    if (d->ctx != NULL) {
        mkr_xpath_context_free(d->ctx);
    }
    xfree(d);
}

static size_t
mkr_xpath_ctx_memsize(const void *ptr)
{
    (void)ptr;
    return sizeof(mkr_xpath_ctx_data_t);
}

static const rb_data_type_t mkr_xpath_ctx_type = {
    "Makiri::XPathContext",
    { mkr_xpath_ctx_mark, mkr_xpath_ctx_free, mkr_xpath_ctx_memsize, },
    0, 0, RUBY_TYPED_FREE_IMMEDIATELY,
};

/* ------------------------------------------------------------------ */
/* result + error mapping                                             */
/* ------------------------------------------------------------------ */

/* Translate an engine error into a Ruby exception and raise. Never returns. */
static NORETURN(void mkr_xpath_raise(mkr_xpath_error_t *err));

static void
mkr_xpath_raise(mkr_xpath_error_t *err)
{
    VALUE klass;
    switch (err->status) {
    case MKR_XPATH_ERR_SYNTAX:
        klass = mkr_eXPathSyntaxError;
        break;
    case MKR_XPATH_ERR_LIMIT:
        klass = mkr_eXPathLimitExceeded;
        break;
    default:
        klass = mkr_eError;
        break;
    }
    /* Copy the message before clearing the native error. */
    VALUE msg = err->message ? rb_utf8_str_new_cstr(err->message)
                             : rb_utf8_str_new_cstr("XPath evaluation failed");
    mkr_xpath_error_clear(err);
    rb_exc_raise(rb_exc_new_str(klass, msg));
}

/* Convert a (just-produced) engine value into a Ruby object, then release any
 * heap the engine handed us. document is the keepalive for node-set results. */
static VALUE
mkr_xpath_value_to_ruby(mkr_xpath_value_t *v, VALUE document)
{
    VALUE result;
    switch (v->type) {
    case MKR_XPATH_TYPE_NODESET: {
        result = mkr_node_set_new(document);
        for (size_t i = 0; i < v->u.nodeset.count; ++i) {
            mkr_node_set_push(result, v->u.nodeset.nodes[i]);
        }
        break;
    }
    case MKR_XPATH_TYPE_STRING:
        result = rb_utf8_str_new(v->u.string ? v->u.string : "", (long)v->string_len);
        break;
    case MKR_XPATH_TYPE_NUMBER:
        result = rb_float_new(v->u.number);
        break;
    case MKR_XPATH_TYPE_BOOLEAN:
        result = v->u.boolean ? Qtrue : Qfalse;
        break;
    default:
        result = Qnil;
        break;
    }
    mkr_xpath_value_clear(v);
    return result;
}

/* ------------------------------------------------------------------ */
/* context construction                                               */
/* ------------------------------------------------------------------ */

/* Build a native context bound to rb_node's document, with rb_node as the
 * context node. Raises on allocation failure. The attr->owner index is built
 * up front so the engine's parent/ancestor axes and document-order sort see
 * attribute owners. */
/* Resolve the namespace_matching: keyword (nil | :strict | :lax) to the
 * unprefixed-lax flag (0 = strict default, 1 = lax). +opts+ is the keyword
 * hash from rb_scan_args (nil when none given). Raises ArgumentError otherwise. */
static int
mkr_ns_matching_lax(VALUE opts)
{
    if (NIL_P(opts)) return 0;
    VALUE v = rb_hash_aref(opts, ID2SYM(rb_intern("namespace_matching")));
    if (NIL_P(v) || v == ID2SYM(rb_intern("strict"))) return 0;
    if (v == ID2SYM(rb_intern("lax"))) return 1;
    rb_raise(rb_eArgError,
             "namespace_matching: must be :strict or :lax, got %" PRIsVALUE,
             rb_inspect(v));
}

static mkr_xpath_context_t *
mkr_xpath_context_for(VALUE rb_node, VALUE document)
{
    lxb_dom_node_t     *node = mkr_node_unwrap(rb_node);
    lxb_dom_document_t *doc  = mkr_doc_unwrap(document);

    mkr_parsed_t *parsed = mkr_doc_parsed(document);
    if (mkr_parsed_attr_index_build(parsed) != 0) {
        rb_raise(mkr_eError, "failed to build attribute index for XPath");
    }

    mkr_xpath_context_t *ctx = mkr_xpath_context_new(doc, node);
    if (ctx == NULL) {
        rb_raise(mkr_eError, "failed to allocate XPath context");
    }
    /* Hand the engine the document's element index (tag id -> elements) plus
     * the compat-layer lookups, so `//tag` can be answered without a tree walk.
     * The engine calls back through these hooks and never sees the index's
     * concrete type. Borrowed; the index lives on the parsed document, which
     * outlives this context. */
    mkr_xpath_context_set_element_index(ctx, mkr_parsed_element_index(parsed),
                                        mkr_element_index_tag,
                                        mkr_element_index_has_foreign);
    return ctx;
}

/*
 * call-seq: XPathContext.new(node, namespace_matching: :strict) -> XPathContext
 *
 * Build a context whose context node is +node+ (a Node or Document).
 * namespace_matching: :strict (default) resolves unprefixed name tests in the
 * HTML namespace (HTML5/WHATWG-faithful; SVG/MathML need a prefix); :lax makes
 * unprefixed tests namespace-agnostic.
 */
static VALUE
mkr_xpath_ctx_s_new(int argc, VALUE *argv, VALUE klass)
{
    VALUE rb_node, opts;
    rb_scan_args(argc, argv, "1:", &rb_node, &opts);
    int lax = mkr_ns_matching_lax(opts);

    if (!rb_obj_is_kind_of(rb_node, mkr_cNode)) {
        rb_raise(rb_eTypeError, "expected a Makiri::Node");
    }
    VALUE document = mkr_node_document(rb_node);

    mkr_xpath_ctx_data_t *d;
    VALUE obj = TypedData_Make_Struct(klass, mkr_xpath_ctx_data_t,
                                      &mkr_xpath_ctx_type, d);
    d->ctx         = NULL;
    d->document    = document;
    d->node        = rb_node;
    d->cache       = NULL;
    d->cache_count = 0;
    d->cache_cap   = 0;
    d->ctx         = mkr_xpath_context_for(rb_node, document);
    mkr_ctx_set_unprefixed_lax(d->ctx, lax);
    return obj;
}

static mkr_xpath_ctx_data_t *
mkr_xpath_ctx_unwrap(VALUE self)
{
    mkr_xpath_ctx_data_t *d;
    TypedData_Get_Struct(self, mkr_xpath_ctx_data_t, &mkr_xpath_ctx_type, d);
    return d;
}

/* XPathContext#node= : rebind the context node (must be in the same document),
 * so the context can be reused to evaluate relative expressions against
 * several nodes. Namespace/variable registrations are preserved. */
static VALUE
mkr_xpath_ctx_set_node(VALUE self, VALUE rb_node)
{
    if (!rb_obj_is_kind_of(rb_node, mkr_cNode)) {
        rb_raise(rb_eTypeError, "expected a Makiri::Node");
    }
    mkr_xpath_ctx_data_t *d = mkr_xpath_ctx_unwrap(self);
    if (mkr_node_document(rb_node) != d->document) {
        rb_raise(mkr_eError, "context node must belong to the same document");
    }
    d->node = rb_node; /* keepalive; marked in mkr_xpath_ctx_mark */
    mkr_ctx_set_node(d->ctx, mkr_node_unwrap(rb_node));
    return rb_node;
}

/* ------------------------------------------------------------------ */
/* custom function handler bridge                                     */
/* ------------------------------------------------------------------ */

typedef struct {
    VALUE handler;  /* Ruby object answering custom-function methods */
    VALUE document; /* keepalive + wraps node-set arguments */
} mkr_handler_bridge_t;

/* engine value -> Ruby */
static VALUE
mkr_arg_to_ruby(mkr_handler_bridge_t *b, const mkr_val_t *v)
{
    switch (v->type) {
    case MKR_XPATH_TYPE_NODESET: {
        VALUE set = mkr_node_set_new(b->document);
        for (size_t i = 0; i < v->u.nodeset.count; i++) {
            mkr_node_set_push(set, v->u.nodeset.items[i]);
        }
        return set;
    }
    case MKR_XPATH_TYPE_STRING:
        return rb_utf8_str_new(v->u.string.ptr ? v->u.string.ptr : "", (long)v->u.string.len);
    case MKR_XPATH_TYPE_NUMBER:
        return rb_float_new(v->u.number);
    case MKR_XPATH_TYPE_BOOLEAN:
        return v->u.boolean ? Qtrue : Qfalse;
    }
    return Qnil;
}

/* Duplicate a validated text view into a fresh owned NUL-terminated C string. */
static char *
mkr_dup_text_view(mkr_ruby_text_view_t v)
{
    return mkr_strndup(v.ptr, v.len);
}

static int
mkr_push_result_node(mkr_xpath_context_t *ctx, VALUE rb_node, mkr_val_t *out,
                     char *errbuf, size_t errlen)
{
    lxb_dom_node_t *n = mkr_node_unwrap(rb_node);
    if (n->owner_document != mkr_ctx_document(ctx)) {
        snprintf(errbuf, errlen, "handler returned a node from a different document");
        return -1;
    }
    mkr_xpath_error_t ierr = {0};
    if (mkr_nodeset_push(&out->u.nodeset, n, mkr_ctx_limits(ctx), &ierr) != 0) {
        mkr_xpath_error_clear(&ierr);
        snprintf(errbuf, errlen, "out of memory building handler result");
        return -1;
    }
    return 0;
}

/* Ruby return value -> engine value. Returns 0 on success, -1 with errbuf set. */
static int
mkr_ruby_to_out(mkr_xpath_context_t *ctx, VALUE r, mkr_val_t *out,
                char *errbuf, size_t errlen)
{
    if (r == Qtrue || r == Qfalse) {
        out->type = MKR_XPATH_TYPE_BOOLEAN;
        out->u.boolean = (r == Qtrue);
        return 0;
    }
    if (RB_FLOAT_TYPE_P(r) || RB_INTEGER_TYPE_P(r)) {
        out->type = MKR_XPATH_TYPE_NUMBER;
        out->u.number = NUM2DBL(r);
        return 0;
    }
    if (rb_obj_is_kind_of(r, mkr_cNode) || rb_obj_is_kind_of(r, mkr_cNodeSet)) {
        out->type = MKR_XPATH_TYPE_NODESET;
        mkr_nodeset_init(&out->u.nodeset);
        if (rb_obj_is_kind_of(r, mkr_cNode)) {
            if (mkr_push_result_node(ctx, r, out, errbuf, errlen) != 0) {
                mkr_nodeset_clear(&out->u.nodeset);
                return -1;
            }
        } else {
            long n = NUM2LONG(rb_funcall(r, rb_intern("length"), 0));
            for (long i = 0; i < n; i++) {
                VALUE node = rb_funcall(r, rb_intern("[]"), 1, LONG2NUM(i));
                if (!rb_obj_is_kind_of(node, mkr_cNode)) {
                    continue;
                }
                if (mkr_push_result_node(ctx, node, out, errbuf, errlen) != 0) {
                    mkr_nodeset_clear(&out->u.nodeset);
                    return -1;
                }
            }
        }
        return 0;
    }
    /* nil and everything else: coerce to string (nil -> ""). */
    if (NIL_P(r)) {
        mkr_val_set_owned_string(out, mkr_strdup(""), 0);
    } else {
        VALUE sv = rb_obj_as_string(r);
        mkr_ruby_text_view_t vv;
        const char *bad =
            mkr_ruby_engine_string_view(sv, mkr_ctx_limits(ctx)->max_string_bytes, &vv);
        if (bad != NULL) {
            snprintf(errbuf, errlen, "handler returned an invalid string: %s", bad);
            return -1;
        }
        mkr_val_set_owned_string(out, mkr_dup_text_view(vv), vv.len);
        RB_GC_GUARD(vv.value);
    }
    if (out->u.string.ptr == NULL) {
        snprintf(errbuf, errlen, "out of memory converting handler result");
        return -1;
    }
    return 0;
}

/* Upper bound on handler arguments. Matches the engine's default
 * max_function_args; the resolver rejects any call above it, so the fixed argv
 * array below cannot overflow regardless of how the limit is tuned. Using a
 * compile-time-fixed array (not ALLOCA_N) keeps stack use independent of the
 * runtime argument count. */
#define MKR_HANDLER_MAX_ARGS 64

typedef struct {
    mkr_handler_bridge_t *b;
    mkr_xpath_context_t  *ctx;
    ID                    method;
    const mkr_val_t      *args;
    size_t                nargs;
    mkr_val_t            *out;
    int                   status; /* set by the body: 0 ok, -1 conversion error */
    char                  errbuf[200];
    VALUE                 argv[MKR_HANDLER_MAX_ARGS];
} mkr_handler_call_t;

/* Runs under rb_protect: build Ruby args, invoke the handler, convert result.
 * c->nargs is <= MKR_HANDLER_MAX_ARGS (enforced by the resolver). */
static VALUE
mkr_handler_call_body(VALUE p)
{
    mkr_handler_call_t *c = (mkr_handler_call_t *)p;
    for (size_t i = 0; i < c->nargs; i++) {
        c->argv[i] = mkr_arg_to_ruby(c->b, &c->args[i]);
    }
    VALUE r = rb_funcallv(c->b->handler, c->method, (int)c->nargs, c->argv);
    c->status = mkr_ruby_to_out(c->ctx, r, c->out, c->errbuf, sizeof(c->errbuf));
    return Qnil;
}

/* Resolver return convention: 0 = handled, -1 = errored, +1 = not found. */
static int
mkr_handler_resolver(void *user_data, mkr_xpath_context_t *ctx,
                     void *self_node, size_t self_pos, size_t self_size,
                     const char *ns_uri, const char *local_name,
                     void *args, size_t nargs, void *out, mkr_xpath_error_t *err)
{
    (void)self_node; (void)self_pos; (void)self_size; (void)ns_uri;
    mkr_handler_bridge_t *b = (mkr_handler_bridge_t *)user_data;
    if (b == NULL || NIL_P(b->handler) || local_name == NULL) {
        return 1;
    }

    /* Method name: XPath uses '-', Ruby uses '_'. */
    char name[128];
    size_t n = 0;
    while (n + 1 < sizeof(name) && local_name[n] != '\0') {
        name[n] = (local_name[n] == '-') ? '_' : local_name[n];
        n++;
    }
    if (local_name[n] != '\0') {
        return 1; /* too long to map to a Ruby method name */
    }
    name[n] = '\0';

    ID method = rb_intern(name);
    if (!rb_respond_to(b->handler, method)) {
        return 1; /* let the engine raise "unknown function" */
    }

    /* The fixed argv buffer holds up to MKR_HANDLER_MAX_ARGS; refuse a call
     * beyond it (the engine's default cap keeps this unreachable in practice). */
    if (nargs > MKR_HANDLER_MAX_ARGS) {
        mkr_err_setf(err, MKR_XPATH_ERR_RUNTIME,
                     "handler function '%s' called with too many arguments (%zu > %d)",
                     name, nargs, MKR_HANDLER_MAX_ARGS);
        return -1;
    }

    mkr_handler_call_t call = {
        .b = b, .ctx = ctx, .method = method,
        .args = (const mkr_val_t *)args, .nargs = nargs,
        .out = (mkr_val_t *)out, .status = 0,
    };
    call.errbuf[0] = '\0';

    int state = 0;
    rb_protect(mkr_handler_call_body, (VALUE)&call, &state);
    if (state != 0) {
        VALUE exc = rb_errinfo();
        rb_set_errinfo(Qnil);
        char msg[200];
        mkr_ruby_exception_message(exc, msg, sizeof(msg));
        mkr_err_setf(err, MKR_XPATH_ERR_RUNTIME, "handler raised: %s", msg);
        return -1;
    }
    if (call.status != 0) {
        mkr_err_set(err, MKR_XPATH_ERR_RUNTIME, call.errbuf);
        return -1;
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/* evaluate / register                                                */
/* ------------------------------------------------------------------ */

/* Evaluate a compiled AST under the GVL.
 *
 * XPath evaluation deliberately does NOT release the GVL. The engine and DOM
 * are not thread-safe against concurrent mutation, and holding the GVL across
 * every evaluation makes that safe by construction: the GVL serialises all
 * Ruby-thread C code, so an XPath walk can never run in parallel with a tree
 * mutation, with another evaluation on the same context, or with a
 * register_variable/register_namespace/node= on the same context. No extra
 * locking is required (and none is used). Parsing still releases the GVL —
 * a freshly parsed document is not yet shared, so it has no such hazard.
 *
 * (Releasing the GVL for the handler-free case was measured to scale XPath
 * across threads, but the locking needed to make a GVL-released walk safe
 * against shared-document mutation was not worth the verification burden;
 * single-thread performance is unaffected by holding the GVL.) */
static int
mkr_eval_compiled(mkr_xpath_context_t *ctx, mkr_node_t *ast,
                  mkr_xpath_value_t *value, mkr_xpath_error_t *error)
{
    return mkr_xpath_eval_compiled(ctx, ast, value, error);
}

/* Return the compiled AST for +expr+, parsing and caching it on first use.
 * Sets *owned to 1 when the returned AST is NOT owned by the cache (the cache
 * was full or could not grow) and the caller must free it. Returns NULL on a
 * parse error (error filled). */
static mkr_node_t *
mkr_ctx_cached_ast(mkr_xpath_ctx_data_t *d, mkr_valid_text_t expr,
                   mkr_xpath_error_t *error, int *owned)
{
    *owned = 0;
    for (size_t i = 0; i < d->cache_count; i++) {
        if (strcmp(d->cache[i].expr, expr.ptr) == 0) {
            return d->cache[i].ast;
        }
    }

    mkr_xpath_limits_t *limits = mkr_ctx_limits(d->ctx);
    limits->ast_nodes = 0;
    mkr_node_t *ast = mkr_parse(expr, limits, error);
    if (ast == NULL) {
        return NULL;
    }

    /* Cache it unless the cache is full or cannot grow; in that case the
     * caller owns (and frees) the freshly-parsed AST. */
    if (d->cache_count >= MKR_AST_CACHE_MAX) {
        *owned = 1;
        return ast;
    }
    if (mkr_grow_reserve((void **)&d->cache, &d->cache_cap, d->cache_count + 1,
                         sizeof(*d->cache)) != MKR_OK) {
        *owned = 1;
        return ast;
    }
    char *key = mkr_strdup(expr.ptr);
    if (key == NULL) {
        *owned = 1;
        return ast;
    }
    d->cache[d->cache_count].expr = key;
    d->cache[d->cache_count].ast  = ast;
    d->cache_count++;
    return ast;
}

static VALUE
mkr_xpath_ctx_evaluate(int argc, VALUE *argv, VALUE self)
{
    VALUE rb_expr, handler;
    rb_scan_args(argc, argv, "11", &rb_expr, &handler);

    mkr_xpath_ctx_data_t *d = mkr_xpath_ctx_unwrap(self);
    mkr_ruby_text_view_t ev = mkr_ruby_checked_text(rb_expr, "XPath expression");

    mkr_xpath_error_t error = {0};
    int owned = 0;
    mkr_node_t *ast = mkr_ctx_cached_ast(d, mkr_text_from_view(ev), &error, &owned);
    RB_GC_GUARD(ev.value);
    if (ast == NULL) {
        mkr_xpath_raise(&error); /* parse error, never returns */
    }

    mkr_handler_bridge_t bridge = { handler, d->document };
    int has_handler = !NIL_P(handler);
    if (has_handler) {
        mkr_xpath_context_set_user_data(d->ctx, &bridge);
        mkr_xpath_set_func_resolver(d->ctx, mkr_handler_resolver);
    }
    mkr_xpath_value_t value = {0};
    int rc = mkr_eval_compiled(d->ctx, ast, &value, &error);
    if (has_handler) {
        mkr_xpath_set_func_resolver(d->ctx, NULL);
        mkr_xpath_context_set_user_data(d->ctx, NULL);
    }
    if (owned) {
        mkr_node_free(ast);
    }
    if (rc != 0) {
        mkr_xpath_raise(&error); /* never returns */
    }
    return mkr_xpath_value_to_ruby(&value, d->document);
}

static VALUE
mkr_xpath_ctx_register_ns(VALUE self, VALUE rb_prefix, VALUE rb_uri)
{
    mkr_xpath_ctx_data_t *d = mkr_xpath_ctx_unwrap(self);
    mkr_ruby_text_view_t pv = mkr_ruby_checked_text(rb_prefix, "namespace prefix");
    mkr_ruby_text_view_t uv = mkr_ruby_checked_text(rb_uri, "namespace URI");
    int rc = mkr_xpath_register_ns(d->ctx, mkr_text_from_view(pv),
                                   mkr_text_from_view(uv)); /* copies both */
    RB_GC_GUARD(pv.value);
    RB_GC_GUARD(uv.value);
    if (rc != 0) {
        rb_raise(mkr_eError, "failed to register namespace");
    }
    return self;
}

static VALUE
mkr_xpath_ctx_register_variable(VALUE self, VALUE rb_name, VALUE rb_value)
{
    mkr_xpath_ctx_data_t *d = mkr_xpath_ctx_unwrap(self);
    mkr_ruby_text_view_t nv = mkr_ruby_checked_text(rb_name, "variable name");
    /* The value gets the stricter engine-string check (adds the byte cap on top
     * of no-NUL / valid-UTF-8). */
    VALUE value = rb_obj_as_string(rb_value);
    mkr_ruby_text_view_t vv;
    const char *bad =
        mkr_ruby_engine_string_view(value, mkr_ctx_limits(d->ctx)->max_string_bytes, &vv);
    if (bad != NULL) {
        rb_raise(mkr_eError, "invalid variable value: %s", bad);
    }
    int rc = mkr_xpath_register_variable_string(d->ctx, mkr_text_from_view(nv),
                                                mkr_text_from_view(vv)); /* copies both */
    RB_GC_GUARD(nv.value);
    RB_GC_GUARD(value);
    if (rc != 0) {
        rb_raise(mkr_eError, "failed to register variable");
    }
    return self;
}

/* ------------------------------------------------------------------ */
/* Node#xpath / Node#at_xpath                                         */
/* ------------------------------------------------------------------ */

/* Evaluate expr against self with a throwaway context and optional handler.
 * Evaluation runs under the GVL (see mkr_eval_compiled — XPath never releases
 * it). */
static VALUE
mkr_node_xpath_run(VALUE self, VALUE rb_expr, VALUE handler, int lax)
{
    VALUE document = mkr_node_document(self);
    mkr_ruby_text_view_t ev = mkr_ruby_checked_text(rb_expr, "XPath expression");

    mkr_xpath_context_t *ctx = mkr_xpath_context_for(self, document);
    mkr_ctx_set_unprefixed_lax(ctx, lax);

    mkr_xpath_error_t error = {0};
    mkr_xpath_limits_t *limits = mkr_ctx_limits(ctx);
    limits->ast_nodes = 0;
    mkr_node_t *ast = mkr_parse(mkr_text_from_view(ev), limits, &error);
    RB_GC_GUARD(ev.value); /* keep the expr bytes alive across the parse */
    if (ast == NULL) {
        mkr_xpath_context_free(ctx);
        mkr_xpath_raise(&error); /* parse error, never returns */
    }

    mkr_handler_bridge_t bridge = { handler, document };
    int has_handler = !NIL_P(handler);
    if (has_handler) {
        mkr_xpath_context_set_user_data(ctx, &bridge);
        mkr_xpath_set_func_resolver(ctx, mkr_handler_resolver);
    }
    mkr_xpath_value_t value = {0};
    int rc = mkr_eval_compiled(ctx, ast, &value, &error);
    if (has_handler) {
        mkr_xpath_set_func_resolver(ctx, NULL);
        mkr_xpath_context_set_user_data(ctx, NULL);
    }
    mkr_node_free(ast);
    if (rc != 0) {
        mkr_xpath_context_free(ctx);
        mkr_xpath_raise(&error); /* never returns */
    }
    VALUE result = mkr_xpath_value_to_ruby(&value, document);
    mkr_xpath_context_free(ctx);
    return result;
}

static VALUE
mkr_node_xpath(int argc, VALUE *argv, VALUE self)
{
    VALUE rb_expr, handler, opts;
    rb_scan_args(argc, argv, "11:", &rb_expr, &handler, &opts);
    return mkr_node_xpath_run(self, rb_expr, handler, mkr_ns_matching_lax(opts));
}

/* First matching node (for a node-set), or the scalar value otherwise. */
static VALUE
mkr_node_at_xpath(int argc, VALUE *argv, VALUE self)
{
    VALUE rb_expr, handler, opts;
    rb_scan_args(argc, argv, "11:", &rb_expr, &handler, &opts);
    VALUE result = mkr_node_xpath_run(self, rb_expr, handler, mkr_ns_matching_lax(opts));
    if (rb_obj_is_kind_of(result, mkr_cNodeSet)) {
        return rb_funcall(result, rb_intern("first"), 0);
    }
    return result;
}

void
mkr_init_xpath(void)
{
    rb_define_singleton_method(mkr_cXPathContext, "new", mkr_xpath_ctx_s_new, -1);
    rb_define_method(mkr_cXPathContext, "evaluate", mkr_xpath_ctx_evaluate, -1);
    rb_define_method(mkr_cXPathContext, "register_namespace", mkr_xpath_ctx_register_ns, 2);
    rb_define_method(mkr_cXPathContext, "register_variable",  mkr_xpath_ctx_register_variable, 2);
    rb_define_method(mkr_cXPathContext, "node=", mkr_xpath_ctx_set_node, 1);

    rb_define_method(mkr_cNode, "xpath",    mkr_node_xpath,    -1);
    rb_define_method(mkr_cNode, "at_xpath", mkr_node_at_xpath, -1);
}
