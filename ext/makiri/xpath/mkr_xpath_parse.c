#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"

#include <stdlib.h>
#include <string.h>

/*
 * XPath 1.0 recursive-descent parser. Produces an AST that the
 * evaluator walks. Lookahead is one token via the lexer; a small
 * number of constructs need two-token lookahead, which we do by
 * advancing and saving the original token before deciding.
 */

typedef struct {
  mkr_lexer_t         L;
  mkr_xpath_error_t  *err;
  mkr_xpath_limits_t *limits;
} mkr_parser_t;

#define TOK(P) ((P)->L.tok)

/* ---------- small helpers ---------- */

/* strndup-style helper. On OOM populates err with MKR_XPATH_ERR_OOM. */
static char *
P_strndup(mkr_parser_t *P, const char *s, size_t n)
{
  char *p = mkr_strndup(s, n);
  if (p == NULL) {
    mkr_err_set(P->err, MKR_XPATH_ERR_OOM, "out of memory in parser");
    return NULL;
  }
  return p;
}

/* strndup into an owned-text AST slot (node-test name, literal, varref/fncall
 * name). Returns 0 on success, -1 on OOM (P->err set, slot left {NULL,0}).
 * Callers MUST propagate the failure - a {NULL,0} slot left in the AST would
 * silently mis-compare at evaluation, so the parse fails closed instead. The
 * stored length lets the evaluator compare names without a per-node strlen.
 * `text` is a slice of the already-validated expr buffer, so the copy is valid. */
static int
P_fill_owned_text(mkr_parser_t *P, mkr_borrowed_text_t text, mkr_owned_text_t *out)
{
  out->ptr = P_strndup(P, text.ptr, text.len);
  if (out->ptr == NULL) { out->len = 0; return -1; }
  out->len = text.len;
  return 0;
}

static int
P_advance(mkr_parser_t *P)
{
  return mkr_lexer_advance(&P->L, P->err);
}

static int
P_eat(mkr_parser_t *P, mkr_tok_kind_t k, const char *what)
{
  if (TOK(P).kind != k) {
    mkr_err_setf(P->err, MKR_XPATH_ERR_SYNTAX, "expected %s", what);
    return -1;
  }
  return P_advance(P);
}

/* AST node allocator - the shared mkr_node_alloc with this parser's limits/err
 * (counts against max_ast_nodes, reports OOM and LIMIT distinctly). */
static mkr_node_t *
new_node(mkr_parser_t *P, mkr_nk_t kind)
{
  return mkr_node_alloc(P->limits, P->err, kind);
}

/* ---------- axis lookup ---------- */

static int
axis_by_name(const char *s, size_t n, mkr_axis_t *out)
{
#define A(name, val) do { if (mkr_bytes_eq(s, n, name, sizeof(name)-1)) { *out = val; return 1; } } while (0)
  A("child",                MKR_AXIS_CHILD);
  A("descendant",           MKR_AXIS_DESCENDANT);
  A("parent",               MKR_AXIS_PARENT);
  A("ancestor",             MKR_AXIS_ANCESTOR);
  A("following-sibling",    MKR_AXIS_FOLLOWING_SIBLING);
  A("preceding-sibling",    MKR_AXIS_PRECEDING_SIBLING);
  A("following",            MKR_AXIS_FOLLOWING);
  A("preceding",            MKR_AXIS_PRECEDING);
  A("attribute",            MKR_AXIS_ATTRIBUTE);
  A("namespace",            MKR_AXIS_NAMESPACE);
  A("self",                 MKR_AXIS_SELF);
  A("descendant-or-self",   MKR_AXIS_DESCENDANT_OR_SELF);
  A("ancestor-or-self",     MKR_AXIS_ANCESTOR_OR_SELF);
#undef A
  return 0;
}

static int
is_nodetype_name(const char *s, size_t n)
{
  return mkr_bytes_eq(s, n, "node", 4)
      || mkr_bytes_eq(s, n, "text", 4)
      || mkr_bytes_eq(s, n, "comment", 7)
      || mkr_bytes_eq(s, n, "processing-instruction", 22);
}

/* ---------- forward decls ---------- */

