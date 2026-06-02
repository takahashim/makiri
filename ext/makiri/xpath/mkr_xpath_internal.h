#ifndef MKR_XPATH_INTERNAL_H
#define MKR_XPATH_INTERNAL_H

#include "mkr_xpath.h"
#include <lexbor/dom/dom.h>
#include <stddef.h>
#include <stdint.h>

/* Qualified name of any node as a borrowed Lexbor byte run. For HTML
 * elements this is the lowercase local name (matching Makiri::Node#name);
 * for other node kinds it falls back to lxb_dom_node_name. Defined in
 * mkr_xpath.c. Keeping the engine free of <ruby.h> / our glue headers. */
const lxb_char_t *mkr_dom_node_name_qualified(lxb_dom_node_t *node, size_t *len);

/*
 * Nokogiri-compatible namespace URIs. These must match the strings
 * registered from nl_xpath_context.c so prefixed names like
 * "nokogiri-builtin:css-class" resolve correctly via mkr_ctx_lookup_ns
 * and then through mkr_lookup_function.
 */
#define MKR_NS_NOKOGIRI_URI         "http://www.nokogiri.org/default_ns/ruby/extensions_functions"
#define MKR_NS_NOKOGIRI_BUILTIN_URI "https://www.nokogiri.org/default_ns/ruby/builtins"

/*
 * Evaluation limits and live counters. The struct lives on the context
 * so every helper can reach it. Defaults are picked to be safely above
 * any realistic real-world query but well below DoS territory. Callers
 * that need bigger budgets can raise the values later (after exposure
 * is reviewed).
 *
 * Every overrun returns MKR_XPATH_ERR_LIMIT — NEVER silent truncation
 * and NEVER an empty result.
 */
