#ifndef MKR_XPATH_INTERNAL_H
#define MKR_XPATH_INTERNAL_H

#include "mkr_xpath.h"
#include <lexbor/dom/dom.h>
#include <stddef.h>
#include <stdint.h>

/* The concrete DOM node/element/attr/document types the engine instance is
 * compiled against (§2.5 monomorphization). Each instance binds these to its
 * representation BEFORE including this header (mkr_xpath_{html,xml}_prelude.h):
 * the HTML instance to Lexbor's lxb_dom, the XML instance to mkr_xml_node_t. The
 * default here is the neutral `void` - used by the representation-independent
 * translation units (driver / lexer / parser / shared engine helpers), which
 * only move node pointers around and never dereference them, so void* is exact
 * and neither representation is privileged as "the default". The MKR_NODE_*
 * field-access contract (mkr_xpath_node_access_*.h) is the matching binding. */
#ifndef MKR_DOM_NODE
#  define MKR_DOM_NODE     void
#  define MKR_DOM_ELEMENT  void
#  define MKR_DOM_ATTR     void
#  define MKR_DOM_DOCUMENT void
#endif

/*
 * Nokogiri-compatible namespace URIs. These must match the strings
 * registered from nl_xpath_context.c so prefixed names like
 * "nokogiri-builtin:css-class" resolve correctly via mkr_ctx_lookup_ns
 * and then through mkr_lookup_function.
 */
#define MKR_NS_NOKOGIRI_URI         "http://www.nokogiri.org/default_ns/ruby/extensions_functions"
#define MKR_NS_NOKOGIRI_BUILTIN_URI "https://www.nokogiri.org/default_ns/ruby/builtins"

/*
 * Evaluation limits and live counters. mkr_xpath_limits_t lives in mkr_xpath.h
 * (the glue reads/writes its fields); the init + the per-op check helpers below
 * are engine-internal. Every overrun returns MKR_XPATH_ERR_LIMIT - NEVER silent
 * truncation and NEVER an empty result.
 */
void mkr_xpath_limits_init_defaults(mkr_xpath_limits_t *L);

/* Internal helpers for the limits. Each returns 0 on success and -1
 * on overrun (with err populated). */
int  mkr_limit_ast_node    (mkr_xpath_limits_t *L, mkr_xpath_error_t *err);
int  mkr_limit_eval_op     (mkr_xpath_limits_t *L, mkr_xpath_error_t *err);
int  mkr_limit_recurse_enter(mkr_xpath_limits_t *L, mkr_xpath_error_t *err);
void mkr_limit_recurse_leave(mkr_xpath_limits_t *L);
int  mkr_limit_check_nodeset_size(mkr_xpath_limits_t *L, size_t new_count, mkr_xpath_error_t *err);
int  mkr_limit_check_string_bytes(mkr_xpath_limits_t *L, size_t bytes, mkr_xpath_error_t *err);
int  mkr_limit_check_steps      (mkr_xpath_limits_t *L, size_t nsteps, mkr_xpath_error_t *err);
int  mkr_limit_check_predicates (mkr_xpath_limits_t *L, size_t npreds, mkr_xpath_error_t *err);
int  mkr_limit_check_func_args  (mkr_xpath_limits_t *L, size_t nargs, mkr_xpath_error_t *err);
int  mkr_limit_check_expr_bytes (mkr_xpath_limits_t *L, size_t bytes, mkr_xpath_error_t *err);

/*
 * Internal definitions shared by the native XPath engine's
 * lexer / parser / evaluator / function library.
 */