static mkr_node_t *parse_expr(mkr_parser_t *P);
static int        parse_relative_path(mkr_parser_t *P, mkr_step_t **steps, size_t *nsteps);
static int        parse_step(mkr_parser_t *P, mkr_step_t *out);
static int        parse_predicates(mkr_parser_t *P, mkr_node_t ***preds, size_t *npreds);
static mkr_node_t *parse_primary(mkr_parser_t *P);

/* ---------- step builder helpers ---------- */

static int
push_step(mkr_parser_t *P, mkr_step_t **steps, size_t *n, size_t *cap, mkr_step_t s)
{
  if (mkr_limit_check_steps(P->limits, *n + 1, P->err) != 0) return -1;
  {
    mkr_step_t *steps_p = *steps;
    if (mkr_grow_reserve((void **)&steps_p, cap, *n + 1, sizeof(*steps_p)) != MKR_OK) {
      mkr_err_set(P->err, MKR_XPATH_ERR_OOM, "out of memory growing step array");
      return -1;
    }
    *steps = steps_p;
  }
  (*steps)[(*n)++] = s;
  return 0;
}

static int
make_implicit_step(mkr_step_t *out, mkr_axis_t axis, mkr_nt_kind_t nt_kind)
{
  memset(out, 0, sizeof(*out));
  out->axis = axis;
  out->test.kind = nt_kind;
  return 0;
}

/* Parse a run of `('/' | '//') Step` continuations onto the step array,
 * expanding each `//` to an implicit descendant-or-self::node() step. TOK(P)
 * must be positioned at the (possible) leading separator; a non-separator token
 * makes this a no-op (zero iterations). On failure the steps pushed so far stay
 * in the array - the caller's owning node frees them (the path/filter node, via
 * mkr_node_free). This is the single home for the slash-step loop that the
 * relative-path, absolute `//`, and filter-expr trailing-path forms all share. */
static int
parse_step_tail(mkr_parser_t *P, mkr_step_t **steps, size_t *nsteps, size_t *cap)
{
  while (TOK(P).kind == MKR_TK_SLASH || TOK(P).kind == MKR_TK_DSLASH) {
    int dslash = (TOK(P).kind == MKR_TK_DSLASH);
    if (P_advance(P) != 0) return -1;
    if (dslash) {
      mkr_step_t implicit;
      make_implicit_step(&implicit, MKR_AXIS_DESCENDANT_OR_SELF, MKR_NT_NODE);
      if (push_step(P, steps, nsteps, cap, implicit) != 0) return -1;
    }
    mkr_step_t next = {0};
    if (parse_step(P, &next) != 0) return -1;
    if (push_step(P, steps, nsteps, cap, next) != 0) { mkr_step_clear(&next); return -1; }
  }
  return 0;
}

/* ---------- node-test parsing ---------- */

/* Split a QNAME token's text at the first ':' into +prefix+ (the part before)
 * and +local+ (the part after; a colon is always present in a QNAME token). 0 /
 * -1 (OOM). The one place function-call and variable-reference prefixes are
 * peeled off the QName. */
static int
P_fill_qname_split(mkr_parser_t *P, mkr_borrowed_text_t tok,
                   mkr_owned_text_t *prefix, mkr_owned_text_t *local)
{
  mkr_span_t sp = mkr_span(tok.ptr, tok.len);
  size_t colon;
  if (!mkr_span_find(&sp, ':', &colon)) colon = tok.len;
  if (P_fill_owned_text(P, mkr_borrowed_text(tok.ptr, colon), prefix) != 0) return -1;
  size_t loff = (colon < tok.len) ? colon + 1 : colon;   /* skip the ':' when present */
  return P_fill_owned_text(P, mkr_borrowed_text(tok.ptr + loff, tok.len - loff), local);
}

/* +saved+ is an already-consumed NAME token; TOK(P) is the token after it. If
 * +saved+ is a node-type keyword (node/text/comment/processing-instruction)
 * immediately followed by '(', parse the node-type test - capturing an optional
 * PI target literal - and eat the ')'; otherwise it is an NCName name test. Fills
 * *out. The single home of the node-type grammar, shared by parse_node_test's
 * NAME branch and parse_step_inner's axis-lookahead replay. */
