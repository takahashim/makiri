#ifndef MKR_CSS_H
#define MKR_CSS_H

#include "mkr_xpath.h"   /* mkr_node_t, mkr_xpath_limits_t, mkr_xpath_error_t, mkr_verified_text_t */

#ifdef __cplusplus
extern "C" {
#endif

/*
 * CSS selector front-end for the native XPath engine.
 *
 * Rather than convert CSS to an XPath *string* and re-parse it, this lowers a
 * CSS selector directly into the engine's compiled AST (the same mkr_node_t that
 * mkr_parse produces), which is then run by mkr_xpath_eval_compiled[_first].
 * The whole evaluation - per-evaluate budgets, document order, dedup, namespace
 * resolution - is therefore shared with XPath, and CSS gains no new evaluator
 * opcodes: every supported selector lowers to existing PATH/step/predicate nodes
 * (name tests + contains/starts-with/substring/count/position/not + arithmetic).
 *
 * The CSS *parsing* reuses Lexbor's selector parser (which, unlike its matcher,
 * preserves name case), so this layer only walks the parsed selector list and
 * builds AST. The result AST is owned by the caller and freed with mkr_node_free,
 * exactly like an mkr_parse result.
 *
 * Namespaces (Nokogiri-compatible): a bare type selector binds to the document's
 * DEFAULT namespace when one is in scope. The glue collects the in-scope
 * namespaces (Ruby side, mirroring Nokogiri's `document.namespaces`), registers
 * them on the context so the engine resolves prefixes at eval, and registers the
 * default namespace under the synthetic prefix "xmlns" (Nokogiri's convention).
 * +default_prefix+ is that synthetic prefix ("xmlns") when a default namespace is
 * in scope, or NULL otherwise; lowering uses it to emit name tests:
 *   - bare `el`   -> prefix = default_prefix (when set), else no-namespace
 *   - `p|el`      -> prefix = "p"  (resolved by the engine; unbound -> error)
 *   - `*|el`      -> any namespace (nodetest wildcard + local-name() predicate)
 *   - `|el`       -> no namespace
 */
/* The synthetic prefix bound to the document's default namespace (Nokogiri's
 * convention). The glue registers this prefix -> default-ns URI on the context. */
#define MKR_CSS_DEFAULT_NS_PREFIX "xmlns"

typedef struct {
  const char *default_prefix; /* MKR_CSS_DEFAULT_NS_PREFIX when a default ns is in scope, else NULL */
} mkr_css_ns_t;

/*
 * Compile +selector+ into a freshly allocated AST (caller frees with
 * mkr_node_free). Returns NULL on error with *err filled:
 *   MKR_XPATH_ERR_SYNTAX        - malformed selector or an unsupported construct
 *                                 (jQuery extensions, pseudo-elements, the case
 *                                 modifier)
 *   MKR_XPATH_ERR_OOM / _LIMIT  - allocation failure / selector-complexity cap
 * +ns+ may be NULL (bare selectors then match no-namespace). +limits+ bounds the
 * AST node count.
 */
mkr_node_t *mkr_css_compile(mkr_verified_text_t selector,
                            const mkr_css_ns_t *ns,
                            mkr_xpath_limits_t *limits,
                            mkr_xpath_error_t  *err);

#ifdef __cplusplus
}
#endif

#endif /* MKR_CSS_H */
