#include "mkr_xpath_internal.h"
#include "../core/mkr_safe.h"

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

/* strndup into a node-test name slot, recording its byte length (0 on failure —
 * the parse then aborts via P->err). Lets node_principal_match compare names
 * without a per-node strlen. */
static char *
P_strndup_len(mkr_parser_t *P, const char *s, size_t n, size_t *out_len)
{
  char *p = P_strndup(P, s, n);
  *out_len = (p != NULL) ? n : 0;
  return p;
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

/* AST node allocator. Counts against max_ast_nodes and reports both
 * OOM and LIMIT distinctly. */
static mkr_node_t *
new_node(mkr_parser_t *P, mkr_nk_t kind)
{
  if (mkr_limit_ast_node(P->limits, P->err) != 0) return NULL;
  mkr_node_t *n = mkr_callocarray(1, sizeof(*n));
  if (n == NULL) {
    mkr_err_set(P->err, MKR_XPATH_ERR_OOM, "out of memory allocating AST node");
    return NULL;
  }
  n->kind = kind;
  return n;
}

/* ---------- axis lookup ---------- */

static int
axis_by_name(const char *s, size_t n, mkr_axis_t *out)
{
#define A(name, val) do { if (n == sizeof(name)-1 && memcmp(s, name, n) == 0) { *out = val; return 1; } } while (0)
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
  return (n == 4 && memcmp(s, "node", 4) == 0)
      || (n == 4 && memcmp(s, "text", 4) == 0)
      || (n == 7 && memcmp(s, "comment", 7) == 0)
      || (n == 22 && memcmp(s, "processing-instruction", 22) == 0);
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

/* ---------- node-test parsing ---------- */

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
    const char *s = TOK(P).start;
    size_t      n = TOK(P).len;
    /* NodeType test if followed by '(' and the name matches. */
    if (is_nodetype_name(s, n)) {
      /* peek: advance once and check for '(' */
      mkr_token_t saved = TOK(P);
      if (P_advance(P) != 0) return -1;
      if (TOK(P).kind == MKR_TK_LPAREN) {
        if (P_advance(P) != 0) return -1;
        if      (saved.len == 4 && memcmp(saved.start, "node", 4)    == 0) out->kind = MKR_NT_NODE;
        else if (saved.len == 4 && memcmp(saved.start, "text", 4)    == 0) out->kind = MKR_NT_TEXT;
        else if (saved.len == 7 && memcmp(saved.start, "comment", 7) == 0) out->kind = MKR_NT_COMMENT;
        else /* processing-instruction */ {
          out->kind = MKR_NT_PI;
          if (TOK(P).kind == MKR_TK_LITERAL) {
            out->pi_target = P_strndup_len(P, TOK(P).start, TOK(P).len, &out->pi_target_len);
            if (P_advance(P) != 0) return -1;
          }
        }
        if (P_eat(P, MKR_TK_RPAREN, "')' after node type test") != 0) return -1;
        return 0;
      }
      /* Not followed by '(': it's an NCName name test. */
      out->kind  = MKR_NT_NAME;
      out->local = P_strndup_len(P, saved.start, saved.len, &out->local_len);
      return 0;
    }
    /* Plain NCName: check for ':' '*' form. */
    out->kind  = MKR_NT_NAME;
    out->local = P_strndup_len(P, s, n, &out->local_len);
    return P_advance(P);
  }

  if (TOK(P).kind == MKR_TK_QNAME) {
    /* QName name test: `prefix:local` or `prefix:*` — split at the colon. */
    const char *s = TOK(P).start;
    size_t      n = TOK(P).len;
    size_t      colon = 0;
    while (colon < n && s[colon] != ':') colon++;
    out->prefix = P_strndup(P, s, colon);
    if (n - colon - 1 == 1 && s[colon + 1] == '*') {
      /* prefix:* — any element in the prefix's namespace. */
      out->kind = MKR_NT_WILDCARD;
    } else {
      out->kind  = MKR_NT_NAME;
      out->local = P_strndup_len(P, s + colon + 1, n - colon - 1, &out->local_len);
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

static int
parse_step(mkr_parser_t *P, mkr_step_t *out)
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
      if (!axis_by_name(saved.start, saved.len, &ax)) {
        mkr_err_setf(P->err, MKR_XPATH_ERR_SYNTAX, "unknown axis '%.*s'", (int)saved.len, saved.start);
        return -1;
      }
      out->axis = ax;
      if (P_advance(P) != 0) return -1; /* eat '::' */
    } else {
      /* It was a NameTest. Process saved as the node test directly. */
      out->axis = MKR_AXIS_CHILD;
      /* Replay the saved NAME via the same logic as parse_node_test
       * but without re-consuming (we already advanced). */
      if (is_nodetype_name(saved.start, saved.len) && TOK(P).kind == MKR_TK_LPAREN) {
        if (P_advance(P) != 0) return -1;
        if      (saved.len == 4 && memcmp(saved.start, "node", 4)    == 0) out->test.kind = MKR_NT_NODE;
        else if (saved.len == 4 && memcmp(saved.start, "text", 4)    == 0) out->test.kind = MKR_NT_TEXT;
        else if (saved.len == 7 && memcmp(saved.start, "comment", 7) == 0) out->test.kind = MKR_NT_COMMENT;
        else {
          out->test.kind = MKR_NT_PI;
          if (TOK(P).kind == MKR_TK_LITERAL) {
            out->test.pi_target = P_strndup_len(P, TOK(P).start, TOK(P).len, &out->test.pi_target_len);
            if (P_advance(P) != 0) return -1;
          }
        }
        if (P_eat(P, MKR_TK_RPAREN, "')' after node type test") != 0) return -1;
      } else {
        out->test.kind  = MKR_NT_NAME;
        out->test.local = P_strndup_len(P, saved.start, saved.len, &out->test.local_len);
      }
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

  while (TOK(P).kind == MKR_TK_SLASH || TOK(P).kind == MKR_TK_DSLASH) {
    int dslash = (TOK(P).kind == MKR_TK_DSLASH);
    if (P_advance(P) != 0) return -1;
    if (dslash) {
      mkr_step_t implicit;
      make_implicit_step(&implicit, MKR_AXIS_DESCENDANT_OR_SELF, MKR_NT_NODE);
      if (push_step(P, steps, nsteps, &cap, implicit) != 0) return -1;
    }
    mkr_step_t next = {0};
    if (parse_step(P, &next) != 0) return -1;
    if (push_step(P, steps, nsteps, &cap, next) != 0) { mkr_step_clear(&next); return -1; }
  }
  return 0;
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
    n->u.path.absolute = 1;
    if (P_advance(P) != 0) goto fail;
    /* '//' = '/descendant-or-self::node()/' */
    size_t cap = 0;
    mkr_step_t implicit;
    make_implicit_step(&implicit, MKR_AXIS_DESCENDANT_OR_SELF, MKR_NT_NODE);
    if (push_step(P, &n->u.path.steps, &n->u.path.nsteps, &cap, implicit) != 0) goto fail;
    mkr_step_t s = {0};
    if (parse_step(P, &s) != 0) goto fail;
    if (push_step(P, &n->u.path.steps, &n->u.path.nsteps, &cap, s) != 0) { mkr_step_clear(&s); goto fail; }
    while (TOK(P).kind == MKR_TK_SLASH || TOK(P).kind == MKR_TK_DSLASH) {
      int dslash = (TOK(P).kind == MKR_TK_DSLASH);
      if (P_advance(P) != 0) goto fail;
      if (dslash) {
        mkr_step_t imp2;
        make_implicit_step(&imp2, MKR_AXIS_DESCENDANT_OR_SELF, MKR_NT_NODE);
        if (push_step(P, &n->u.path.steps, &n->u.path.nsteps, &cap, imp2) != 0) goto fail;
      }
      mkr_step_t s2 = {0};
      if (parse_step(P, &s2) != 0) goto fail;
      if (push_step(P, &n->u.path.steps, &n->u.path.nsteps, &cap, s2) != 0) { mkr_step_clear(&s2); goto fail; }
    }
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
    size_t colon = 0;
    while (colon < name_tok.len && name_tok.start[colon] != ':') colon++;
    n->u.fncall.prefix = P_strndup(P,name_tok.start, colon);
    n->u.fncall.name   = P_strndup(P,name_tok.start + colon + 1, name_tok.len - colon - 1);
  } else {
    n->u.fncall.name = P_strndup(P,name_tok.start, name_tok.len);
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
      size_t colon = 0;
      while (colon < TOK(P).len && TOK(P).start[colon] != ':') colon++;
      n->u.varref.prefix = P_strndup(P,TOK(P).start, colon);
      n->u.varref.name   = P_strndup(P,TOK(P).start + colon + 1, TOK(P).len - colon - 1);
    } else {
      n->u.varref.name = P_strndup(P,TOK(P).start, TOK(P).len);
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
    n->u.literal_str = P_strndup(P,TOK(P).start, TOK(P).len);
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
  if (TOK(P).kind == MKR_TK_SLASH || TOK(P).kind == MKR_TK_DSLASH) {
    int dslash = (TOK(P).kind == MKR_TK_DSLASH);
    if (P_advance(P) != 0) { mkr_node_free(f); return NULL; }
    size_t cap = 0;
    if (dslash) {
      mkr_step_t implicit;
      make_implicit_step(&implicit, MKR_AXIS_DESCENDANT_OR_SELF, MKR_NT_NODE);
      if (push_step(P, &f->u.filter.path_steps, &f->u.filter.npath, &cap, implicit) != 0) { mkr_node_free(f); return NULL; }
    }
    mkr_step_t s = {0};
    if (parse_step(P, &s) != 0) { mkr_node_free(f); return NULL; }
    if (push_step(P, &f->u.filter.path_steps, &f->u.filter.npath, &cap, s) != 0) { mkr_step_clear(&s); mkr_node_free(f); return NULL; }
    while (TOK(P).kind == MKR_TK_SLASH || TOK(P).kind == MKR_TK_DSLASH) {
      int dd = (TOK(P).kind == MKR_TK_DSLASH);
      if (P_advance(P) != 0) { mkr_node_free(f); return NULL; }
      if (dd) {
        mkr_step_t imp2;
        make_implicit_step(&imp2, MKR_AXIS_DESCENDANT_OR_SELF, MKR_NT_NODE);
        if (push_step(P, &f->u.filter.path_steps, &f->u.filter.npath, &cap, imp2) != 0) { mkr_node_free(f); return NULL; }
      }
      mkr_step_t s2 = {0};
      if (parse_step(P, &s2) != 0) { mkr_node_free(f); return NULL; }
      if (push_step(P, &f->u.filter.path_steps, &f->u.filter.npath, &cap, s2) != 0) { mkr_step_clear(&s2); mkr_node_free(f); return NULL; }
    }
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
    const char *s = TOK(P).start;
    size_t      n = TOK(P).len;
    if (TOK(P).kind == MKR_TK_NAME && is_nodetype_name(s, n)) {
      return 0;
    }
    /* peek next char (skipping ws) for '(' — cheaper than running the lexer. */
    const char *p = P->L.cur;
    while (*p && (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r')) p++;
    return (*p == '(');
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

static mkr_node_t *
parse_multiplicative(mkr_parser_t *P)
{
  mkr_node_t *l = parse_unary(P);
  while (l) {
    mkr_op_t op;
    if (TOK(P).kind == MKR_TK_STAR) {
      op = MKR_OP_MUL;
    } else if (mkr_tok_is_word(&TOK(P), "div")) {
      op = MKR_OP_DIV;
    } else if (mkr_tok_is_word(&TOK(P), "mod")) {
      op = MKR_OP_MOD;
    } else {
      break;
    }
    if (P_advance(P) != 0) { mkr_node_free(l); return NULL; }
    mkr_node_t *r = parse_unary(P);
    if (r == NULL) { mkr_node_free(l); return NULL; }
    mkr_node_t *b = make_binop(P, op, l, r);
    if (b == NULL) { mkr_node_free(l); mkr_node_free(r); return NULL; }
    l = b;
  }
  return l;
}

static mkr_node_t *
parse_additive(mkr_parser_t *P)
{
  mkr_node_t *l = parse_multiplicative(P);
  while (l && (TOK(P).kind == MKR_TK_PLUS || TOK(P).kind == MKR_TK_MINUS)) {
    mkr_op_t op = (TOK(P).kind == MKR_TK_PLUS) ? MKR_OP_ADD : MKR_OP_SUB;
    if (P_advance(P) != 0) { mkr_node_free(l); return NULL; }
    mkr_node_t *r = parse_multiplicative(P);
    if (r == NULL) { mkr_node_free(l); return NULL; }
    mkr_node_t *b = make_binop(P, op, l, r);
    if (b == NULL) { mkr_node_free(l); mkr_node_free(r); return NULL; }
    l = b;
  }
  return l;
}

static mkr_node_t *
parse_relational(mkr_parser_t *P)
{
  mkr_node_t *l = parse_additive(P);
  while (l) {
    mkr_op_t op;
    switch (TOK(P).kind) {
    case MKR_TK_LT: op = MKR_OP_LT; break;
    case MKR_TK_GT: op = MKR_OP_GT; break;
    case MKR_TK_LE: op = MKR_OP_LE; break;
    case MKR_TK_GE: op = MKR_OP_GE; break;
    default: return l;
    }
    if (P_advance(P) != 0) { mkr_node_free(l); return NULL; }
    mkr_node_t *r = parse_additive(P);
    if (r == NULL) { mkr_node_free(l); return NULL; }
    mkr_node_t *b = make_binop(P, op, l, r);
    if (b == NULL) { mkr_node_free(l); mkr_node_free(r); return NULL; }
    l = b;
  }
  return l;
}

static mkr_node_t *
parse_equality(mkr_parser_t *P)
{
  mkr_node_t *l = parse_relational(P);
  while (l && (TOK(P).kind == MKR_TK_EQ || TOK(P).kind == MKR_TK_NE)) {
    mkr_op_t op = (TOK(P).kind == MKR_TK_EQ) ? MKR_OP_EQ : MKR_OP_NE;
    if (P_advance(P) != 0) { mkr_node_free(l); return NULL; }
    mkr_node_t *r = parse_relational(P);
    if (r == NULL) { mkr_node_free(l); return NULL; }
    mkr_node_t *b = make_binop(P, op, l, r);
    if (b == NULL) { mkr_node_free(l); mkr_node_free(r); return NULL; }
    l = b;
  }
  return l;
}

static mkr_node_t *
parse_and(mkr_parser_t *P)
{
  mkr_node_t *l = parse_equality(P);
  while (l && mkr_tok_is_word(&TOK(P), "and")) {
    if (P_advance(P) != 0) { mkr_node_free(l); return NULL; }
    mkr_node_t *r = parse_equality(P);
    if (r == NULL) { mkr_node_free(l); return NULL; }
    mkr_node_t *b = make_binop(P, MKR_OP_AND, l, r);
    if (b == NULL) { mkr_node_free(l); mkr_node_free(r); return NULL; }
    l = b;
  }
  return l;
}

static mkr_node_t *
parse_or(mkr_parser_t *P)
{
  mkr_node_t *l = parse_and(P);
  while (l && mkr_tok_is_word(&TOK(P), "or")) {
    if (P_advance(P) != 0) { mkr_node_free(l); return NULL; }
    mkr_node_t *r = parse_and(P);
    if (r == NULL) { mkr_node_free(l); return NULL; }
    mkr_node_t *b = make_binop(P, MKR_OP_OR, l, r);
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
  mkr_node_t *n = parse_or(P);
  mkr_limit_recurse_leave(P->limits);
  return n;
}

/* ---------- entry ---------- */

mkr_node_t *
mkr_parse(mkr_valid_text_t expr, mkr_xpath_limits_t *limits, mkr_xpath_error_t *err)
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
  /* expr.ptr is a validated, NUL-terminated C string (mkr_valid_text_t). */
  mkr_lexer_init(&P.L, expr.ptr, err);
  if (!P.L.good) return NULL;

  mkr_node_t *root = parse_expr(&P);
  if (root == NULL) return NULL;
  if (TOK(&P).kind != MKR_TK_EOF) {
    mkr_err_setf(err, MKR_XPATH_ERR_SYNTAX,
                "trailing input at '%.*s'",
                (int)TOK(&P).len, TOK(&P).start);
    mkr_node_free(root);
    return NULL;
  }
  /* Peephole: simplify //X//Y patterns. Runs before the hoisting
   * pass so the latter sees the rewritten step structure. */
  mkr_apply_peephole(root);
  /* Static hoisting pass: marks subtrees that can be memoized during
   * eval. Cheap (single AST walk) and runs once per parse — cached by
   * the wrapper-level AST cache. */
  mkr_mark_context_independent(root);
  return root;
}