static int
parse_nodetype_or_name(mkr_parser_t *P, mkr_token_t saved, mkr_nodetest_t *out)
{
  if (is_nodetype_name(saved.text.ptr, saved.text.len) && TOK(P).kind == MKR_TK_LPAREN) {
    if (P_advance(P) != 0) return -1;
    if      (mkr_bytes_eq(saved.text.ptr, saved.text.len, "node", 4))    out->kind = MKR_NT_NODE;
    else if (mkr_bytes_eq(saved.text.ptr, saved.text.len, "text", 4))    out->kind = MKR_NT_TEXT;
    else if (mkr_bytes_eq(saved.text.ptr, saved.text.len, "comment", 7)) out->kind = MKR_NT_COMMENT;
    else /* processing-instruction */ {
      out->kind = MKR_NT_PI;
      if (TOK(P).kind == MKR_TK_LITERAL) {
        if (P_fill_owned_text(P, TOK(P).text, &out->pi_target) != 0) return -1;
        if (P_advance(P) != 0) return -1;
      }
    }
    return P_eat(P, MKR_TK_RPAREN, "')' after node type test");
  }
  out->kind = MKR_NT_NAME;
  return P_fill_owned_text(P, saved.text, &out->local);
}

/*
 * Called with TOK(P) at the first token of the node test. On success
 * fills *out and leaves TOK(P) at the token after the node test.
 */
static int
parse_node_test(mkr_parser_t *P, mkr_axis_t axis, mkr_nodetest_t *out)
{
  (void)axis;
  memset(out, 0, sizeof(*out));

  if (TOK(P).kind == MKR_TK_STAR) {
    out->kind = MKR_NT_WILDCARD;
    return P_advance(P);
  }

  if (TOK(P).kind == MKR_TK_NAME) {
    /* Consume the NAME, then let the shared node-type grammar decide whether it
     * introduces a node-type test ('(' follows a keyword) or is a name test. */
    mkr_token_t saved = TOK(P);
    if (P_advance(P) != 0) return -1;
    return parse_nodetype_or_name(P, saved, out);
  }

  if (TOK(P).kind == MKR_TK_QNAME) {
    /* QName name test: `prefix:local` or `prefix:*` - split at the colon. */
    const char *s = TOK(P).text.ptr;
    size_t      n = TOK(P).text.len;
    mkr_span_t  sp = mkr_span(s, n);
    size_t      colon;
    if (!mkr_span_find(&sp, ':', &colon)) colon = n;
    if (P_fill_owned_text(P, mkr_borrowed_text(s, colon), &out->prefix) != 0) return -1;
    if (n - colon - 1 == 1 && s[colon + 1] == '*') {
      /* prefix:* - any element in the prefix's namespace. */
      out->kind = MKR_NT_WILDCARD;
    } else {
      out->kind  = MKR_NT_NAME;
      if (P_fill_owned_text(P, mkr_borrowed_text(s + colon + 1, n - colon - 1), &out->local) != 0) return -1;
    }
    return P_advance(P);
  }

  mkr_err_set(P->err, MKR_XPATH_ERR_SYNTAX, "expected node test");
  return -1;
}

/* ---------- predicates ---------- */

static int
parse_predicates(mkr_parser_t *P, mkr_node_t ***preds, size_t *npreds)
{
  *preds = NULL;
  *npreds = 0;
  size_t cap = 0;
  while (TOK(P).kind == MKR_TK_LBRACKET) {
    if (mkr_limit_check_predicates(P->limits, *npreds + 1, P->err) != 0) return -1;
    if (P_advance(P) != 0) return -1;
    mkr_node_t *e = parse_expr(P);
    if (e == NULL) return -1;
    if (P_eat(P, MKR_TK_RBRACKET, "']' to close predicate") != 0) {
      mkr_node_free(e);
      return -1;
    }
    mkr_node_t **preds_p = *preds;
    if (mkr_grow_reserve((void **)&preds_p, &cap, *npreds + 1, sizeof(*preds_p)) != MKR_OK) {
      mkr_node_free(e);
      mkr_err_set(P->err, MKR_XPATH_ERR_OOM, "out of memory growing predicate array");
      return -1;
    }
    *preds = preds_p;
    (*preds)[(*npreds)++] = e;
  }
  return 0;
}