/*
 * Namespace semantics - the rules the evaluator follows. The DEFAULT is
 * strict / HTML5-faithful: it matches browsers' document.evaluate and
 * Nokogiri::HTML5. A per-context/per-call opt-in (namespace_matching: :lax,
 * see mkr_ctx_unprefixed_lax) relaxes only the unprefixed element rule (§2)
 * for HTML4 / Nokogiri::HTML-style convenience.
 *
 *   1. Prefix resolution. A prefix in a NameTest is resolved against
 *      the XPath context's registry (built via XPathContext#register_ns).
 *      Unknown prefixes produce MKR_XPATH_ERR_RUNTIME at evaluation.
 *
 *   2. Unprefixed element name test ("local").
 *      - strict (default): resolves in the HTML namespace. Matches an
 *        element iff its local name matches AND its namespace is HTML
 *        (LXB_NS_HTML) or none (LXB_NS__UNDEF). So "//p" matches an HTML
 *        <p>, but "//path" does NOT match an SVG <path> - foreign content
 *        needs a prefix (or :lax). This is the browser / Nokogiri::HTML5
 *        behaviour. (Pure XPath 1.0 would require the null namespace only,
 *        which would break "//p" since Lexbor marks HTML elements LXB_NS_HTML;
 *        admitting LXB_NS_HTML is the HTML adaptation browsers also make.)
 *      - lax: namespace ignored; match by local name regardless (so "//path"
 *        finds the SVG path). The old HTML-friendly default, now opt-in.
 *
 *   3. Prefixed element name test ("prefix:local"). Matches an element
 *      iff its namespace URI (resolved via lxb_ns_by_id(doc->ns,
 *      node->ns)) equals the URI registered for prefix, AND the local
 *      name matches. Unaffected by the strict/lax mode.
 *
 *   4. Attribute name test. Unprefixed "local" matches by no-namespace
 *      local name in BOTH modes (the strict element rule does NOT apply to
 *      attributes): an attribute is in no namespace per the DOM even on a
 *      foreign element (e.g. `id` on an <svg>), so "//@id" matches it. The
 *      comparison uses the attribute's QUALIFIED name, so a prefixed foreign
 *      attribute (xlink:href) does not match the unprefixed "@href". For
 *      "prefix:local", URI + local name must both match. (Lexbor tags an
 *      attribute with its element's ns, which does not reflect the DOM, so
 *      attribute matching deliberately does not gate on node->ns.)
 *
 *   5. Wildcards. "*" with no prefix matches the principal node type
 *      of the axis regardless of namespace (foreign included), in both
 *      modes. "prefix:*" matches any node in the namespace bound to prefix.
 *
 *   6. Namespace axis. Currently NOT implemented - Lexbor has no
 *      DOM-level namespace nodes and the namespace axis is rarely
 *      used in HTML practice. The step driver returns NOT_IMPLEMENTED.
 *      A future Phase may synthesise virtual namespace nodes by
 *      walking the element's ancestor chain.
 */

/* ---------- engine text ---------- */

/* mkr_owned_text_t / mkr_borrowed_text_t and mkr_borrowed_text_from_owned() live in
 * core/mkr_core.h (included via mkr_xpath.h) so the public value type can hold
 * an owned text directly; the lexer token, node tests, literals, and names use
 * them too. */

/* Borrowed-text equality, NUL-safe (NULL only equals NULL). The single name/
 * value/registry/token comparison used across the engine. */
int mkr_borrowed_text_eq(mkr_borrowed_text_t a, mkr_borrowed_text_t b);

/* Pointer hash (SplitMix-style). Shared primitive: keys both the per-evaluate
 * document-order index and the string-value cache index. */
uint32_t mkr_pointer_hash(const void *p);

/* ---------- tokens ---------- */

typedef enum {
  MKR_TK_EOF = 0,
  MKR_TK_LPAREN, MKR_TK_RPAREN,
  MKR_TK_LBRACKET, MKR_TK_RBRACKET,
  MKR_TK_DOT, MKR_TK_DOTDOT,
  MKR_TK_AT, MKR_TK_COMMA, MKR_TK_PIPE,
  MKR_TK_SLASH, MKR_TK_DSLASH,
  MKR_TK_PLUS, MKR_TK_MINUS, MKR_TK_STAR,
  MKR_TK_EQ, MKR_TK_NE, MKR_TK_LT, MKR_TK_GT, MKR_TK_LE, MKR_TK_GE,
  MKR_TK_COLONCOLON,
  MKR_TK_DOLLAR,
  MKR_TK_NUMBER,      /* literal_num */
  MKR_TK_LITERAL,     /* literal_str (quoted) */
  MKR_TK_NAME,        /* NCName (no colon) */
  MKR_TK_QNAME,       /* prefix:local */
  /* operator names that are NAMEs in some contexts and operators in others
   * are resolved by the parser using XPath disambiguation rules; the lexer
   * emits MKR_TK_NAME and the parser checks the spelling. */
} mkr_tok_kind_t;

