#include "mkr_xpath.h"
#include "mkr_xpath_internal.h"
#include "../core/mkr_safe.h"

#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/*
 * Native XPath engine — top-level: context lifetime, namespace + variable
 * registries, the eval entry point that drives parser + evaluator.
 *
 * Phase 1 builds the engine up to a working subset (see plan file).
 */

typedef struct {
  char *prefix;
  size_t prefix_len;
  char *uri;
  size_t uri_len;
} mkr_ns_entry_t;

typedef struct {
  char *prefix; /* may be NULL for default namespace */
  size_t prefix_len;
  char *name;
  size_t name_len;
  char *value;
  size_t value_len;
} mkr_var_entry_t;

struct mkr_xpath_context_s {
  lxb_dom_document_t *doc;
  lxb_dom_node_t     *node;

  mkr_ns_entry_t *ns;
  size_t         ns_count;
  size_t         ns_cap;

  mkr_var_entry_t *vars;
  size_t          vars_count;
  size_t          vars_cap;

  mkr_xpath_limits_t limits;

  /* Custom function resolver (set by the Ruby handler bridge during a
   * single evaluate() call, cleared after). */
  void              *user_data;
  mkr_func_resolver_t func_resolver;

  /* Per-evaluate string-value cache. Lives across the whole evaluate()
   * call; nested evaluates snapshot+restore via mkr_str_cache_truncate. */
  mkr_str_cache_t    str_cache;

  /* Per-evaluate document-order index (lazy). */
  mkr_doc_order_index_t order_index;

  /* Borrowed document-level element index (tag id -> elements) plus the lookup
   * hooks for it, injected by the glue layer before evaluation (the engine
   * never sees the index's concrete type). element_index == NULL or NULL hooks
   * disable the //tag fast path; the engine then falls back to tree walks. */
  void                    *element_index;
  mkr_tag_index_lookup_t   tag_lookup;
  mkr_tag_index_foreign_t  tag_has_foreign;

  /* Namespace-matching policy for UNPREFIXED name tests. 0 (default) =
   * strict/HTML5-faithful: an unprefixed name resolves in the HTML namespace
   * (matches HTML-namespace or no-namespace nodes only; foreign SVG/MathML
   * needs a prefix). 1 = lax: ignore namespace, match by local name. Prefixed
   * tests and wildcards are unaffected. Set by the glue from the
   * namespace_matching: option. */
  int unprefixed_lax;
};

mkr_doc_order_index_t *
mkr_ctx_order_index(mkr_xpath_context_t *ctx)
{
  return ctx ? &ctx->order_index : NULL;
}

void
mkr_xpath_context_set_element_index(mkr_xpath_context_t *ctx, void *index,
                                    mkr_tag_index_lookup_t lookup,
                                    mkr_tag_index_foreign_t has_foreign)
{
  if (ctx == NULL) return;
  ctx->element_index   = index;
  ctx->tag_lookup      = lookup;
  ctx->tag_has_foreign = has_foreign;
}

void *
mkr_ctx_element_index(mkr_xpath_context_t *ctx)
{
  return ctx ? ctx->element_index : NULL;
}

mkr_tag_index_lookup_t
mkr_ctx_tag_lookup(mkr_xpath_context_t *ctx)
{
  return ctx ? ctx->tag_lookup : NULL;
}

mkr_tag_index_foreign_t
mkr_ctx_tag_has_foreign(mkr_xpath_context_t *ctx)
{
  return ctx ? ctx->tag_has_foreign : NULL;
}

mkr_str_cache_t *
mkr_ctx_str_cache(mkr_xpath_context_t *ctx)
{
  return ctx ? &ctx->str_cache : NULL;
}

void
mkr_xpath_context_set_user_data(mkr_xpath_context_t *ctx, void *user_data)
{
  if (ctx) ctx->user_data = user_data;
}

void *
mkr_xpath_get_user_data(mkr_xpath_context_t *ctx)
{
  return ctx ? ctx->user_data : NULL;
}

void
mkr_xpath_set_func_resolver(mkr_xpath_context_t *ctx, mkr_func_resolver_t resolver)
{
  if (ctx) ctx->func_resolver = resolver;
}

mkr_func_resolver_t
mkr_ctx_func_resolver(mkr_xpath_context_t *ctx)
{
  return ctx ? ctx->func_resolver : NULL;
}

mkr_xpath_limits_t *
mkr_ctx_limits(mkr_xpath_context_t *ctx)
{
  return ctx ? &ctx->limits : NULL;
}