/* ---------- step ---------- */

static int parse_step_inner(mkr_parser_t *P, mkr_step_t *out);

/* Parse one Step into *out. On failure *out is CLEARED here - a failing
 * parse_step_inner can leave a partially-built step behind (an owned name-test
 * text already strndup'd, predicates already pushed) and the callers all just
 * bail, so freeing the partial step in one place keeps every error path
 * leak-free without per-call-site cleanup. */
static int
parse_step(mkr_parser_t *P, mkr_step_t *out)
{
  if (parse_step_inner(P, out) == 0) {
    return 0;
  }
  mkr_step_clear(out);
  return -1;
}

static int
parse_step_inner(mkr_parser_t *P, mkr_step_t *out)
{
  memset(out, 0, sizeof(*out));

  /* Abbreviated steps. */
  if (TOK(P).kind == MKR_TK_DOT) {
    if (P_advance(P) != 0) return -1;
    out->axis = MKR_AXIS_SELF;
    out->test.kind = MKR_NT_NODE;
    return 0;
  }
  if (TOK(P).kind == MKR_TK_DOTDOT) {
    if (P_advance(P) != 0) return -1;
    out->axis = MKR_AXIS_PARENT;
    out->test.kind = MKR_NT_NODE;
    return 0;
  }

  /* AxisSpecifier: '@' or NAME '::'. */
  if (TOK(P).kind == MKR_TK_AT) {
    out->axis = MKR_AXIS_ATTRIBUTE;
    if (P_advance(P) != 0) return -1;
  } else if (TOK(P).kind == MKR_TK_NAME) {
    /* Could be axis::test or just a NameTest. Look ahead one token. */
    mkr_token_t saved = TOK(P);
    /* Peek: save and advance. */
    if (P_advance(P) != 0) return -1;
    if (TOK(P).kind == MKR_TK_COLONCOLON) {
      mkr_axis_t ax;
      if (!axis_by_name(saved.text.ptr, saved.text.len, &ax)) {
        mkr_err_setf(P->err, MKR_XPATH_ERR_SYNTAX, "unknown axis '%.*s'", (int)saved.text.len, saved.text.ptr);
        return -1;
      }
      out->axis = ax;
      if (P_advance(P) != 0) return -1; /* eat '::' */
    } else {
      /* It was a NameTest. The NAME is already consumed (we advanced to peek for
       * '::'), so replay it through the shared node-type grammar. */
      out->axis = MKR_AXIS_CHILD;
      if (parse_nodetype_or_name(P, saved, &out->test) != 0) return -1;
      return parse_predicates(P, &out->predicates, &out->npredicates);
    }
  } else {
    out->axis = MKR_AXIS_CHILD;
  }

  if (parse_node_test(P, out->axis, &out->test) != 0) return -1;
  return parse_predicates(P, &out->predicates, &out->npredicates);
}

/* ---------- relative & absolute paths ---------- */

static int
parse_relative_path(mkr_parser_t *P, mkr_step_t **steps, size_t *nsteps)
{
  *steps = NULL;
  *nsteps = 0;
  size_t cap = 0;

  mkr_step_t s = {0};
  if (parse_step(P, &s) != 0) return -1;
  if (push_step(P, steps, nsteps, &cap, s) != 0) { mkr_step_clear(&s); return -1; }

  return parse_step_tail(P, steps, nsteps, &cap);
}

/* Returns 1 if the current token can begin a Step. */
static int
can_start_step(mkr_parser_t *P)
{
  switch (TOK(P).kind) {
  case MKR_TK_DOT: case MKR_TK_DOTDOT: case MKR_TK_AT:
  case MKR_TK_STAR: case MKR_TK_NAME: case MKR_TK_QNAME:
    return 1;
  default:
    return 0;
  }
}