typedef struct {
  mkr_tok_kind_t kind;
  mkr_borrowed_text_t text; /* borrowed slice of the input buffer */
  double         num;       /* valid for MKR_TK_NUMBER */
} mkr_token_t;

typedef struct {
  const char *src;
  const char *cur;
  mkr_token_t  tok;   /* current (peeked) token */
  int         good;
} mkr_lexer_t;

void mkr_lexer_init(mkr_lexer_t *L, const char *src, mkr_xpath_error_t *err);
int  mkr_lexer_advance(mkr_lexer_t *L, mkr_xpath_error_t *err);
/* Token spelling helpers (string equality with a compile-time literal). */
int  mkr_tok_is_word_len(const mkr_token_t *t, const char *word, size_t word_len);
#define mkr_tok_is_word_lit(t, word) \
  mkr_tok_is_word_len((t), (word), sizeof(word) - 1)

/* ---------- AST ---------- */

typedef enum {
  MKR_AXIS_CHILD = 0,
  MKR_AXIS_DESCENDANT,
  MKR_AXIS_PARENT,
  MKR_AXIS_ANCESTOR,
  MKR_AXIS_FOLLOWING_SIBLING,
  MKR_AXIS_PRECEDING_SIBLING,
  MKR_AXIS_FOLLOWING,
  MKR_AXIS_PRECEDING,
  MKR_AXIS_ATTRIBUTE,
  MKR_AXIS_NAMESPACE,
  MKR_AXIS_SELF,
  MKR_AXIS_DESCENDANT_OR_SELF,
  MKR_AXIS_ANCESTOR_OR_SELF,
} mkr_axis_t;

typedef enum {
  MKR_NT_NAME = 0,    /* element/attribute name (possibly with prefix) */
  MKR_NT_WILDCARD,    /* * */
  MKR_NT_NODE,        /* node() */
  MKR_NT_TEXT,        /* text() */
  MKR_NT_COMMENT,     /* comment() */
  MKR_NT_PI,          /* processing-instruction([literal]) */
} mkr_nt_kind_t;

typedef struct {
  mkr_nt_kind_t kind;
  mkr_owned_text_t prefix;    /* .ptr may be NULL */
  mkr_owned_text_t local;     /* .ptr may be NULL for non-name tests */
  mkr_owned_text_t pi_target; /* for processing-instruction("target"); .ptr may be NULL */
} mkr_nodetest_t;

typedef enum {
  MKR_OP_OR = 0, MKR_OP_AND,
  MKR_OP_EQ, MKR_OP_NE,
  MKR_OP_LT, MKR_OP_GT, MKR_OP_LE, MKR_OP_GE,
  MKR_OP_ADD, MKR_OP_SUB,
  MKR_OP_MUL, MKR_OP_DIV, MKR_OP_MOD,
  MKR_OP_UNION,
} mkr_op_t;

typedef enum {
  MKR_NK_LITERAL_STR = 0,
  MKR_NK_LITERAL_NUM,
  MKR_NK_VARREF,
  MKR_NK_FNCALL,
  MKR_NK_UNARY,       /* unary minus */
  MKR_NK_BINOP,
  MKR_NK_PATH,        /* steps with optional leading '/' */
  MKR_NK_FILTER,      /* PrimaryExpr Predicate*  optionally followed by /Path */
} mkr_nk_t;

/* mkr_node_t (opaque), mkr_nodeset_t and mkr_val_t live in mkr_xpath.h - they
 * are embedded in mkr_node_s below (the memoization slot) and handed to the glue
 * via the custom-function resolver, so they sit on the public boundary. */

typedef struct {
  mkr_axis_t      axis;
  mkr_nodetest_t  test;
  mkr_node_t    **predicates;
  size_t         npredicates;
} mkr_step_t;

