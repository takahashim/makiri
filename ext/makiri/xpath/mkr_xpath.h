#ifndef MKR_XPATH_H
#define MKR_XPATH_H

#include <lexbor/dom/dom.h>
#include <lexbor/tag/const.h>
#include <stddef.h>

#include "../core/mkr_core.h" /* mkr_verified_text_t */

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Makiri's native XPath 1.0 engine. An original implementation that runs
 * directly on the Lexbor DOM with no libxml2/libxslt dependency (see NOTICE).
 *
 * Security: every evaluate enforces per-call budgets (op count, recursion
 * depth, node-set / string caps); any overrun fails closed with
 * MKR_XPATH_ERR_LIMIT - never a truncated or wrong result.
 */

typedef enum {
  MKR_XPATH_TYPE_NODESET = 0,
  MKR_XPATH_TYPE_STRING,
  MKR_XPATH_TYPE_NUMBER,
  MKR_XPATH_TYPE_BOOLEAN
} mkr_xpath_type_t;

typedef enum {
  MKR_XPATH_OK = 0,
  MKR_XPATH_ERR_NOT_IMPLEMENTED,
  MKR_XPATH_ERR_SYNTAX,
  MKR_XPATH_ERR_TYPE,
  MKR_XPATH_ERR_RUNTIME,
  MKR_XPATH_ERR_INTERNAL,
  /* Distinct codes so callers / tests can verify which guard tripped. */
  MKR_XPATH_ERR_OOM,
  MKR_XPATH_ERR_LIMIT
} mkr_xpath_status_t;

typedef struct mkr_xpath_context_s mkr_xpath_context_t;

typedef struct {
  mkr_xpath_type_t type;
  union {
    struct {
      /* Representation-agnostic node pointers: lxb_dom_node_t* for an HTML
       * document, mkr_xml_node_t* for an XML one. The glue knows the kind and
       * casts; the engine boundary stays neutral. */
      void **nodes;
      size_t count;
    } nodeset;
    mkr_owned_text_t string; /* owned; valid when type == MKR_XPATH_TYPE_STRING */
    double number;
    int boolean;
  } u;
} mkr_xpath_value_t;

typedef struct {
  mkr_xpath_status_t status;
  char *message; /* heap-allocated, freed with mkr_xpath_error_free */
} mkr_xpath_error_t;

/* Create a context for +node+ in +doc+ (representation-agnostic void* - the glue
 * passes lxb_dom_* for HTML, mkr_xml_* for XML; the engine kind is selected via
 * mkr_xpath_set_engine_kind). */
mkr_xpath_context_t *mkr_xpath_context_new(void *doc, void *node);
void                mkr_xpath_context_free(mkr_xpath_context_t *ctx);

int  mkr_xpath_register_ns(mkr_xpath_context_t *ctx, mkr_verified_text_t prefix, mkr_verified_text_t uri);
int  mkr_xpath_register_variable_string(mkr_xpath_context_t *ctx, mkr_verified_text_t name, mkr_verified_text_t value);

/*
 * Custom function fallback. When a function call's (namespace URI,
 * local name) does not match any built-in, the evaluator delegates
 * the entire dispatch to the registered resolver (if any). The
 * resolver is responsible for:
 *   - looking the function up,
 *   - evaluating it against args (already evaluated by the engine),
 *   - filling *out, or
 *   - signalling NOT_FOUND so the engine can raise.
 *
 * Return convention:
 *    0  = function found AND successfully evaluated (out populated)
 *   -1  = function found but evaluation failed (err populated)
 *   +1  = function NOT found; caller raises MKR_XPATH_ERR_RUNTIME
 *
 * args/out and the value lifetimes follow the same rules as built-in
 * functions: the engine owns args (frees them after the call) and
 * the resolver populates out with values whose ownership is then
 * transferred to the engine.
 *
 * The Ruby handler bridge installs a resolver before each evaluate()
 * call and unregisters it (NULL) afterwards. Reading user_data back
 * from inside the resolver is done via mkr_xpath_get_user_data(ctx).
 */