static mkr_node_t *
parse_location_path(mkr_parser_t *P)
{
  mkr_node_t *n = new_node(P, MKR_NK_PATH);
  if (n == NULL) return NULL;

  if (TOK(P).kind == MKR_TK_SLASH) {
    n->u.path.absolute = 1;
    if (P_advance(P) != 0) goto fail;
    if (can_start_step(P)) {
      if (parse_relative_path(P, &n->u.path.steps, &n->u.path.nsteps) != 0) goto fail;
    }
    return n;
  }
  if (TOK(P).kind == MKR_TK_DSLASH) {
    /* '//' = '/descendant-or-self::node()/'. Leave TOK at the DSLASH so the
     * shared loop expands it (implicit step + the following step) itself. */
    n->u.path.absolute = 1;
    size_t cap = 0;
    if (parse_step_tail(P, &n->u.path.steps, &n->u.path.nsteps, &cap) != 0) goto fail;
    return n;
  }
  /* Relative location path. */
  n->u.path.absolute = 0;
  if (parse_relative_path(P, &n->u.path.steps, &n->u.path.nsteps) != 0) goto fail;
  return n;

fail:
  mkr_node_free(n);
  return NULL;
}

/* ---------- primary / function call / filter ---------- */

static mkr_node_t *
parse_function_call(mkr_parser_t *P, mkr_token_t name_tok)
{
  /* name_tok is the function NAME/QNAME; current TOK is the next token (should be '('). */
  if (TOK(P).kind != MKR_TK_LPAREN) {
    mkr_err_set(P->err, MKR_XPATH_ERR_SYNTAX, "expected '(' in function call");
    return NULL;
  }
  if (P_advance(P) != 0) return NULL;

  mkr_node_t *n = new_node(P, MKR_NK_FNCALL);
  if (n == NULL) return NULL;

  if (name_tok.kind == MKR_TK_QNAME) {
    if (P_fill_qname_split(P, name_tok.text, &n->u.fncall.prefix, &n->u.fncall.name) != 0) goto fail;
  } else {
    if (P_fill_owned_text(P, name_tok.text, &n->u.fncall.name) != 0) goto fail;
  }

  size_t cap = 0;
  if (TOK(P).kind != MKR_TK_RPAREN) {
    for (;;) {
      if (mkr_limit_check_func_args(P->limits, n->u.fncall.nargs + 1, P->err) != 0) goto fail;
      mkr_node_t *arg = parse_expr(P);
      if (arg == NULL) goto fail;
      if (mkr_grow_reserve((void **)&n->u.fncall.args, &cap,
                           n->u.fncall.nargs + 1, sizeof(*n->u.fncall.args)) != MKR_OK) {
        mkr_node_free(arg);
        mkr_err_set(P->err, MKR_XPATH_ERR_OOM, "out of memory growing function args array");
        goto fail;
      }
      n->u.fncall.args[n->u.fncall.nargs++] = arg;
      if (TOK(P).kind != MKR_TK_COMMA) break;
      if (P_advance(P) != 0) goto fail;
    }
  }
  if (P_eat(P, MKR_TK_RPAREN, "')' after function arguments") != 0) goto fail;
  return n;
fail:
  mkr_node_free(n);
  return NULL;
}

