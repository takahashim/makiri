#ifndef MKR_XPATH_H
#define MKR_XPATH_H

#include <lexbor/dom/dom.h>
#include <stddef.h>

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
    char *string;
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

int  mkr_xpath_register_ns(mkr_xpath_context_t *ctx, const char *prefix, const char *uri);
int  mkr_xpath_register_variable_string(mkr_xpath_context_t *ctx, const char *name, const char *value);

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

/* Install the borrowed document-level element index (see lexbor_compat). The
 * engine uses it to answer `//tag` from the document without a tree walk; pass
 * NULL to disable. The index must outlive every evaluation made on ctx. */
void  mkr_xpath_context_set_element_index(mkr_xpath_context_t *ctx, void *index);

/*
 * Evaluate an XPath expression. On success returns 0 and fills *out_value.
 * On failure returns non-zero and fills *out_error (caller frees with
 * mkr_xpath_error_free). Either out_value or out_error must be non-NULL.
 */
int  mkr_xpath_eval(mkr_xpath_context_t *ctx,
                   const char *expr,
                   mkr_xpath_value_t *out_value,
                   mkr_xpath_error_t *out_error);

void mkr_xpath_value_clear(mkr_xpath_value_t *v);
void mkr_xpath_error_clear(mkr_xpath_error_t *e);

#ifdef __cplusplus
}
#endif

#endif /* MKR_XPATH_H */
