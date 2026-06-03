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
 * MKR_XPATH_ERR_LIMIT — never a truncated or wrong result.
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
      lxb_dom_node_t **nodes;
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

mkr_xpath_context_t *mkr_xpath_context_new(lxb_dom_document_t *doc, lxb_dom_node_t *node);
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
 * pointer. The engine never sees the index's concrete type — it only calls
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


void mkr_xpath_value_clear(mkr_xpath_value_t *v);
void mkr_xpath_error_clear(mkr_xpath_error_t *e);

#ifdef __cplusplus
}
#endif

#endif /* MKR_XPATH_H */