static mkr_node_t *
parse_primary(mkr_parser_t *P)
{
  mkr_node_t *n = NULL;
  switch (TOK(P).kind) {
  case MKR_TK_DOLLAR: {
    if (P_advance(P) != 0) return NULL;
    if (TOK(P).kind != MKR_TK_NAME && TOK(P).kind != MKR_TK_QNAME) {
      mkr_err_set(P->err, MKR_XPATH_ERR_SYNTAX, "expected name after '$'");
      return NULL;
    }
    n = new_node(P, MKR_NK_VARREF);
    if (n == NULL) return NULL;
    if (TOK(P).kind == MKR_TK_QNAME) {
      if (P_fill_qname_split(P, TOK(P).text, &n->u.varref.prefix, &n->u.varref.name) != 0) { mkr_node_free(n); return NULL; }
    } else {
      if (P_fill_owned_text(P, TOK(P).text, &n->u.varref.name) != 0) { mkr_node_free(n); return NULL; }
    }
    if (P_advance(P) != 0) { mkr_node_free(n); return NULL; }
    return n;
  }
  case MKR_TK_LPAREN: {
    if (P_advance(P) != 0) return NULL;
    n = parse_expr(P);
    if (n == NULL) return NULL;
    if (P_eat(P, MKR_TK_RPAREN, "')' after parenthesised expr") != 0) { mkr_node_free(n); return NULL; }
    return n;
  }
  case MKR_TK_LITERAL: {
    n = new_node(P, MKR_NK_LITERAL_STR);
    if (n == NULL) return NULL;
    if (P_fill_owned_text(P, TOK(P).text, &n->u.literal) != 0) { mkr_node_free(n); return NULL; }
    if (P_advance(P) != 0) { mkr_node_free(n); return NULL; }
    return n;
  }
  case MKR_TK_NUMBER: {
    n = new_node(P, MKR_NK_LITERAL_NUM);
    if (n == NULL) return NULL;
    n->u.literal_num = TOK(P).num;
    if (P_advance(P) != 0) { mkr_node_free(n); return NULL; }
    return n;
  }
  case MKR_TK_NAME:
  case MKR_TK_QNAME: {
    mkr_token_t name_tok = TOK(P);
    if (P_advance(P) != 0) return NULL;
    if (TOK(P).kind == MKR_TK_LPAREN) {
      return parse_function_call(P, name_tok);
    }
    mkr_err_set(P->err, MKR_XPATH_ERR_SYNTAX, "expected '(' after function name");
    return NULL;
  }
  default:
    mkr_err_set(P->err, MKR_XPATH_ERR_SYNTAX, "expected primary expression");
    return NULL;
  }
}

static mkr_node_t *
parse_filter_expr(mkr_parser_t *P)
{
  mkr_node_t *primary = parse_primary(P);
  if (primary == NULL) return NULL;
  if (TOK(P).kind != MKR_TK_LBRACKET
      && TOK(P).kind != MKR_TK_SLASH
      && TOK(P).kind != MKR_TK_DSLASH) {
    return primary;
  }
  mkr_node_t *f = new_node(P, MKR_NK_FILTER);
  if (f == NULL) { mkr_node_free(primary); return NULL; }
  f->u.filter.expr = primary;
  if (parse_predicates(P, &f->u.filter.preds, &f->u.filter.npreds) != 0) {
    mkr_node_free(f);
    return NULL;
  }
  /* Optional trailing location path (e.g. `$x/foo`, `(expr)//bar`). The shared
   * loop is a no-op when no separator follows, so call it unconditionally. */
  size_t cap = 0;
  if (parse_step_tail(P, &f->u.filter.path_steps, &f->u.filter.npath, &cap) != 0) {
    mkr_node_free(f);
    return NULL;
  }
  return f;
}

/* ---------- path expression chooser ---------- */

/* Decide whether the upcoming tokens are a LocationPath or a FilterExpr. */
static int
looks_like_filter_expr(mkr_parser_t *P)
{
  switch (TOK(P).kind) {
  case MKR_TK_DOLLAR:
  case MKR_TK_LPAREN:
  case MKR_TK_LITERAL:
  case MKR_TK_NUMBER:
    return 1;
  case MKR_TK_NAME:
  case MKR_TK_QNAME: {
    /* It's a function call iff followed by '(' AND the name is not a NodeType. */
    const char *s = TOK(P).text.ptr;
    size_t      n = TOK(P).text.len;
    if (TOK(P).kind == MKR_TK_NAME && is_nodetype_name(s, n)) {
      return 0;
    }
    /* peek next byte (skipping ws) for '(' - cheaper than running the lexer. */
    return (mkr_lexer_peek_nonws(&P->L) == '(');
  }
  default:
    return 0;
  }
}

static mkr_node_t *
parse_path_expr(mkr_parser_t *P)
{
  if (TOK(P).kind == MKR_TK_SLASH || TOK(P).kind == MKR_TK_DSLASH) {
    return parse_location_path(P);
  }
  if (looks_like_filter_expr(P)) {
    return parse_filter_expr(P);
  }
  return parse_location_path(P);
}

/* ---------- union, unary, binary ladder ---------- */