void
mkr_ctx_set_unprefixed_lax(mkr_xpath_context_t *ctx, int lax)
{
  if (ctx) ctx->unprefixed_lax = lax ? 1 : 0;
}

int
mkr_ctx_unprefixed_lax(mkr_xpath_context_t *ctx)
{
  return ctx ? ctx->unprefixed_lax : 0;
}

/* ---------- limits ---------- */

void
mkr_xpath_limits_init_defaults(mkr_xpath_limits_t *L)
{
  /* Defaults aimed safely above realistic queries; tighten if needed. */
  L->max_expr_bytes      = 64 * 1024;       /* 64 KB XPath string  */
  L->max_ast_nodes       = 100000;          /* ~100k AST nodes     */
  L->max_steps           = 256;             /* path step count     */
  L->max_predicates      = 64;              /* per-step predicates */
  L->max_function_args   = 64;
  L->max_nodeset_size    = 10 * 1000 * 1000; /* 10M nodes — large but bounded */
  L->max_eval_ops        = 50 * 1000 * 1000; /* 50M evaluator steps */
  L->max_string_bytes    = 64 * 1024 * 1024; /* 64 MB string-value */
  L->max_recursion_depth = 256;

  L->ast_nodes       = 0;
  L->eval_ops        = 0;
  L->recursion_depth = 0;
}

int
mkr_limit_ast_node(mkr_xpath_limits_t *L, mkr_xpath_error_t *err)
{
  if (++L->ast_nodes > L->max_ast_nodes) {
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT, "AST node limit exceeded (%zu)", L->max_ast_nodes);
    return -1;
  }
  return 0;
}

int
mkr_limit_eval_op(mkr_xpath_limits_t *L, mkr_xpath_error_t *err)
{
  if (++L->eval_ops > L->max_eval_ops) {
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT, "evaluation budget exceeded (%zu ops)", L->max_eval_ops);
    return -1;
  }
  return 0;
}

int
mkr_limit_recurse_enter(mkr_xpath_limits_t *L, mkr_xpath_error_t *err)
{
  if (++L->recursion_depth > L->max_recursion_depth) {
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT, "recursion depth limit exceeded (%zu)", L->max_recursion_depth);
    L->recursion_depth--; /* don't count this failed entry */
    return -1;
  }
  return 0;
}

void
mkr_limit_recurse_leave(mkr_xpath_limits_t *L)
{
  if (L->recursion_depth) L->recursion_depth--;
}

int
mkr_limit_check_nodeset_size(mkr_xpath_limits_t *L, size_t new_count, mkr_xpath_error_t *err)
{
  if (new_count > L->max_nodeset_size) {
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT, "nodeset size limit exceeded (%zu)", L->max_nodeset_size);
    return -1;
  }
  return 0;
}

int
mkr_limit_check_string_bytes(mkr_xpath_limits_t *L, size_t bytes, mkr_xpath_error_t *err)
{
  if (bytes > L->max_string_bytes) {
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT, "string size limit exceeded (%zu bytes)", L->max_string_bytes);
    return -1;
  }
  return 0;
}

int
mkr_limit_check_steps(mkr_xpath_limits_t *L, size_t nsteps, mkr_xpath_error_t *err)
{
  if (nsteps > L->max_steps) {
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT, "path step count limit exceeded (%zu)", L->max_steps);
    return -1;
  }
  return 0;
}

int
mkr_limit_check_predicates(mkr_xpath_limits_t *L, size_t npreds, mkr_xpath_error_t *err)
{
  if (npreds > L->max_predicates) {
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT, "predicate count limit exceeded (%zu)", L->max_predicates);
    return -1;
  }
  return 0;
}

int
mkr_limit_check_func_args(mkr_xpath_limits_t *L, size_t nargs, mkr_xpath_error_t *err)
{
  if (nargs > L->max_function_args) {
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT, "function argument count limit exceeded (%zu)", L->max_function_args);
    return -1;
  }
  return 0;
}

int
mkr_limit_check_expr_bytes(mkr_xpath_limits_t *L, size_t bytes, mkr_xpath_error_t *err)
{
  if (bytes > L->max_expr_bytes) {
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT, "expression too long (%zu bytes, max %zu)", bytes, L->max_expr_bytes);
    return -1;
  }
  return 0;
}

/* ---------- node qualified name ---------- */