struct mkr_node_s {
  mkr_nk_t kind;
  /*
   * Hoisting state. mark_context_independent() (called once per parse)
   * sets is_context_independent=1 on subtrees that depend on neither
   * the context node nor any variable/handler. During eval, the first
   * visit to such a node stores its value in memo_value and sets
   * memoized=1; subsequent visits within the same evaluate clone it
   * back instead of re-evaluating. Cleared by mkr_node_clear_memos at
   * the outermost evaluate's exit.
   *
   * The conservative classification used initially:
   *   CI = literal / absolute-path / unary or binop of CI children /
   *        FNCALL with prefix=NULL and name in a fixed pure-builtin
   *        list (count, string-length, number, boolean, not, floor,
   *        ceiling, round, sum, concat, starts-with, contains,
   *        substring(-before/-after), translate, true, false), all
   *        args CI.
   *   non-CI = variable / filter / relative path / FNCALL outside that
   *           set (handler-routed, position/last/id/lang/0-arg context
   *           consumers, etc.).
   */
  uint8_t  is_context_independent;
  uint8_t  memoized;
  /* Cached value when memoized=1. Owned by the node; freed by
   * mkr_node_clear_memos and mkr_node_free. */
  mkr_val_t memo_value;
  union {
    mkr_owned_text_t literal;
    double literal_num;
    struct {
      mkr_owned_text_t prefix;
      mkr_owned_text_t name;
    } varref;
    struct {
      mkr_owned_text_t prefix;
      mkr_owned_text_t name;
      mkr_node_t     **args;
      size_t           nargs;
    } fncall;
    struct {
      mkr_node_t *expr;
    } unary;
    struct {
      mkr_op_t    op;
      mkr_node_t *lhs;
      mkr_node_t *rhs;
    } binop;
    struct {
      int        absolute;     /* starts at root */
      mkr_step_t *steps;
      size_t     nsteps;
    } path;
    struct {
      mkr_node_t  *expr;
      mkr_node_t **preds;
      size_t      npreds;
      mkr_step_t  *path_steps;  /* optional trailing path */
      size_t      npath;
    } filter;
  } u;
};

/* ---------- runtime values ---------- */

/* mkr_nodeset_init / mkr_nodeset_push / mkr_nodeset_clear live in mkr_xpath.h
 * (the handler bridge builds node-sets); they are shared (mkr_xpath_shared.c). */

void mkr_owned_text_init (mkr_owned_text_t *t);
void mkr_owned_text_clear(mkr_owned_text_t *t);
int  mkr_owned_text_from_borrowed_copy(mkr_owned_text_t *out, mkr_borrowed_text_t t,
                                       mkr_xpath_error_t *err, const char *what);

/* mkr_owned_text_from_buf_steal is file-static in the per-instance value body
 * (only the node string-value builders, in the engine TU, steal a buffer). */

/* Store an owned text value by transfer. After this call, +v+ owns +text.ptr+;
 * the caller must not clear +text+. Shared (pure). */
void mkr_val_set_owned_text(mkr_val_t *v, mkr_owned_text_t text);

/* mkr_val_set_borrowed_text_copy (the glue handler bridge's string setter) lives
 * in mkr_xpath.h. */

/*
 * Document-order sort/dedup of a node-set and the per-evaluate document-order
 * index's build/lookup are file-static in mkr_xpath_value_body.h (they
 * dereference nodes, so they are per-instance). Only the index lifecycle below
 * is shared, so the context can own the index without touching node internals.
 */

/*
 * Per-evaluate document-order index. Built lazily on the first sort
 * that needs it, persists for the remainder of the evaluate call, and
 * is cleared at the OUTERMOST evaluate exit (nested evals inherit the
 * parent's build to avoid rebuilding mid-call).
 *
 * Open-addressing hash table keyed by MKR_DOM_NODE pointer. Value
 * is a 32-bit ordinal that places attribute nodes immediately after
 * their owner element and before any descendants, matching the
 * comparator's existing semantics so behavior is preserved. Build /
 * lookup are file-static helpers in mkr_xpath_value_body.h; only the
 * lifecycle hooks below are public so the context can own the index.
 */
typedef struct {
  struct {
    const MKR_DOM_NODE *node;  /* NULL = empty slot */
    size_t                ord;   /* document-order ordinal (size_t: no 2^32 cap) */
  } *buckets;
  size_t cap;
  size_t count;
  int    built;
} mkr_doc_order_index_t;

void mkr_doc_order_index_init (mkr_doc_order_index_t *idx);
void mkr_doc_order_index_clear(mkr_doc_order_index_t *idx);

mkr_doc_order_index_t *mkr_ctx_order_index(struct mkr_xpath_context_s *ctx);

/* Borrowed document-level element index (tag id -> elements) and its injected
 * lookup hooks, or NULL when the //tag fast path is unavailable. */