static mkr_node_t *
make_binop(mkr_parser_t *P, mkr_op_t op, mkr_node_t *lhs, mkr_node_t *rhs)
{
  mkr_node_t *n = new_node(P, MKR_NK_BINOP);
  if (n == NULL) {
    /* lhs/rhs ownership: caller must free on NULL return. */
    return NULL;
  }
  n->u.binop.op = op;
  n->u.binop.lhs = lhs;
  n->u.binop.rhs = rhs;
  return n;
}

static mkr_node_t *
parse_union(mkr_parser_t *P)
{
  mkr_node_t *l = parse_path_expr(P);
  while (l && TOK(P).kind == MKR_TK_PIPE) {
    if (P_advance(P) != 0) { mkr_node_free(l); return NULL; }
    mkr_node_t *r = parse_path_expr(P);
    if (r == NULL) { mkr_node_free(l); return NULL; }
    mkr_node_t *u = make_binop(P, MKR_OP_UNION, l, r);
    if (u == NULL) { mkr_node_free(l); mkr_node_free(r); return NULL; }
    l = u;
  }
  return l;
}

static mkr_node_t *
parse_unary(mkr_parser_t *P)
{
  int neg = 0;
  while (TOK(P).kind == MKR_TK_MINUS) {
    neg = !neg;
    if (P_advance(P) != 0) return NULL;
  }
  mkr_node_t *e = parse_union(P);
  if (e == NULL || !neg) return e;
  mkr_node_t *u = new_node(P, MKR_NK_UNARY);
  if (u == NULL) { mkr_node_free(e); return NULL; }
  u->u.unary.expr = e;
  return u;
}

/* The binary-operator precedence levels (tightest binding first). Each level
 * lists the tokens it matches and the mkr_op_t they map to; the left-associative
 * fold is driven once by parse_binary_level, replacing six near-identical loops
 * (whose only differences were these token->op maps and the next-tighter level).
 * The tightest level's operand is parse_unary (prefix '-'), a distinct production. */
typedef struct {
  const char       *word;   /* non-NULL: match a word-literal token ("div"/"and"/...) */
  size_t            wordlen; /* the word's compile-time length (0 when word == NULL) */
  mkr_tok_kind_t    kind;   /* else: match this token kind */
  mkr_op_t          op;
} mkr_binop_match_t;

/* Table-entry builders: WORD carries the literal's compile-time length (sizeof),
 * so the match below needs no strlen over the const char *. */
#define MKR_WORD(w)  (w), (sizeof(w) - 1)
#define MKR_NOWORD   NULL, 0

typedef struct {
  const mkr_binop_match_t *matches;
  size_t                   count;
} mkr_binop_level_t;

static const mkr_binop_match_t MKR_BINOPS_MUL[] = {
  { MKR_NOWORD,      MKR_TK_STAR, MKR_OP_MUL },
  { MKR_WORD("div"), MKR_TK_EOF,  MKR_OP_DIV },
  { MKR_WORD("mod"), MKR_TK_EOF,  MKR_OP_MOD },
};
static const mkr_binop_match_t MKR_BINOPS_ADD[] = {
  { MKR_NOWORD, MKR_TK_PLUS,  MKR_OP_ADD },
  { MKR_NOWORD, MKR_TK_MINUS, MKR_OP_SUB },
};
static const mkr_binop_match_t MKR_BINOPS_REL[] = {
  { MKR_NOWORD, MKR_TK_LT, MKR_OP_LT },
  { MKR_NOWORD, MKR_TK_GT, MKR_OP_GT },
  { MKR_NOWORD, MKR_TK_LE, MKR_OP_LE },
  { MKR_NOWORD, MKR_TK_GE, MKR_OP_GE },
};
static const mkr_binop_match_t MKR_BINOPS_EQ[] = {
  { MKR_NOWORD, MKR_TK_EQ, MKR_OP_EQ },
  { MKR_NOWORD, MKR_TK_NE, MKR_OP_NE },
};
static const mkr_binop_match_t MKR_BINOPS_AND[] = { { MKR_WORD("and"), MKR_TK_EOF, MKR_OP_AND } };
static const mkr_binop_match_t MKR_BINOPS_OR[]  = { { MKR_WORD("or"),  MKR_TK_EOF, MKR_OP_OR  } };