/*
 * Borrowed qualified name for any node. HTML elements report their
 * lowercase local name (lxb_dom_element_qualified_name), matching
 * Makiri::Node#name and the lowercase-tag HTML data model the engine
 * assumes; every other node kind defers to lxb_dom_node_name.
 */
const lxb_char_t *
mkr_dom_node_name_qualified(lxb_dom_node_t *node, size_t *len)
{
  if (node == NULL) {
    if (len) *len = 0;
    return NULL;
  }
  if (node->type == LXB_DOM_NODE_TYPE_ELEMENT) {
    return lxb_dom_element_qualified_name(lxb_dom_interface_element(node), len);
  }
  return lxb_dom_node_name(node, len);
}

/* ---------- context ---------- */

mkr_xpath_context_t *
mkr_xpath_context_new(lxb_dom_document_t *doc, lxb_dom_node_t *node)
{
  mkr_xpath_context_t *ctx = mkr_callocarray(1, sizeof(*ctx));
  if (ctx == NULL) {
    return NULL;
  }
  ctx->doc = doc;
  ctx->node = node;
  mkr_xpath_limits_init_defaults(&ctx->limits);
  mkr_str_cache_init(&ctx->str_cache);
  mkr_doc_order_index_init(&ctx->order_index);
  return ctx;
}

void
mkr_xpath_context_free(mkr_xpath_context_t *ctx)
{
  if (ctx == NULL) return;
  for (size_t i = 0; i < ctx->ns_count; ++i) {
    free(ctx->ns[i].prefix);
    free(ctx->ns[i].uri);
  }
  free(ctx->ns);
  for (size_t i = 0; i < ctx->vars_count; ++i) {
    free(ctx->vars[i].prefix);
    free(ctx->vars[i].name);
    free(ctx->vars[i].value);
  }
  free(ctx->vars);
  mkr_str_cache_clear(&ctx->str_cache);
  mkr_doc_order_index_clear(&ctx->order_index);
  free(ctx);
}

/* Per-context registration caps. These bound an abusive Ruby loop that calls
 * register_namespace / register_variable without limit; far above any real use. */
#define MKR_MAX_NAMESPACES ((size_t)65536)
#define MKR_MAX_VARIABLES  ((size_t)65536)

static int
mkr_grow(void **base, size_t *cap, size_t elem)
{
  /* Grow by at least one element; mkr_grow_reserve handles the geometric,
   * overflow-checked sizing. */
  return mkr_grow_reserve(base, cap, *cap + 1, elem) == MKR_OK ? 0 : -1;
}

static int
mkr_text_eq(const char *a, size_t a_len, const char *b, size_t b_len)
{
  if (a == NULL || b == NULL) return a == b;
  return a_len == b_len && memcmp(a, b, a_len) == 0;
}

int
mkr_xpath_register_ns(mkr_xpath_context_t *ctx, mkr_valid_text_t prefix_t, mkr_valid_text_t uri_t)
{
  const char *prefix = prefix_t.ptr;
  const char *uri    = uri_t.ptr;
  if (ctx == NULL || prefix == NULL || uri == NULL) return -1;
  /* Replace if prefix already registered. */
  for (size_t i = 0; i < ctx->ns_count; ++i) {
    if (mkr_text_eq(ctx->ns[i].prefix, ctx->ns[i].prefix_len, prefix, prefix_t.len)) {
      char *new_uri = mkr_strndup(uri, uri_t.len);
      if (new_uri == NULL) return -1;
      free(ctx->ns[i].uri);
      ctx->ns[i].uri = new_uri;
      ctx->ns[i].uri_len = uri_t.len;
      return 0;
    }
  }
  if (ctx->ns_count >= MKR_MAX_NAMESPACES) return -1; /* registration cap */
  if (ctx->ns_count == ctx->ns_cap) {
    if (mkr_grow((void **)&ctx->ns, &ctx->ns_cap, sizeof(*ctx->ns)) != 0) return -1;
  }
  ctx->ns[ctx->ns_count].prefix_len = prefix_t.len;
  ctx->ns[ctx->ns_count].uri_len    = uri_t.len;
  ctx->ns[ctx->ns_count].prefix = mkr_strndup(prefix, prefix_t.len);
  ctx->ns[ctx->ns_count].uri    = mkr_strndup(uri, uri_t.len);
  if (ctx->ns[ctx->ns_count].prefix == NULL || ctx->ns[ctx->ns_count].uri == NULL) {
    free(ctx->ns[ctx->ns_count].prefix);
    free(ctx->ns[ctx->ns_count].uri);
    return -1;
  }
  ctx->ns_count++;
  return 0;
}