void *mkr_ctx_element_index(struct mkr_xpath_context_s *ctx);
mkr_tag_index_lookup_t  mkr_ctx_tag_lookup(struct mkr_xpath_context_s *ctx);
mkr_tag_index_foreign_t mkr_ctx_tag_has_foreign(struct mkr_xpath_context_s *ctx);

void mkr_val_clear(mkr_val_t *v);

/* The value model's node-DEREFERENCING half is file-static in the per-instance
 * mkr_xpath_value_body.h (each compiled once per representation):
 *   mkr_val_clone, mkr_val_to_boolean, mkr_val_to_number_unchecked,
 *   mkr_borrowed_text_to_number, mkr_node_to_owned_text_or_fail,
 *   mkr_val_to_owned_text_or_fail, mkr_val_to_number_or_fail,
 *   mkr_nodeset_sort_doc_order, mkr_nodeset_unique_sorted,
 *   mkr_get_cached_node_text.
 * They reach across to the shared primitives declared in this header but are not
 * themselves shared (string(node-set), number(node-set), and doc-order all read
 * node fields). Only the evaluator and the function library call them, both in
 * the same engine translation unit. */

/*
 * Per-evaluation string-value cache.
 *
 * The cache holds (node, string) entries owned by the context. During
 * a single evaluate() call, the same node may be visited many times -
 * for example by compare_eq across a node-set, or by fn_sum walking
 * every numeric leaf - and each visit otherwise re-walks the subtree
 * and re-allocates the result.
 *
 * Lookup returns a BORROWED const char *: ownership stays with the
 * cache and lasts until mkr_str_cache_truncate(c, snapshot) restores
 * a previous state (the standard nested-eval discipline) or until
 * mkr_str_cache_clear() frees everything.
 *
 * Safety contract:
 *   - The XPath spec treats nodes as immutable during evaluation; we
 *     rely on that to key by pointer identity.
 *   - max_string_bytes / OOM are enforced at insert time. A cache miss
 *     that hits a limit fails closed; the entry is NOT added.
 *   - If a handler mutates the document via Ruby in the middle of
 *     evaluation, cached strings for affected nodes can go stale. This
 *     matches the existing "do not mutate during XPath" assumption.
 */
typedef struct {
  MKR_DOM_NODE *node;
  char           *str;
  size_t          len;
} mkr_str_cache_entry_t;

typedef struct {
  mkr_str_cache_entry_t *entries;
  size_t count;
  size_t cap;
  /* Open-addressing index: node pointer -> (entry index + 1); 0 = empty slot.
   * Keeps lookup/insert O(1) so per-node predicate comparisons over a large
   * node-set don't degrade to O(n^2). entries[] is the ordered store (so
   * truncate to a snapshot stays cheap); buckets is just an index into it.
   * size_t (not uint32_t) so the stored entry index can never truncate. */
  size_t   *buckets;
  size_t    bucket_cap; /* power of two, or 0 */
} mkr_str_cache_t;

void mkr_str_cache_init    (mkr_str_cache_t *c);
void mkr_str_cache_clear   (mkr_str_cache_t *c);
void mkr_str_cache_truncate(mkr_str_cache_t *c, size_t target_count);

/* Index bookkeeping (pure): insert one entry's slot, and rebuild the whole
 * index. Shared because the cache's pure lifecycle (truncate, above) and its
 * node-dereferencing insert (mkr_get_cached_node_text, file-static in
 * mkr_xpath_value_body.h) both drive the one open-addressing index. Callers
 * ensure room before mkr_str_cache_index_put; mkr_str_cache_reindex returns -1
 * on OOM. */
void mkr_str_cache_index_put(mkr_str_cache_t *c, size_t idx);
int  mkr_str_cache_reindex  (mkr_str_cache_t *c, size_t bucket_cap);

/* Returns a pointer to the context's per-eval cache. Used by
 * eval_compiled to manage nested-eval snapshots. */
mkr_str_cache_t *mkr_ctx_str_cache(struct mkr_xpath_context_s *ctx);

/* mkr_err_set / mkr_err_setf (error helpers) live in mkr_xpath.h. */

/* ---------- AST helpers ---------- */

/* mkr_node_free lives in mkr_xpath.h (the glue frees a parsed AST). */
void mkr_step_clear(mkr_step_t *s);