struct mkr_xpath_value_s_internal;  /* layout in mkr_xpath_internal.h */
typedef int (*mkr_func_resolver_t)(void                            *user_data,
                                  mkr_xpath_context_t              *ctx,
                                  void                            *self_node, /* lxb_dom_node_t * */
                                  size_t                           self_pos,
                                  size_t                           self_size,
                                  const char                      *ns_uri,
                                  const char                      *local_name,
                                  void                            *args,      /* mkr_val_t * */
                                  size_t                           nargs,
                                  void                            *out,       /* mkr_val_t * */
                                  mkr_xpath_error_t                *err);

void  mkr_xpath_context_set_user_data(mkr_xpath_context_t *ctx, void *user_data);
void *mkr_xpath_get_user_data       (mkr_xpath_context_t *ctx);
void  mkr_xpath_set_func_resolver   (mkr_xpath_context_t *ctx, mkr_func_resolver_t resolver);

/*
 * Element-index hooks, injected by the glue layer alongside an opaque index
 * pointer. The engine never sees the index's concrete type - it only calls
 * back through these, exactly as it calls Ruby only through the func resolver
 * above. This keeps the engine free of the lexbor_compat headers.
 *
 *   lookup(index, tag_id, &count) -> the document-ordered bucket of elements
 *     whose tag id == tag_id (count via *count), or NULL / *count == 0.
 *   has_foreign(index) -> nonzero if the document contains any non-HTML-
 *     namespace element (the //tag fast path is sound only for pure HTML).
 */
typedef lxb_dom_node_t *const *(*mkr_tag_index_lookup_t)(const void *index,
                                                         lxb_tag_id_t tag_id,
                                                         size_t *count);
typedef int (*mkr_tag_index_foreign_t)(const void *index);

/* Install the borrowed document-level element index plus its lookup hooks, used
 * to answer `//tag` from the document without a tree walk. Pass index == NULL
 * (or NULL hooks) to disable. The index must outlive every evaluation on ctx. */
void  mkr_xpath_context_set_element_index(mkr_xpath_context_t *ctx, void *index,
                                          mkr_tag_index_lookup_t lookup,
                                          mkr_tag_index_foreign_t has_foreign);

/* XML element-name index hooks (string-keyed; the HTML tag-id index above is
 * integer-keyed). get(owner) lazily builds + caches the index on the owning
 * document and returns it (or NULL on OOM -> the engine walks); lookup returns
 * the document-ordered bucket for (local, ns_uri). Node pointers are void*. */
typedef void *(*mkr_name_index_get_t)(void *owner);
typedef void *const *(*mkr_name_index_lookup_t)(const void *index,
                                                const char *local, size_t local_len,
                                                const char *ns_uri, size_t ns_uri_len,
                                                size_t *count);
void  mkr_xpath_context_set_name_index(mkr_xpath_context_t *ctx, void *owner,
                                       mkr_name_index_get_t get,
                                       mkr_name_index_lookup_t lookup);


void mkr_xpath_value_clear(mkr_xpath_value_t *v);
void mkr_xpath_error_clear(mkr_xpath_error_t *e);

/* ------------------------------------------------------------------ *
 * Engine-client API: parser, compiled-AST evaluator, context accessors
 * and the internal value model. The Ruby glue (the engine's client) drives the
 * engine through these and bridges custom-function handlers, so they live on
 * this public boundary -- representation-neutral (node pointers are void*; the
 * glue casts to the document's kind) and free of the MKR_DOM_* monomorphization
 * machinery in mkr_xpath_internal.h, which the glue never includes. The engine
 * itself reaches them through internal.h, which includes this header.
 * ------------------------------------------------------------------ */