int
mkr_xpath_register_variable_string(mkr_xpath_context_t *ctx, mkr_valid_text_t name_t, mkr_valid_text_t value_t)
{
  const char *name  = name_t.ptr;
  const char *value = value_t.ptr;
  if (ctx == NULL || name == NULL) return -1;
  /* Phase 1: only unprefixed string variables. */
  for (size_t i = 0; i < ctx->vars_count; ++i) {
    if (ctx->vars[i].prefix == NULL &&
        mkr_text_eq(ctx->vars[i].name, ctx->vars[i].name_len, name, name_t.len)) {
      const char *src = value ? value : "";
      size_t src_len = value ? value_t.len : 0;
      char *new_value = mkr_strndup(src, src_len);
      if (new_value == NULL) return -1;
      free(ctx->vars[i].value);
      ctx->vars[i].value = new_value;
      ctx->vars[i].value_len = src_len;
      return 0;
    }
  }
  if (ctx->vars_count >= MKR_MAX_VARIABLES) return -1; /* registration cap */
  if (ctx->vars_count == ctx->vars_cap) {
    if (mkr_grow((void **)&ctx->vars, &ctx->vars_cap, sizeof(*ctx->vars)) != 0) return -1;
  }
  ctx->vars[ctx->vars_count].prefix = NULL;
  ctx->vars[ctx->vars_count].prefix_len = 0;
  ctx->vars[ctx->vars_count].name_len = name_t.len;
  ctx->vars[ctx->vars_count].name   = mkr_strndup(name, name_t.len);
  ctx->vars[ctx->vars_count].value_len = value ? value_t.len : 0;
  ctx->vars[ctx->vars_count].value  = mkr_strndup(value ? value : "", ctx->vars[ctx->vars_count].value_len);
  if (ctx->vars[ctx->vars_count].name == NULL || ctx->vars[ctx->vars_count].value == NULL) {
    free(ctx->vars[ctx->vars_count].name);
    free(ctx->vars[ctx->vars_count].value);
    return -1;
  }
  ctx->vars_count++;
  return 0;
}

const char *
mkr_ctx_lookup_ns(mkr_xpath_context_t *ctx, const char *prefix,
                  size_t prefix_len, size_t *out_uri_len)
{
  if (out_uri_len != NULL) *out_uri_len = 0;
  if (ctx == NULL || prefix == NULL) return NULL;
  for (size_t i = 0; i < ctx->ns_count; ++i) {
    if (ctx->ns[i].prefix != NULL &&
        ctx->ns[i].prefix_len == prefix_len &&
        memcmp(ctx->ns[i].prefix, prefix, ctx->ns[i].prefix_len) == 0) {
      if (out_uri_len != NULL) *out_uri_len = ctx->ns[i].uri_len;
      return ctx->ns[i].uri;
    }
  }
  return NULL;
}

int
mkr_ctx_lookup_variable_text(mkr_xpath_context_t *ctx, const char *prefix,
                             size_t prefix_len, const char *name,
                             size_t name_len, mkr_borrowed_text_t *out)
{
  if (out != NULL) *out = (mkr_borrowed_text_t){ NULL, 0 };
  if (ctx == NULL || name == NULL || out == NULL) return 0;
  for (size_t i = 0; i < ctx->vars_count; ++i) {
    int prefix_match;
    if (prefix == NULL) {
      prefix_match = (ctx->vars[i].prefix == NULL);
    } else {
      prefix_match = (ctx->vars[i].prefix != NULL
                      && ctx->vars[i].prefix_len == prefix_len
                      && memcmp(ctx->vars[i].prefix, prefix, ctx->vars[i].prefix_len) == 0);
    }
    if (prefix_match && ctx->vars[i].name != NULL
        && ctx->vars[i].name_len == name_len
        && memcmp(ctx->vars[i].name, name, ctx->vars[i].name_len) == 0) {
      *out = (mkr_borrowed_text_t){ ctx->vars[i].value, ctx->vars[i].value_len };
      return 1;
    }
  }
  return 0;
}

lxb_dom_node_t *
mkr_ctx_node(mkr_xpath_context_t *ctx)
{
  return ctx ? ctx->node : NULL;
}

void
mkr_ctx_set_node(mkr_xpath_context_t *ctx, lxb_dom_node_t *node)
{
  if (ctx) ctx->node = node;
}