/* Hoisting / memoization. After parse, the parser entry calls
 * mkr_mark_context_independent on the AST root once. During eval,
 * eval_node consults the memo slot before re-evaluating CI subtrees.
 * mkr_node_clear_memos is called at the outermost evaluate exit. */
void mkr_mark_context_independent(mkr_node_t *n);
void mkr_node_clear_memos        (mkr_node_t *n);

/* Peephole optimisation. Runs once after parse. Currently fuses
 * (descendant-or-self::node() with no predicates) + (child::X with no
 * predicates) into a single descendant::X step. Equivalent per XPath
 * 1.0 §2.5, but moves the intermediate "all-nodes" set out of the way
 * so //X//Y avoids materialising the cross-product. */
void mkr_apply_peephole(mkr_node_t *n);

/* mkr_parse (parser entry) and mkr_xpath_set_engine_kind both live in
 * mkr_xpath.h - the glue parses, selects the instance, then evaluates. */

/* Structural self-test of the XML engine instance (Makiri.__c_selftest): parses
 * a small XML document and runs a few XPath queries through the _xml engine,
 * asserting node-set counts / namespace matching. Returns 0 or a 1-based index. */
int mkr_xml_xpath_selftest(void);

/* mkr_xpath_eval_compiled / mkr_xpath_eval_compiled_first (the AST evaluator
 * entries the glue calls) live in mkr_xpath.h. */

/* at_xpath() fast path: recognise a first-descendant-match shape and walk the
 * subtree in document order, stopping at the first match (out_node = match or
 * NULL). Returns 1 when handled, 0 when the shape is not recognised, -1 when the
 * per-evaluate op budget is exceeded during the walk (*err set). Each visited
 * node is charged to max_eval_ops so the fast path is bounded like the full
 * evaluator. */
int mkr_try_first_match(struct mkr_xpath_context_s *ctx, const mkr_node_t *ast,
                        MKR_DOM_NODE **out_node, mkr_xpath_error_t *err);

/* ---------- evaluator entry ---------- */

struct mkr_xpath_context_s; /* opaque to evaluator clients */

/* Evaluate an AST against the context. The "self" node is the context
 * node carried by ctx. On success fills *out and returns 0. */
int mkr_eval_ast(struct mkr_xpath_context_s *ctx,
                const mkr_node_t          *ast,
                mkr_val_t                 *out,
                mkr_xpath_error_t         *err);

/* Access the registry's variable lookup. Phase 1 only supports string
 * variables; returns NULL if not found. Caller must not free. */
int mkr_ctx_lookup_variable_text(struct mkr_xpath_context_s *ctx,
                                 const char *prefix,
                                 size_t prefix_len,
                                 const char *name,
                                 size_t name_len,
                                 mkr_borrowed_text_t *out);

/* Access the registry's namespace lookup. Returns the URI for the given
 * prefix, or NULL. Caller must not free. */
const char *mkr_ctx_lookup_ns(struct mkr_xpath_context_s *ctx,
                             const char *prefix,
                             size_t prefix_len,
                             size_t *out_uri_len);

/* The context node, exposed to the evaluator (the typed accessor). The glue-
 * facing accessors mkr_ctx_document / mkr_ctx_set_node / mkr_ctx_limits /
 * mkr_ctx_set_unprefixed_lax live in mkr_xpath.h (void* node pointers). */
MKR_DOM_NODE     *mkr_ctx_node(struct mkr_xpath_context_s *ctx);
mkr_func_resolver_t  mkr_ctx_func_resolver(struct mkr_xpath_context_s *ctx);

/* Namespace-matching policy read-back (the setter is in mkr_xpath.h). */
int  mkr_ctx_unprefixed_lax(struct mkr_xpath_context_s *ctx);

/* ---------- function library ---------- */

typedef int (*mkr_func_impl_t)(struct mkr_xpath_context_s *ctx,
                              MKR_DOM_NODE *self_node,
                              size_t self_pos,
                              size_t self_size,
                              mkr_val_t *args, size_t nargs,
                              mkr_val_t *out,
                              mkr_xpath_error_t *err);

/* The built-in function table and its lookup (mkr_lookup_function) are
 * file-static in the per-instance mkr_xpath_funcs_body.h: the evaluator resolves
 * through them within the same engine translation unit. */

#endif /* MKR_XPATH_INTERNAL_H */