/* Per-evaluate budgets + live counters. Filled by
 * mkr_xpath_limits_init_defaults; every overrun fails closed with
 * MKR_XPATH_ERR_LIMIT. */
typedef struct {
  /* Static budgets. */
  size_t max_expr_bytes;
  size_t max_ast_nodes;
  size_t max_steps;
  size_t max_predicates;
  size_t max_function_args;
  size_t max_nodeset_size;
  size_t max_eval_ops;
  size_t max_string_bytes;
  size_t max_recursion_depth;
  /* Live counters. */
  size_t ast_nodes;
  size_t eval_ops;
  size_t recursion_depth;
} mkr_xpath_limits_t;

/* The compiled AST, opaque to clients (built by mkr_parse, freed by
 * mkr_node_free, run by the evaluator). */
typedef struct mkr_node_s mkr_node_t;

/* The engine's internal value -- the type a custom-function resolver receives
 * as its void* args / out (distinct from mkr_xpath_value_t, the result-boundary
 * type, which names the node array `nodes`). Node pointers are void*; the glue
 * handler bridge reads/writes them and casts to the document's representation. */
typedef struct {
  void  **items;
  size_t  count;
  size_t  capacity;
} mkr_nodeset_t;

typedef struct {
  mkr_xpath_type_t type;
  union {
    mkr_nodeset_t    nodeset;
    mkr_owned_text_t string;
    double           number;
    int              boolean;
  } u;
} mkr_val_t;

/* Parse an expression into a compiled AST (caller frees with mkr_node_free);
 * NULL on error (*err filled). 'limits' bounds the AST node count. */
mkr_node_t *mkr_parse(mkr_verified_text_t expr, mkr_xpath_limits_t *limits, mkr_xpath_error_t *err);
void        mkr_node_free(mkr_node_t *n);

/* Evaluate a compiled AST; fills *out_value / *out_error. _first is the
 * Node#at_xpath variant (a 0-or-1-node result without building the full set). */
int mkr_xpath_eval_compiled      (mkr_xpath_context_t *ctx, mkr_node_t *ast,
                                  mkr_xpath_value_t *out_value, mkr_xpath_error_t *out_error);
int mkr_xpath_eval_compiled_first(mkr_xpath_context_t *ctx, mkr_node_t *ast,
                                  mkr_xpath_value_t *out_value, mkr_xpath_error_t *out_error);

/* Select the monomorphized engine instance: 0 = HTML (lxb_dom), 1 = XML. */
void mkr_xpath_set_engine_kind(mkr_xpath_context_t *ctx, int kind);

/* Context accessors used by the glue (node pointers are void*). */
mkr_xpath_limits_t *mkr_ctx_limits           (mkr_xpath_context_t *ctx);
void               *mkr_ctx_document         (mkr_xpath_context_t *ctx);
void                mkr_ctx_set_node         (mkr_xpath_context_t *ctx, void *node);
void                mkr_ctx_set_unprefixed_lax(mkr_xpath_context_t *ctx, int lax);

/* Error helpers (fill *err; never raise). */
void mkr_err_set (mkr_xpath_error_t *err, mkr_xpath_status_t status, const char *msg);
void mkr_err_setf(mkr_xpath_error_t *err, mkr_xpath_status_t status, const char *fmt, ...);

/* Node-set builder + value setter, for the custom-function handler bridge. */
void mkr_nodeset_init (mkr_nodeset_t *ns);
int  mkr_nodeset_push (mkr_nodeset_t *ns, void *node, mkr_xpath_limits_t *limits, mkr_xpath_error_t *err);
void mkr_nodeset_clear(mkr_nodeset_t *ns);
int  mkr_val_set_borrowed_text_copy(mkr_val_t *v, mkr_borrowed_text_t text,
                                    mkr_xpath_error_t *err, const char *what);

#ifdef __cplusplus
}
#endif

#endif /* MKR_XPATH_H */