lxb_dom_document_t *
mkr_ctx_document(mkr_xpath_context_t *ctx)
{
  return ctx ? ctx->doc : NULL;
}

/* ---------- error helpers ---------- */

void
mkr_err_set(mkr_xpath_error_t *err, mkr_xpath_status_t status, const char *msg)
{
  if (err == NULL) return;
  free(err->message);
  err->status  = status;
  err->message = msg ? mkr_strdup(msg) : NULL;
}

void
mkr_err_setf(mkr_xpath_error_t *err, mkr_xpath_status_t status, const char *fmt, ...)
{
  if (err == NULL) return;
  free(err->message);
  err->status = status;
  va_list ap;
  va_start(ap, fmt);
  char buf[512];
  vsnprintf(buf, sizeof(buf), fmt, ap);
  va_end(ap);
  err->message = mkr_strdup(buf);
}

void
mkr_xpath_error_clear(mkr_xpath_error_t *e)
{
  if (e == NULL) return;
  free(e->message);
  e->message = NULL;
  e->status  = MKR_XPATH_OK;
}

void
mkr_xpath_value_clear(mkr_xpath_value_t *v)
{
  if (v == NULL) return;
  switch (v->type) {
  case MKR_XPATH_TYPE_NODESET:
    free(v->u.nodeset.nodes);
    v->u.nodeset.nodes = NULL;
    v->u.nodeset.count = 0;
    break;
  case MKR_XPATH_TYPE_STRING:
    free(v->u.string);
    v->u.string = NULL;
    v->string_len = 0;
    break;
  default:
    break;
  }
}

/* ---------- eval entry ---------- */

/* Move an internal mkr_val_t into the public mkr_xpath_value_t (different
 * field name on the nodeset side). Ownership of the items array, the
 * string, etc. is transferred. */
static void
mkr_val_to_public(const mkr_val_t *v, mkr_xpath_value_t *out)
{
  out->type = v->type;
  switch (v->type) {
  case MKR_XPATH_TYPE_NODESET:
    out->u.nodeset.nodes = v->u.nodeset.items;
    out->u.nodeset.count = v->u.nodeset.count;
    break;
  case MKR_XPATH_TYPE_STRING:
    out->u.string = v->u.string;
    out->string_len = v->string_len;
    break;
  case MKR_XPATH_TYPE_NUMBER:
    out->u.number = v->u.number;
    break;
  case MKR_XPATH_TYPE_BOOLEAN:
    out->u.boolean = v->u.boolean;
    break;
  }
}

int
mkr_xpath_eval_compiled(mkr_xpath_context_t *ctx, mkr_node_t *ast,
                       mkr_xpath_value_t *out_value,
                       mkr_xpath_error_t *out_error)
{
  if (ctx == NULL || ast == NULL || out_value == NULL) {
    if (out_error) {
      mkr_err_set(out_error, MKR_XPATH_ERR_INTERNAL, "mkr_xpath_eval_compiled: bad arguments");
    }
    return -1;
  }
  mkr_xpath_error_t err = {0};

  /* Per-eval counters reset. ast_nodes is NOT reset because the AST is
   * already built and its budget was checked at parse time. */
  ctx->limits.eval_ops        = 0;
  ctx->limits.recursion_depth = 0;

  /* String-value cache snapshot. Nested eval (handler that calls back
   * into XPath on the same context) sees outer entries but anything it
   * adds is discarded on return — outer borrowed pointers stay valid. */
  size_t str_cache_snapshot = ctx->str_cache.count;
  /* Document-order index: outermost evaluate owns the lifecycle. If the
   * outer wasn't built, the inner's build (if any) gets cleared at this
   * exit; if the outer WAS built, leave the index alone so its sorts
   * still see it after the inner returns. */
  int order_was_built = ctx->order_index.built;

  mkr_val_t v = {0};
  int eval_rc = mkr_eval_ast(ctx, ast, &v, &err);
  mkr_str_cache_truncate(&ctx->str_cache, str_cache_snapshot);
  if (!order_was_built && ctx->order_index.built) {
    mkr_doc_order_index_clear(&ctx->order_index);
  }
  /* Clear memoized values regardless of success: they are valid only
   * within a single evaluate scope. Defensive — also clears on error. */
  mkr_node_clear_memos(ast);
  if (eval_rc != 0) {
    if (out_error) *out_error = err; else mkr_xpath_error_clear(&err);
    return -1;
  }
  mkr_val_to_public(&v, out_value);
  return 0;
}