typedef struct {
  /* Static budgets — initialised in mkr_xpath_limits_init_defaults(). */
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
 * Namespace semantics — the rules the evaluator follows. The DEFAULT is
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
 *        <p>, but "//path" does NOT match an SVG <path> — foreign content
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
 *   6. Namespace axis. Currently NOT implemented — Lexbor has no
 *      DOM-level namespace nodes and the namespace axis is rarely
 *      used in HTML practice. The step driver returns NOT_IMPLEMENTED.
 *      A future Phase may synthesise virtual namespace nodes by
 *      walking the element's ancestor chain.
 */

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
  const char   *start;  /* pointer into the input buffer */
  size_t        len;
  double        num;    /* valid for MKR_TK_NUMBER */
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

/* Owned / borrowed engine text. Defined here (ahead of the AST) so node tests,
 * literals, and names can hold an mkr_owned_text_t directly. */
typedef struct {
  char  *ptr; /* owned; kept NUL-terminated at ptr[len] */
  size_t len; /* bytes, excluding the terminator */
} mkr_owned_text_t;

typedef struct {
  const char *ptr; /* borrowed; owner is value/cache/AST/Lexbor */
  size_t      len; /* bytes, excluding the terminator */
} mkr_borrowed_text_t;

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

typedef struct mkr_node_s mkr_node_t;

typedef struct {
  mkr_axis_t      axis;
  mkr_nodetest_t  test;
  mkr_node_t    **predicates;
  size_t         npredicates;
} mkr_step_t;

/* mkr_val_t / mkr_nodeset_t defined here so they can be embedded inside
 * mkr_node_s (for the memoization slot used by Perf 5 hoisting). */
typedef struct {
  lxb_dom_node_t **items;
  size_t           count;
  size_t           capacity;
} mkr_nodeset_t;

typedef struct {
  mkr_xpath_type_t type;
  union {
    mkr_nodeset_t    nodeset;
    mkr_owned_text_t string; /* owned; valid when type == MKR_XPATH_TYPE_STRING */
    double           number;
    int              boolean;
  } u;
} mkr_val_t;

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

/* Result-builder for the evaluator. */
void mkr_nodeset_init(mkr_nodeset_t *ns);
/* Push a node. Enforces max_nodeset_size when limits is non-NULL.
 * Returns 0 on success, -1 on OOM/LIMIT (err populated). */
int  mkr_nodeset_push(mkr_nodeset_t *ns, lxb_dom_node_t *node,
                     mkr_xpath_limits_t *limits, mkr_xpath_error_t *err);
void mkr_nodeset_clear(mkr_nodeset_t *ns);

void mkr_owned_text_init (mkr_owned_text_t *t);
void mkr_owned_text_clear(mkr_owned_text_t *t);
int  mkr_owned_text_from_bytes_copy(mkr_owned_text_t *out, const char *s, size_t len,
                                    mkr_xpath_error_t *err, const char *what);
int  mkr_owned_text_from_buf_steal(mkr_owned_text_t *out, mkr_buf_t *buf,
                                   mkr_xpath_error_t *err, const char *what);
void mkr_val_set_owned_string(mkr_val_t *v, char *s, size_t len);

/*
 * Sort entry points consult the context's per-evaluate document-order
 * index when available, which collapses comparisons to an O(1) hash
 * lookup + integer compare. Without the index they fall back to a
 * parent-chain walk (O(depth) per cmp). Attribute nodes are positioned
 * right after their owner element and before any descendant of that
 * element (XPath 1.0 §5.2/5.3). Cross-document comparisons are
 * implementation-defined; we return 0.
 */
void mkr_nodeset_sort_doc_order(struct mkr_xpath_context_s *ctx, mkr_nodeset_t *ns);
/* Sort then collapse adjacent identical pointers. Beats per-push
 * O(n) contains() checks for build-then-dedup patterns. */
void mkr_nodeset_unique_sorted(struct mkr_xpath_context_s *ctx, mkr_nodeset_t *ns);

/*
 * Per-evaluate document-order index. Built lazily on the first sort
 * that needs it, persists for the remainder of the evaluate call, and
 * is cleared at the OUTERMOST evaluate exit (nested evals inherit the
 * parent's build to avoid rebuilding mid-call).
 *
 * Open-addressing hash table keyed by lxb_dom_node_t pointer. Value
 * is a 32-bit ordinal that places attribute nodes immediately after
 * their owner element and before any descendants, matching the
 * comparator's existing semantics so behavior is preserved. Build /
 * lookup are file-static helpers in mkr_xpath_value.c; only the
 * lifecycle hooks below are public so the context can own the index.
 */
typedef struct {
  struct {
    const lxb_dom_node_t *node;  /* NULL = empty slot */
    uint32_t              ord;
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

/* Deep-copy src into dst. Used by the hoisting layer to return a fresh
 * value on memo hit. Returns 0 on success, -1 on OOM (err populated). */
int  mkr_val_clone(const mkr_val_t *src, mkr_val_t *dst, mkr_xpath_error_t *err);

/* Type coercions (XPath 1.0 §3.4 subset used by Phase 1). */
/*
 * Three-layer value conversion API.
 *
 *   Layer 1 — canonical entries (use these from the evaluator,
 *   functions, and the Ruby bridge):
 *
 *     mkr_node_string_text_or_fail   — node ─→ owned text
 *     mkr_val_to_owned_text_or_fail  — value ─→ owned text
 *     mkr_val_to_number_or_fail      — value ─→ double (via *out)
 *
 *   These thread the active limits and an err pointer so OOM and
 *   max_string_bytes overruns surface as MKR_XPATH_ERR_OOM /
 *   MKR_XPATH_ERR_LIMIT instead of silently producing a short or
 *   truncated result.
 *
 *   Layer 2 — _unchecked helpers (internal-only). Pure scalar
 *   conversions with no allocation cap and no err channel. Safe to
 *   call only on values that are guaranteed not to need limit
 *   enforcement (e.g. an already-extracted string, or two non-nodeset
 *   scalars from arithmetic). Calling these on a NODESET is
 *   correctness-wise OK but performs unchecked allocation; new code
 *   should prefer the _or_fail variants.
 *
 *   Layer 3 — internal builders (file-static in mkr_xpath_value.c):
 *   node_string_value_inner, append_text_content, etc. Never called
 *   from outside the value layer.
 *
 *   mkr_val_to_boolean has no allocation path, so there is no
 *   _or_fail counterpart — the single entry is correct.
 */
int mkr_node_string_text_or_fail(const lxb_dom_node_t *node,
                                mkr_xpath_limits_t *limits,
                                mkr_xpath_error_t *err,
                                mkr_owned_text_t *out);
int mkr_val_to_owned_text_or_fail(const mkr_val_t *v,
                                  mkr_xpath_limits_t *limits,
                                  mkr_xpath_error_t *err,
                                  mkr_owned_text_t *out);
int   mkr_val_to_number_or_fail(const mkr_val_t *v,
                               mkr_xpath_limits_t *limits,
                               mkr_xpath_error_t *err,
                               double *out);
int   mkr_val_to_boolean(const mkr_val_t *v);

/* Internal _unchecked helpers (Layer 2). Documented above. */
char   *mkr_val_to_string_unchecked        (const mkr_val_t *v);
double  mkr_val_to_number_unchecked        (const mkr_val_t *v);
char   *mkr_build_node_string_value_unchecked(const lxb_dom_node_t *node);

/* Parse a borrowed text as an XPath number. Parses t.ptr as a NUL-terminated
 * string (engine strings are NUL-terminated; t.len is advisory), returning NaN
 * for anything that is not a valid XPath number. Lets callers convert a
 * borrowed string (cache entry, value's own string) without wrapping it in a
 * temporary mkr_val_t. */
double  mkr_borrowed_text_to_number        (mkr_borrowed_text_t t);

/*
 * Per-evaluation string-value cache.
 *
 * The cache holds (node, string) entries owned by the context. During
 * a single evaluate() call, the same node may be visited many times —
 * for example by compare_eq across a node-set, or by fn_sum walking
 * every numeric leaf — and each visit otherwise re-walks the subtree
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
  lxb_dom_node_t *node;
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
   * truncate to a snapshot stays cheap); buckets is just an index into it. */
  uint32_t *buckets;
  size_t    bucket_cap; /* power of two, or 0 */
} mkr_str_cache_t;

void mkr_str_cache_init    (mkr_str_cache_t *c);
void mkr_str_cache_clear   (mkr_str_cache_t *c);
void mkr_str_cache_truncate(mkr_str_cache_t *c, size_t target_count);

/* Borrowed lookup. On hit *out is filled from the cache. On miss the string is
 * computed via mkr_node_string_text_or_fail, inserted, then returned. Returns 0
 * on success, -1 on OOM/LIMIT. */
int  mkr_get_cached_node_text  (struct mkr_xpath_context_s *ctx,
                               lxb_dom_node_t            *node,
                               mkr_borrowed_text_t       *out,
                               mkr_xpath_error_t         *err);

/* Returns a pointer to the context's per-eval cache. Used by
 * eval_compiled to manage nested-eval snapshots. */
mkr_str_cache_t *mkr_ctx_str_cache(struct mkr_xpath_context_s *ctx);

/* ---------- error helpers ---------- */

void mkr_err_set(mkr_xpath_error_t *err, mkr_xpath_status_t status, const char *msg);
void mkr_err_setf(mkr_xpath_error_t *err, mkr_xpath_status_t status, const char *fmt, ...);

/* ---------- AST helpers ---------- */

void mkr_node_free(mkr_node_t *n);
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

/* ---------- parser entry ---------- */

/* On success returns a fresh AST root (caller frees with mkr_node_free).
 * On failure returns NULL and fills *err.
 *
 * 'limits' is required (non-NULL): the parser increments ast_nodes for
 * every node it allocates and rejects expressions that exceed any of
 * the configured budgets with MKR_XPATH_ERR_LIMIT. */
mkr_node_t *mkr_parse(mkr_valid_text_t expr, mkr_xpath_limits_t *limits, mkr_xpath_error_t *err);

/*
 * Evaluate a pre-parsed AST. Public counterpart to mkr_xpath_eval that
 * lets callers cache the AST across multiple evaluate() calls.
 * Per-evaluation live counters (eval_ops, recursion_depth) are reset
 * here; the cached ast_nodes count is NOT reset because the AST is
 * already built.
 */
int mkr_xpath_eval_compiled(struct mkr_xpath_context_s *ctx,
                           mkr_node_t                 *ast,
                           mkr_xpath_value_t          *out_value,
                           mkr_xpath_error_t          *out_error);

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

/* The context node and document — exposed to the evaluator. */
lxb_dom_node_t     *mkr_ctx_node(struct mkr_xpath_context_s *ctx);
void                mkr_ctx_set_node(struct mkr_xpath_context_s *ctx,
                                     lxb_dom_node_t *node);
lxb_dom_document_t *mkr_ctx_document(struct mkr_xpath_context_s *ctx);
mkr_xpath_limits_t  *mkr_ctx_limits (struct mkr_xpath_context_s *ctx);
mkr_func_resolver_t  mkr_ctx_func_resolver(struct mkr_xpath_context_s *ctx);

/* Namespace-matching policy for unprefixed name tests (see the struct field).
 * Default strict (0). lax (1) makes unprefixed tests namespace-agnostic. */
void mkr_ctx_set_unprefixed_lax(struct mkr_xpath_context_s *ctx, int lax);
int  mkr_ctx_unprefixed_lax(struct mkr_xpath_context_s *ctx);

/* ---------- function library ---------- */

typedef int (*mkr_func_impl_t)(struct mkr_xpath_context_s *ctx,
                              lxb_dom_node_t *self_node,
                              size_t self_pos,
                              size_t self_size,
                              mkr_val_t *args, size_t nargs,
                              mkr_val_t *out,
                              mkr_xpath_error_t *err);

/* Look up a built-in XPath 1.0 function by namespace + local name.
 * Returns NULL if unknown. ns_uri may be NULL for the default namespace
 * (XPath 1.0 built-ins live in the default namespace). */
mkr_func_impl_t mkr_lookup_function(const char *ns_uri, const char *local_name);

#endif /* MKR_XPATH_INTERNAL_H */