/* Precedence order, tightest-binding first (index 0). */
static const mkr_binop_level_t MKR_BINOP_LEVELS[] = {
  { MKR_BINOPS_MUL, 3 },
  { MKR_BINOPS_ADD, 2 },
  { MKR_BINOPS_REL, 4 },
  { MKR_BINOPS_EQ,  2 },
  { MKR_BINOPS_AND, 1 },
  { MKR_BINOPS_OR,  1 },
};
#define MKR_BINOP_TOP ((int)(sizeof(MKR_BINOP_LEVELS) / sizeof(MKR_BINOP_LEVELS[0])) - 1)

/* Parse binary-operator level +li+ (recursing into all tighter levels; the
 * tightest level's operand is parse_unary). Left-associative. The partial-result
 * freeing on error is the ownership contract of the six loops this replaces,
 * now kept in one place. */
static mkr_node_t *
parse_binary_level(mkr_parser_t *P, int li)
{
  mkr_node_t *l = (li == 0) ? parse_unary(P) : parse_binary_level(P, li - 1);
  const mkr_binop_level_t *lvl = &MKR_BINOP_LEVELS[li];
  while (l) {
    mkr_op_t op = MKR_OP_OR;   /* overwritten on a match below */
    int matched = 0;
    for (size_t i = 0; i < lvl->count; ++i) {
      const mkr_binop_match_t *m = &lvl->matches[i];
      /* wordlen is the literal's compile-time length (MKR_WORD's sizeof), so no
       * strlen over the stored const char * is needed. */
      int hit = m->word ? mkr_tok_is_word_len(&TOK(P), m->word, m->wordlen)
                        : (TOK(P).kind == m->kind);
      if (hit) { op = m->op; matched = 1; break; }
    }
    if (!matched) break;
    if (P_advance(P) != 0) { mkr_node_free(l); return NULL; }
    mkr_node_t *r = (li == 0) ? parse_unary(P) : parse_binary_level(P, li - 1);
    if (r == NULL) { mkr_node_free(l); return NULL; }
    mkr_node_t *b = make_binop(P, op, l, r);
    if (b == NULL) { mkr_node_free(l); mkr_node_free(r); return NULL; }
    l = b;
  }
  return l;
}

static mkr_node_t *
parse_expr(mkr_parser_t *P)
{
  /* Bound parser recursion so deeply-nested expressions ('((((...))))')
   * cannot blow the C stack. */
  if (mkr_limit_recurse_enter(P->limits, P->err) != 0) return NULL;
  mkr_node_t *n = parse_binary_level(P, MKR_BINOP_TOP);
  mkr_limit_recurse_leave(P->limits);
  return n;
}

/* ---------- entry ---------- */

mkr_node_t *
mkr_parse(mkr_verified_text_t expr, mkr_xpath_limits_t *limits, mkr_xpath_error_t *err)
{
  if (limits == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "mkr_parse: limits required");
    return NULL;
  }
  if (mkr_limit_check_expr_bytes(limits, expr.len, err) != 0) {
    return NULL;
  }

  mkr_parser_t P;
  P.err    = err;
  P.limits = limits;
  /* expr is a validated, NUL-terminated text of known length (mkr_verified_text_t). */
  mkr_lexer_init(&P.L, expr.ptr, expr.len, err);
  if (!P.L.good) return NULL;

  mkr_node_t *root = parse_expr(&P);
  if (root == NULL) return NULL;
  if (TOK(&P).kind != MKR_TK_EOF) {
    mkr_err_setf(err, MKR_XPATH_ERR_SYNTAX,
                "trailing input at '%.*s'",
                (int)TOK(&P).text.len, TOK(&P).text.ptr);
    mkr_node_free(root);
    return NULL;
  }
  /* Peephole: simplify //X//Y patterns. Runs before the hoisting
   * pass so the latter sees the rewritten step structure. */
  mkr_apply_peephole(root);
  /* Static hoisting pass: marks subtrees that can be memoized during
   * eval. Cheap (single AST walk) and runs once per parse - cached by
   * the wrapper-level AST cache. */
  mkr_mark_context_independent(root);
  return root;
}
