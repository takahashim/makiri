/*
 * CSS selector front-end: lowers a Lexbor-parsed selector list into the native
 * XPath engine's AST (see mkr_css.h). No new evaluator opcodes - every selector
 * becomes existing PATH / step / predicate nodes, so the shared evaluator's
 * budgets, document order, dedup and namespace resolution apply unchanged.
 *
 * This is a SHARED translation unit (compiled once, like mkr_xpath_parse.c): it
 * includes mkr_xpath_internal.h with the default `void` MKR_DOM_NODE - it only
 * BUILDS the representation-neutral AST, never dereferences a node.
 */

#include "mkr_css.h"
#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"

#include <lexbor/css/css.h>
#include <lexbor/selectors/selectors.h>

#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ *
 * Process-global Lexbor CSS parser (selector parsing only - NOT the matcher).
 * CSS compilation runs under the GVL (the glue holds it), so a single global is
 * safe with no locking, like the HTML CSS engine. Created lazily; on failure the
 * caller reports a syntax/engine error.
 * ------------------------------------------------------------------ */
static lxb_css_memory_t    *g_mem;
static lxb_css_parser_t    *g_parser;
static lxb_css_selectors_t *g_sel;
static int                  g_ready;

static int
css_parser_ready(void)
{
  if (g_ready) return 1;

  lxb_css_memory_t    *mem = lxb_css_memory_create();
  lxb_css_parser_t    *par = lxb_css_parser_create();
  lxb_css_selectors_t *sel = lxb_css_selectors_create();

  int ok = (mem != NULL && par != NULL && sel != NULL)
        && (lxb_css_memory_init(mem, 128) == LXB_STATUS_OK)
        && (lxb_css_parser_init(par, NULL) == LXB_STATUS_OK)
        && (lxb_css_selectors_init(sel) == LXB_STATUS_OK);
  if (!ok) {
    if (sel != NULL) lxb_css_selectors_destroy(sel, true);
    if (par != NULL) lxb_css_parser_destroy(par, true);
    if (mem != NULL) lxb_css_memory_destroy(mem, true);
    return 0;
  }
  lxb_css_parser_memory_set(par, mem);
  lxb_css_parser_selectors_set(par, sel);
  g_mem = mem; g_parser = par; g_sel = sel; g_ready = 1;
  return 1;
}

/* ------------------------------------------------------------------ *
 * AST builders. Every allocation matches mkr_node_free's free contract: nodes
 * via mkr_callocarray, owned text via mkr_owned_text_from_borrowed_copy, arrays
 * via malloc/realloc. On any OOM/limit the builder sets *err and returns NULL/-1
 * so the whole compile fails closed (a partial AST is freed by the caller).
 * ------------------------------------------------------------------ */

typedef struct {
  mkr_xpath_limits_t *limits;
  mkr_xpath_error_t  *err;
  const mkr_css_ns_t *ns;
} css_build_t;

/* A CSS identifier byte (the chars a namespace prefix / type name is made of). */
static int
css_is_name_byte(unsigned char c)
{
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')
      || (c >= '0' && c <= '9') || c == '-' || c == '_' || c == '\\' || c >= 0x80;
}

/* Detect a leading-pipe type selector (`|el`, the no-namespace form): a top-level
 * `|` not immediately preceded by `*` or an identifier byte (so it follows the
 * start, whitespace, a combinator, `,` or `(`). Lexbor's parser cannot represent
 * `|el` distinctly from `*|el` (both arrive as ns="*"), so rather than silently
 * match any namespace we fail closed here. `*|el`, `ns|el` and the `[a|=v]`
 * attribute operator are NOT leading pipes and are unaffected. */
static int
css_has_leading_pipe(const char *s, size_t len)
{
  mkr_span_t sp = mkr_span(s, len);
  int bracket = 0;
  unsigned char prev = 0;   /* last significant byte; 0 = start, ' ' = whitespace */
  for (int b; (b = mkr_span_take(&sp)) != -1; ) {
    unsigned char c = (unsigned char)b;
    if (c == '[') { bracket++; prev = c; continue; }
    if (c == ']') { if (bracket > 0) bracket--; prev = c; continue; }
    if (c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\f') {
      if (bracket == 0) prev = ' ';
      continue;
    }
    if (c == '|' && bracket == 0 && prev != '*' && !css_is_name_byte(prev)) {
      return 1;
    }
    prev = c;
  }
  return 0;
}

static mkr_node_t *
cb_node(css_build_t *B, mkr_nk_t kind)
{
  return mkr_node_alloc(B->limits, B->err, kind);  /* shared AST factory (co-located with mkr_node_free) */
}

static int
cb_set_text(css_build_t *B, mkr_owned_text_t *out, const char *s, size_t len)
{
  return mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text(s, len), B->err, "css name");
}

static mkr_node_t *
cb_literal(css_build_t *B, const char *s, size_t len)
{
  mkr_node_t *n = cb_node(B, MKR_NK_LITERAL_STR);
  if (n == NULL) return NULL;
  if (cb_set_text(B, &n->u.literal, s, len) != 0) { mkr_node_free(n); return NULL; }
  return n;
}

static mkr_node_t *
cb_num(css_build_t *B, double v)
{
  mkr_node_t *n = cb_node(B, MKR_NK_LITERAL_NUM);
  if (n == NULL) return NULL;
  n->u.literal_num = v;
  return n;
}

static mkr_node_t *
cb_binop(css_build_t *B, mkr_op_t op, mkr_node_t *lhs, mkr_node_t *rhs)
{
  if (lhs == NULL || rhs == NULL) { mkr_node_free(lhs); mkr_node_free(rhs); return NULL; }
  mkr_node_t *n = cb_node(B, MKR_NK_BINOP);
  if (n == NULL) { mkr_node_free(lhs); mkr_node_free(rhs); return NULL; }
  n->u.binop.op = op; n->u.binop.lhs = lhs; n->u.binop.rhs = rhs;
  return n;
}

/* A function call taking ownership of args[0..nargs). On failure frees args.
 * The name is always an internal compile-time literal; the cb_fncall macro
 * supplies its length so no strlen on a (possibly non-NUL-checked) pointer. */
static mkr_node_t *
cb_fncall_n(css_build_t *B, const char *name, size_t namelen, mkr_node_t **args, size_t nargs)
{
  for (size_t i = 0; i < nargs; i++) {
    if (args[i] == NULL) { for (size_t j = 0; j < nargs; j++) mkr_node_free(args[j]); free(args); return NULL; }
  }
  mkr_node_t *n = cb_node(B, MKR_NK_FNCALL);
  if (n == NULL) { for (size_t j = 0; j < nargs; j++) mkr_node_free(args[j]); free(args); return NULL; }
  if (cb_set_text(B, &n->u.fncall.name, name, namelen) != 0) {
    for (size_t j = 0; j < nargs; j++) mkr_node_free(args[j]); free(args); mkr_node_free(n); return NULL;
  }
  n->u.fncall.args = args; n->u.fncall.nargs = nargs;
  return n;
}

/* Callers pass a string literal, so sizeof - 1 is its exact length. */
#define cb_fncall(B, name_lit, args, nargs) \
  cb_fncall_n((B), (name_lit), sizeof(name_lit) - 1, (args), (nargs))

static mkr_node_t **
cb_args(size_t n)
{
  return (mkr_node_t **)mkr_callocarray(n ? n : 1, sizeof(mkr_node_t *));
}

/* A single-step relative PATH: axis::nodetest (no predicates). nt_local may be
 * NULL for a wildcard. Used for @attr, preceding-sibling::*, child::node(), etc. */
static mkr_node_t *
cb_step_path(css_build_t *B, mkr_axis_t axis, mkr_nt_kind_t nt_kind,
             const char *local, size_t local_len)
{
  mkr_node_t *n = cb_node(B, MKR_NK_PATH);
  if (n == NULL) return NULL;
  mkr_step_t *steps = (mkr_step_t *)mkr_callocarray(1, sizeof(mkr_step_t));
  if (steps == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "out of memory (css)"); mkr_node_free(n); return NULL; }
  steps[0].axis = axis;
  steps[0].test.kind = nt_kind;
  if (nt_kind == MKR_NT_NAME && local != NULL) {
    if (cb_set_text(B, &steps[0].test.local, local, local_len) != 0) {
      free(steps); mkr_node_free(n); return NULL;
    }
  }
  n->u.path.absolute = 0;
  n->u.path.steps = steps;
  n->u.path.nsteps = 1;
  return n;
}

/* @prefix:name (or @name when prefix is NULL) as a relative attribute-axis path. */
static mkr_node_t *
cb_attr_ns(css_build_t *B, const char *prefix, size_t plen, const char *name, size_t len)
{
  mkr_node_t *n = cb_node(B, MKR_NK_PATH);
  if (n == NULL) return NULL;
  mkr_step_t *steps = (mkr_step_t *)mkr_callocarray(1, sizeof(mkr_step_t));
  if (steps == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "out of memory (css)"); mkr_node_free(n); return NULL; }
  steps[0].axis = MKR_AXIS_ATTRIBUTE;
  steps[0].test.kind = MKR_NT_NAME;
  if (cb_set_text(B, &steps[0].test.local, name, len) != 0) { free(steps); mkr_node_free(n); return NULL; }
  if (prefix != NULL && plen > 0) {
    if (cb_set_text(B, &steps[0].test.prefix, prefix, plen) != 0) {
      mkr_owned_text_clear(&steps[0].test.local); free(steps); mkr_node_free(n); return NULL;
    }
  }
  n->u.path.absolute = 0;
  n->u.path.steps = steps;
  n->u.path.nsteps = 1;
  return n;
}

/* @name as a relative attribute-axis path (no namespace). */
static mkr_node_t *
cb_attr(css_build_t *B, const char *name, size_t len)
{
  return cb_attr_ns(B, NULL, 0, name, len);
}

/* normalize-space(@[prefix:]name) */
static mkr_node_t *
cb_norm_attr(css_build_t *B, const char *prefix, size_t plen, const char *name, size_t len)
{
  mkr_node_t **a = cb_args(1);
  if (a == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
  a[0] = cb_attr_ns(B, prefix, plen, name, len);
  return cb_fncall(B, "normalize-space", a, 1);
}

/* concat(" ", normalize-space(@name), " ") - the whitespace-padded token list. */
static mkr_node_t *
cb_padded_tokens(css_build_t *B, const char *prefix, size_t plen, const char *name, size_t len)
{
  mkr_node_t **a = cb_args(3);
  if (a == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
  a[0] = cb_literal(B, " ", 1);
  a[1] = cb_norm_attr(B, prefix, plen, name, len);
  a[2] = cb_literal(B, " ", 1);
  return cb_fncall(B, "concat", a, 3);
}

/* contains(concat(' ',normalize-space(@name),' '), ' value ') - the [name~=value]
 * / .class membership predicate. +value+ is wrapped with surrounding spaces. */
static mkr_node_t *
cb_token_match(css_build_t *B, const char *prefix, size_t plen,
               const char *attr, size_t attrlen,
               const char *value, size_t vlen)
{
  /* build " value " literal */
  size_t alen = vlen + 2;
  char *padded = mkr_str_alloc(alen);   /* alen content bytes + a preset NUL */
  if (padded == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
  padded[0] = ' ';
  memcpy(padded + 1, value, vlen);
  padded[1 + vlen] = ' ';

  mkr_node_t **a = cb_args(2);
  if (a == NULL) { free(padded); mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
  a[0] = cb_padded_tokens(B, prefix, plen, attr, attrlen);
  a[1] = cb_literal(B, padded, alen);
  free(padded);
  return cb_fncall(B, "contains", a, 2);
}

/* ------------------------------------------------------------------ *
 * Step / predicate dynamic arrays.
 * ------------------------------------------------------------------ */

typedef struct { mkr_step_t *v; size_t n, cap; } steps_arr_t;
typedef struct { mkr_node_t **v; size_t n, cap; } preds_arr_t;

static int
steps_push(css_build_t *B, steps_arr_t *a, mkr_step_t s)
{
  if (mkr_grow_reserve((void **)&a->v, &a->cap, a->n + 1, sizeof(mkr_step_t)) != MKR_OK) {
    mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "out of memory growing step array");
    return -1;
  }
  a->v[a->n++] = s;
  return 0;
}

static int
preds_push(css_build_t *B, preds_arr_t *a, mkr_node_t *p)
{
  if (p == NULL) return -1;
  if (mkr_grow_reserve((void **)&a->v, &a->cap, a->n + 1, sizeof(mkr_node_t *)) != MKR_OK) {
    mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "out of memory growing predicate array");
    mkr_node_free(p);
    return -1;
  }
  a->v[a->n++] = p;
  return 0;
}

/* ------------------------------------------------------------------ *
 * Lowering a compound (run of CLOSE-linked simple selectors on one element).
 * ------------------------------------------------------------------ */

/* Set the step's name test from a type selector, honouring CSS namespace rules
 * and the Nokogiri default-namespace binding (see mkr_css.h). Returns 0 / -1. */
static int
lower_type(css_build_t *B, const lxb_css_selector_t *s, mkr_step_t *step,
           preds_arr_t *preds)
{
  const char *name = (const char *)s->name.data;
  size_t      nlen = s->name.length;

  if (s->type == LXB_CSS_SELECTOR_TYPE_ANY) {           /* * or ns|*  */
    step->test.kind = MKR_NT_WILDCARD;
    return 0;
  }

  /* Namespace component (s->ns): NULL data = bare (no pipe); "*" = any; ""
   * (length 0, non-NULL) = no namespace; else an explicit prefix. */
  const char *nsd = (const char *)s->ns.data;
  size_t      nsl = s->ns.length;

  if (nsd != NULL && nsl == 1 && nsd[0] == '*') {
    /* *|el : any namespace, specific local name -> wildcard + local-name() pred */
    step->test.kind = MKR_NT_WILDCARD;
    mkr_node_t **a = cb_args(0);
    if (a == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return -1; }
    mkr_node_t *ln = cb_fncall(B, "local-name", a, 0);
    mkr_node_t *lit = cb_literal(B, name, nlen);
    return preds_push(B, preds, cb_binop(B, MKR_OP_EQ, ln, lit));
  }

  step->test.kind = MKR_NT_NAME;
  if (cb_set_text(B, &step->test.local, name, nlen) != 0) return -1;

  if (nsd != NULL) {
    if (nsl > 0) {                                       /* p|el */
      if (cb_set_text(B, &step->test.prefix, nsd, nsl) != 0) return -1;
    }
    /* nsl == 0 (|el): no-namespace -> leave prefix NULL */
  } else if (B->ns != NULL && B->ns->default_prefix != NULL) {
    /* bare el with a document default namespace -> bind to the default prefix
     * (always the MKR_CSS_DEFAULT_NS_PREFIX sentinel, so its length is known). */
    if (cb_set_text(B, &step->test.prefix, B->ns->default_prefix,
                    sizeof(MKR_CSS_DEFAULT_NS_PREFIX) - 1) != 0) return -1;
  }
  return 0;
}

/* [name op value] attribute predicate -> an expression. */
static mkr_node_t *
lower_attribute(css_build_t *B, const lxb_css_selector_t *s)
{
  const char *nm = (const char *)s->name.data;
  size_t      nl = s->name.length;
  const lxb_css_selector_attribute_t *at = &s->u.attribute;

  if (at->modifier == LXB_CSS_SELECTOR_MODIFIER_I
      || at->modifier == LXB_CSS_SELECTOR_MODIFIER_S) {
    mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX,
                "CSS attribute case modifier ([a=v i]) is not supported");
    return NULL;
  }

  /* Attribute namespace (s->ns): NULL = bare (no namespace, the common case);
   * "p" = prefixed (@p:name); "*" = any (not yet supported). Unprefixed CSS
   * attribute selectors match the no-namespace attribute, per CSS/XPath. */
  const char *px = NULL; size_t pxl = 0;
  if (s->ns.data != NULL) {
    if (s->ns.length == 1 && s->ns.data[0] == '*') {
      mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX,
                  "any-namespace attribute selectors ([*|a]) are not supported");
      return NULL;
    }
    px = (const char *)s->ns.data; pxl = s->ns.length; /* length 0 (|a) -> no-ns */
  }

  if (at->value.data == NULL) {              /* [name] - existence */
    return cb_attr_ns(B, px, pxl, nm, nl);
  }

  const char *v = (const char *)at->value.data;
  size_t      vl = at->value.length;

  switch (at->match) {
    case LXB_CSS_SELECTOR_MATCH_EQUAL:       /* [a=v] -> @a = 'v' */
      return cb_binop(B, MKR_OP_EQ, cb_attr_ns(B, px, pxl, nm, nl), cb_literal(B, v, vl));

    case LXB_CSS_SELECTOR_MATCH_INCLUDE:     /* [a~=v] -> token match */
      return cb_token_match(B, px, pxl, nm, nl, v, vl);

    case LXB_CSS_SELECTOR_MATCH_PREFIX: {    /* [a^=v] -> starts-with(@a,'v') */
      mkr_node_t **a = cb_args(2);
      if (a == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
      a[0] = cb_attr_ns(B, px, pxl, nm, nl); a[1] = cb_literal(B, v, vl);
      return cb_fncall(B, "starts-with", a, 2);
    }
    case LXB_CSS_SELECTOR_MATCH_SUBSTRING: { /* [a*=v] -> contains(@a,'v') */
      mkr_node_t **a = cb_args(2);
      if (a == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
      a[0] = cb_attr_ns(B, px, pxl, nm, nl); a[1] = cb_literal(B, v, vl);
      return cb_fncall(B, "contains", a, 2);
    }
    case LXB_CSS_SELECTOR_MATCH_SUFFIX: {    /* [a$=v] -> substring(@a, string-length(@a)-len+1)='v' */
      mkr_node_t **sla = cb_args(1);
      if (sla == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
      sla[0] = cb_attr_ns(B, px, pxl, nm, nl);
      mkr_node_t *slen = cb_fncall(B, "string-length", sla, 1);            /* string-length(@a) */
      mkr_node_t *vlen = cb_num(B, (double)vl);
      mkr_node_t *one  = cb_num(B, 1.0);
      mkr_node_t *start = cb_binop(B, MKR_OP_ADD,
                                   cb_binop(B, MKR_OP_SUB, slen, vlen), one); /* len-vl+1 */
      mkr_node_t **sa = cb_args(2);
      if (sa == NULL) { mkr_node_free(start); mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
      sa[0] = cb_attr_ns(B, px, pxl, nm, nl); sa[1] = start;
      mkr_node_t *sub = cb_fncall(B, "substring", sa, 2);                  /* substring(@a, start) */
      return cb_binop(B, MKR_OP_EQ, sub, cb_literal(B, v, vl));
    }
    case LXB_CSS_SELECTOR_MATCH_DASH: {      /* [a|=v] -> @a='v' or starts-with(@a,'v-') */
      mkr_node_t *eq = cb_binop(B, MKR_OP_EQ, cb_attr_ns(B, px, pxl, nm, nl), cb_literal(B, v, vl));
      size_t dl = vl + 1;
      char *dash = mkr_str_alloc(dl);   /* dl content bytes + a preset NUL */
      if (dash == NULL) { mkr_node_free(eq); mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
      memcpy(dash, v, vl); dash[vl] = '-';
      mkr_node_t **a = cb_args(2);
      if (a == NULL) { free(dash); mkr_node_free(eq); mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
      a[0] = cb_attr_ns(B, px, pxl, nm, nl); a[1] = cb_literal(B, dash, dl);
      free(dash);
      mkr_node_t *pre = cb_fncall(B, "starts-with", a, 2);
      return cb_binop(B, MKR_OP_OR, eq, pre);
    }
    default:
      mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX, "unsupported CSS attribute operator");
      return NULL;
  }
}

/* Forward declarations (mutual recursion: functional pseudos lower sub-selectors
 * back through the compound/complex lowering). */
/* +relative_first+: when nonzero the first compound honours its own combinator
 * relative to the context node (used by :has, where `:has(> a)` / `:has(+ a)` /
 * `:has(~ a)` mean child / adjacent / general-sibling of self); the top-level
 * query passes 0, making the first compound a plain descendant of the context. */
static mkr_node_t *lower_complex(css_build_t *B, const lxb_css_selector_t *first,
                                 int relative_first);
/* Boolean self-test for a (possibly multi-compound) complex selector, used by
 * :is()/:where()/:not(). Combinators are expressed with the reverse axes:
 *   a b  -> self::b/ancestor::a   a > b -> self::b/parent::a
 *   a + b -> self::b/preceding-sibling::*[1]/self::a   a ~ b -> .../preceding-sibling::a
 * so the path is non-empty (truthy) exactly when self matches the selector. */
static mkr_node_t *lower_complex_selftest(css_build_t *B, const lxb_css_selector_t *first);

/* Cap on compounds in one :is()/:not() argument (selector-complexity bound). */
#define MKR_CSS_MAX_COMPOUNDS 64

/* not(axis::*) - "no sibling/child on that axis". */
static mkr_node_t *
cb_not_axis(css_build_t *B, mkr_axis_t axis, mkr_nt_kind_t nt)
{
  mkr_node_t **a = cb_args(1);
  if (a == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
  a[0] = cb_step_path(B, axis, nt, NULL, 0);
  return cb_fncall(B, "not", a, 1);
}

/* not([prefix:]name on axis) - "no same-named sibling on that axis" (of-type). */
static mkr_node_t *
cb_not_named_axis(css_build_t *B, mkr_axis_t axis, const mkr_nodetest_t *t)
{
  mkr_node_t **a = cb_args(1);
  if (a == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
  mkr_node_t *p = cb_node(B, MKR_NK_PATH);
  if (p == NULL) { free(a); return NULL; }
  mkr_step_t *st = (mkr_step_t *)mkr_callocarray(1, sizeof(mkr_step_t));
  if (st == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); mkr_node_free(p); free(a); return NULL; }
  st[0].axis = axis;
  st[0].test.kind = MKR_NT_NAME;
  if (cb_set_text(B, &st[0].test.local, (const char *)t->local.ptr, t->local.len) != 0) {
    free(st); mkr_node_free(p); free(a); return NULL;
  }
  if (t->prefix.ptr != NULL && t->prefix.len > 0) {
    if (cb_set_text(B, &st[0].test.prefix, (const char *)t->prefix.ptr, t->prefix.len) != 0) {
      mkr_owned_text_clear(&st[0].test.local); free(st); mkr_node_free(p); free(a); return NULL;
    }
  }
  p->u.path.absolute = 0; p->u.path.steps = st; p->u.path.nsteps = 1;
  a[0] = p;
  return cb_fncall(B, "not", a, 1);
}

/* count(axis::test) + 1 - 1-based position among the matched siblings. test is *
 * (local==NULL) for nth-child or a copied name (for nth-of-type). */
static mkr_node_t *
cb_pos(css_build_t *B, mkr_axis_t axis, const mkr_nodetest_t *named)
{
  mkr_node_t *path;
  if (named == NULL) {
    path = cb_step_path(B, axis, MKR_NT_WILDCARD, NULL, 0);
  } else {
    path = cb_node(B, MKR_NK_PATH);
    if (path != NULL) {
      mkr_step_t *st = (mkr_step_t *)mkr_callocarray(1, sizeof(mkr_step_t));
      if (st == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); mkr_node_free(path); path = NULL; }
      else {
        st[0].axis = axis; st[0].test.kind = MKR_NT_NAME;
        if (cb_set_text(B, &st[0].test.local, (const char *)named->local.ptr, named->local.len) != 0) {
          free(st); mkr_node_free(path); path = NULL;
        } else {
          if (named->prefix.ptr != NULL && named->prefix.len > 0
              && cb_set_text(B, &st[0].test.prefix, (const char *)named->prefix.ptr, named->prefix.len) != 0) {
            mkr_owned_text_clear(&st[0].test.local); free(st); mkr_node_free(path); path = NULL;
          } else { path->u.path.absolute = 0; path->u.path.steps = st; path->u.path.nsteps = 1; }
        }
      }
    }
  }
  mkr_node_t **a = cb_args(1);
  if (a == NULL) { mkr_node_free(path); mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
  a[0] = path;
  return cb_binop(B, MKR_OP_ADD, cb_fncall(B, "count", a, 1), cb_num(B, 1.0));
}

/* The internal of-type position fn (1-based, among same-type siblings). +forward+
 * counts from the start (preceding-sibling direction), else from the end. Used
 * for an *untyped* of-type, whose "type" is the element's own expanded name - a
 * self comparison pure XPath 1.0 cannot make. See MKR_FN_OF_TYPE_POS. */
static mkr_node_t *
cb_of_type_pos(css_build_t *B, int forward)
{
  mkr_node_t **a = cb_args(0);
  if (a == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
  return forward ? cb_fncall(B, MKR_FN_OF_TYPE_POS, a, 0)
                 : cb_fncall(B, MKR_FN_OF_TYPE_POS_LAST, a, 0);
}

/* 1-based position expression for :nth-*: an untyped of-type uses the internal
 * of-type-pos fn; otherwise count(axis::test)+1 (nth-child when named==NULL, a
 * typed of-type when named). */
static mkr_node_t *
cb_pos_expr(css_build_t *B, mkr_axis_t axis, const mkr_nodetest_t *named, int oftype_untyped)
{
  if (oftype_untyped) return cb_of_type_pos(B, axis == MKR_AXIS_PRECEDING_SIBLING);
  return cb_pos(B, axis, named);
}

/* The :nth-*(an+b) match condition over the position expression on +axis+
 * (preceding/following-sibling), optionally restricted to +named+ (typed
 * of-type) or +oftype_untyped+ (of-type with no explicit type selector). */
static mkr_node_t *
cb_nth(css_build_t *B, mkr_axis_t axis, const mkr_nodetest_t *named,
       int oftype_untyped, long a, long b)
{
  if (a == 0) {                                   /* position = b */
    return cb_binop(B, MKR_OP_EQ, cb_pos_expr(B, axis, named, oftype_untyped), cb_num(B, (double)b));
  }
  /* (pos-b) mod a == 0  AND  (pos-b) div a >= 0   (forward, valid index) */
  mkr_node_t *d1 = cb_binop(B, MKR_OP_SUB, cb_pos_expr(B, axis, named, oftype_untyped), cb_num(B, (double)b));
  mkr_node_t *modz = cb_binop(B, MKR_OP_EQ, cb_binop(B, MKR_OP_MOD, d1, cb_num(B, (double)a)), cb_num(B, 0.0));
  mkr_node_t *d2 = cb_binop(B, MKR_OP_SUB, cb_pos_expr(B, axis, named, oftype_untyped), cb_num(B, (double)b));
  mkr_node_t *qge = cb_binop(B, MKR_OP_GE, cb_binop(B, MKR_OP_DIV, d2, cb_num(B, (double)a)), cb_num(B, 0.0));
  return cb_binop(B, MKR_OP_AND, modz, qge);
}

/* Simple (non-functional) structural pseudo-classes. +step+ supplies the element
 * name for the of-type family. */
static mkr_node_t *
lower_pseudo_simple(css_build_t *B, const lxb_css_selector_t *s, const mkr_step_t *step)
{
  switch (s->u.pseudo.type) {
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FIRST_CHILD:
      return cb_not_axis(B, MKR_AXIS_PRECEDING_SIBLING, MKR_NT_WILDCARD);
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_LAST_CHILD:
      return cb_not_axis(B, MKR_AXIS_FOLLOWING_SIBLING, MKR_NT_WILDCARD);
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_ONLY_CHILD:
      return cb_binop(B, MKR_OP_AND,
                      cb_not_axis(B, MKR_AXIS_PRECEDING_SIBLING, MKR_NT_WILDCARD),
                      cb_not_axis(B, MKR_AXIS_FOLLOWING_SIBLING, MKR_NT_WILDCARD));
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_EMPTY:           /* not(node()) */
      return cb_not_axis(B, MKR_AXIS_CHILD, MKR_NT_NODE);
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_ROOT:            /* not(parent::*) */
      return cb_not_axis(B, MKR_AXIS_PARENT, MKR_NT_WILDCARD);

    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FIRST_OF_TYPE:
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_LAST_OF_TYPE:
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_ONLY_OF_TYPE: {
      /* Typed (a:first-of-type) -> not(preceding-sibling::a); untyped
       * (:first-of-type) -> of-type-pos()=1 (the type is the element's own
       * expanded name, compared at eval time - see cb_of_type_pos). */
      unsigned pt = s->u.pseudo.type;
      if (step->test.kind != MKR_NT_NAME) {
        if (pt == LXB_CSS_SELECTOR_PSEUDO_CLASS_FIRST_OF_TYPE)
          return cb_binop(B, MKR_OP_EQ, cb_of_type_pos(B, 1), cb_num(B, 1.0));
        if (pt == LXB_CSS_SELECTOR_PSEUDO_CLASS_LAST_OF_TYPE)
          return cb_binop(B, MKR_OP_EQ, cb_of_type_pos(B, 0), cb_num(B, 1.0));
        return cb_binop(B, MKR_OP_AND,
                        cb_binop(B, MKR_OP_EQ, cb_of_type_pos(B, 1), cb_num(B, 1.0)),
                        cb_binop(B, MKR_OP_EQ, cb_of_type_pos(B, 0), cb_num(B, 1.0)));
      }
      if (pt == LXB_CSS_SELECTOR_PSEUDO_CLASS_FIRST_OF_TYPE)
        return cb_not_named_axis(B, MKR_AXIS_PRECEDING_SIBLING, &step->test);
      if (pt == LXB_CSS_SELECTOR_PSEUDO_CLASS_LAST_OF_TYPE)
        return cb_not_named_axis(B, MKR_AXIS_FOLLOWING_SIBLING, &step->test);
      return cb_binop(B, MKR_OP_AND,
                      cb_not_named_axis(B, MKR_AXIS_PRECEDING_SIBLING, &step->test),
                      cb_not_named_axis(B, MKR_AXIS_FOLLOWING_SIBLING, &step->test));
    }

    default:
      mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX, "unsupported CSS pseudo-class");
      return NULL;
  }
}

/* OR of compound self-tests over each comma-arg of a selector list (:is / :not).
 * Each arg must be a single compound (no combinators); returns NULL on error. */
static mkr_node_t *
lower_selector_list_selftest(css_build_t *B, const lxb_css_selector_list_t *list)
{
  mkr_node_t *acc = NULL;
  for (const lxb_css_selector_list_t *g = list; g != NULL; g = g->next) {
    mkr_node_t *one = lower_complex_selftest(B, g->first);
    if (one == NULL) { mkr_node_free(acc); return NULL; }
    acc = (acc == NULL) ? one : cb_binop(B, MKR_OP_OR, acc, one);
    if (acc == NULL) return NULL;
  }
  return acc;
}

/* child::text()[pred] - a relative path selecting the element's direct child
 * text nodes that satisfy +pred+ (consumed). In predicate position a non-empty
 * node-set is truthy, so this is "some direct child text node matches", which is
 * exactly how Lexbor's :lexbor-contains matcher scans (immediate child TEXT
 * nodes only, not the deep string-value). +pred+ is owned on success or freed on
 * any failure. */
static mkr_node_t *
cb_child_text_pred(css_build_t *B, mkr_node_t *pred)
{
  if (pred == NULL) return NULL;
  mkr_node_t *n = cb_node(B, MKR_NK_PATH);
  if (n == NULL) { mkr_node_free(pred); return NULL; }
  mkr_step_t *steps = (mkr_step_t *)mkr_callocarray(1, sizeof(mkr_step_t));
  if (steps == NULL) {
    mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "out of memory (css)");
    mkr_node_free(pred); mkr_node_free(n); return NULL;
  }
  mkr_node_t **pv = (mkr_node_t **)mkr_callocarray(1, sizeof(mkr_node_t *));
  if (pv == NULL) {
    mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "out of memory (css)");
    free(steps); mkr_node_free(pred); mkr_node_free(n); return NULL;
  }
  pv[0] = pred;
  steps[0].axis = MKR_AXIS_CHILD;
  steps[0].test.kind = MKR_NT_TEXT;
  steps[0].predicates = pv;
  steps[0].npredicates = 1;
  n->u.path.absolute = 0;
  n->u.path.steps = steps;
  n->u.path.nsteps = 1;
  return n;
}

/* Functional pseudo-classes: :nth-*(an+b), :not(), :is()/:where(), :has(). */
static mkr_node_t *
lower_pseudo_func(css_build_t *B, const lxb_css_selector_t *s, const mkr_step_t *step)
{
  unsigned type = s->u.pseudo.type;

  switch (type) {
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_CHILD:
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_LAST_CHILD:
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_OF_TYPE:
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_LAST_OF_TYPE: {
      const lxb_css_selector_anb_of_t *anb = (const lxb_css_selector_anb_of_t *)s->u.pseudo.data;
      if (anb == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX, "malformed :nth-*()"); return NULL; }
      if (anb->of != NULL) {
        mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX, ":nth-*(... of S) is not supported");
        return NULL;
      }
      int last = (type == LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_LAST_CHILD
               || type == LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_LAST_OF_TYPE);
      int of_type = (type == LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_OF_TYPE
                  || type == LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NTH_LAST_OF_TYPE);
      mkr_axis_t axis = last ? MKR_AXIS_FOLLOWING_SIBLING : MKR_AXIS_PRECEDING_SIBLING;
      const mkr_nodetest_t *named = NULL;
      int oftype_untyped = 0;
      if (of_type) {
        /* Typed (a:nth-of-type) counts same-name siblings via a literal name;
         * untyped (:nth-of-type) compares the element's own expanded name at
         * eval time through the internal of-type-pos fn. */
        if (step->test.kind == MKR_NT_NAME) named = &step->test;
        else oftype_untyped = 1;
      }
      return cb_nth(B, axis, named, oftype_untyped, anb->anb.a, anb->anb.b);
    }

    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_NOT: {
      mkr_node_t *inner = lower_selector_list_selftest(B,
                              (const lxb_css_selector_list_t *)s->u.pseudo.data);
      if (inner == NULL) return NULL;
      mkr_node_t **a = cb_args(1);
      if (a == NULL) { mkr_node_free(inner); mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
      a[0] = inner;
      return cb_fncall(B, "not", a, 1);
    }
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_IS:
    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_WHERE:
      return lower_selector_list_selftest(B,
                 (const lxb_css_selector_list_t *)s->u.pseudo.data);

    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_HAS: {
      /* OR of relative descendant/child paths; truthy when any matches. */
      const lxb_css_selector_list_t *list = (const lxb_css_selector_list_t *)s->u.pseudo.data;
      mkr_node_t *acc = NULL;
      for (const lxb_css_selector_list_t *g = list; g != NULL; g = g->next) {
        /* relative to self: honours a leading >, +, ~ (else descendant) */
        mkr_node_t *path = lower_complex(B, g->first, 1);
        if (path == NULL) { mkr_node_free(acc); return NULL; }
        acc = (acc == NULL) ? path : cb_binop(B, MKR_OP_OR, acc, path);
        if (acc == NULL) return NULL;
      }
      return acc;
    }

    case LXB_CSS_SELECTOR_PSEUDO_CLASS_FUNCTION_LEXBOR_CONTAINS: {
      /* :lexbor-contains("t") -> child::text()[contains(., "t")]: true when a
       * direct child text node contains t. This mirrors Lexbor's matcher, which
       * scans only the element's immediate child TEXT nodes (NOT the deep
       * string-value), so the XML path agrees with the HTML one. The `i` flag is
       * ASCII case-insensitive: fold each side with translate(). Lexbor's own
       * selector; not a CSS standard. The inner "." is the child text node. */
      const lxb_css_selector_contains_t *c =
          (const lxb_css_selector_contains_t *)s->u.pseudo.data;
      if (c == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX, "malformed :lexbor-contains()"); return NULL; }
      const char *needle = (const char *)c->str.data;
      size_t      nlen   = c->str.length;

      if (!c->insensitive) {
        mkr_node_t **a = cb_args(2);
        if (a == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
        a[0] = cb_step_path(B, MKR_AXIS_SELF, MKR_NT_NODE, NULL, 0);  /* "." */
        a[1] = cb_literal(B, needle, nlen);
        return cb_child_text_pred(B, cb_fncall(B, "contains", a, 2));
      }

      /* ASCII case-insensitive: contains(translate(., A-Z, a-z), lower(needle)) */
      static const char UPPER[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
      static const char LOWER[] = "abcdefghijklmnopqrstuvwxyz";
      char *low = mkr_str_alloc(nlen);
      if (low == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
      for (size_t i = 0; i < nlen; i++) {
        unsigned char ch = (unsigned char)needle[i];
        low[i] = (ch >= 'A' && ch <= 'Z') ? (char)(ch - 'A' + 'a') : (char)ch;
      }
      mkr_node_t **ta = cb_args(3);
      if (ta == NULL) { free(low); mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
      ta[0] = cb_step_path(B, MKR_AXIS_SELF, MKR_NT_NODE, NULL, 0);
      ta[1] = cb_literal(B, UPPER, sizeof(UPPER) - 1);
      ta[2] = cb_literal(B, LOWER, sizeof(LOWER) - 1);
      mkr_node_t *folded = cb_fncall(B, "translate", ta, 3);
      mkr_node_t **a = cb_args(2);
      if (a == NULL) { free(low); mkr_node_free(folded); mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); return NULL; }
      a[0] = folded;
      a[1] = cb_literal(B, low, nlen);
      free(low);
      return cb_child_text_pred(B, cb_fncall(B, "contains", a, 2));
    }

    default:
      mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX, "unsupported functional CSS pseudo-class");
      return NULL;
  }
}

/* Fold one simple selector into the current step (sets nodetest for a type, else
 * appends a predicate). Returns 0 / -1. */
static int
fold_simple(css_build_t *B, const lxb_css_selector_t *s, mkr_step_t *step,
            preds_arr_t *preds)
{
  switch (s->type) {
    case LXB_CSS_SELECTOR_TYPE_ANY:
    case LXB_CSS_SELECTOR_TYPE_ELEMENT:
      return lower_type(B, s, step, preds);

    case LXB_CSS_SELECTOR_TYPE_ID:          /* #id -> @id = 'id' */
      return preds_push(B, preds,
        cb_binop(B, MKR_OP_EQ, cb_attr(B, "id", 2),
                 cb_literal(B, (const char *)s->name.data, s->name.length)));

    case LXB_CSS_SELECTOR_TYPE_CLASS:       /* .class -> token match on @class */
      return preds_push(B, preds,
        cb_token_match(B, NULL, 0, "class", 5, (const char *)s->name.data, s->name.length));

    case LXB_CSS_SELECTOR_TYPE_ATTRIBUTE:
      return preds_push(B, preds, lower_attribute(B, s));

    case LXB_CSS_SELECTOR_TYPE_PSEUDO_CLASS:
      return preds_push(B, preds, lower_pseudo_simple(B, s, step));

    case LXB_CSS_SELECTOR_TYPE_PSEUDO_CLASS_FUNCTION:
      return preds_push(B, preds, lower_pseudo_func(B, s, step));

    case LXB_CSS_SELECTOR_TYPE_PSEUDO_ELEMENT:
    case LXB_CSS_SELECTOR_TYPE_PSEUDO_ELEMENT_FUNCTION:
      mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX, "CSS pseudo-elements are not selectable");
      return -1;

    default:
      mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX, "unsupported CSS selector component");
      return -1;
  }
}

/* Axis connecting a compound to its predecessor, from the leading combinator. */
static mkr_axis_t
axis_for_combinator(lxb_css_selector_combinator_t c, int is_first)
{
  if (is_first) return MKR_AXIS_DESCENDANT;       /* relative to context, descendant-only */
  switch (c) {
    case LXB_CSS_SELECTOR_COMBINATOR_CHILD:     return MKR_AXIS_CHILD;
    case LXB_CSS_SELECTOR_COMBINATOR_FOLLOWING: return MKR_AXIS_FOLLOWING_SIBLING; /* ~ */
    case LXB_CSS_SELECTOR_COMBINATOR_DESCENDANT:
    default:                                    return MKR_AXIS_DESCENDANT;
  }
}

/* Build one step for a compound [cfirst .. clast] (inclusive chain), with the
 * given axis, appending it to +steps+. Returns 0 / -1. */
static int
emit_compound_step(css_build_t *B, steps_arr_t *steps, mkr_axis_t axis,
                   const lxb_css_selector_t *cfirst, const lxb_css_selector_t *clast)
{
  mkr_step_t step; memset(&step, 0, sizeof(step));
  step.axis = axis;
  step.test.kind = MKR_NT_WILDCARD;   /* default; a type simple overrides */
  preds_arr_t preds = {0};

  for (const lxb_css_selector_t *s = cfirst; ; s = s->next) {
    if (fold_simple(B, s, &step, &preds) != 0) {
      for (size_t i = 0; i < preds.n; i++) mkr_node_free(preds.v[i]);
      free(preds.v);
      mkr_owned_text_clear(&step.test.local);
      mkr_owned_text_clear(&step.test.prefix);
      return -1;
    }
    if (s == clast) break;
  }
  step.predicates = preds.v;
  step.npredicates = preds.n;
  if (steps_push(B, steps, step) != 0) {
    for (size_t i = 0; i < preds.n; i++) mkr_node_free(preds.v[i]);
    free(preds.v);
    mkr_owned_text_clear(&step.test.local);
    mkr_owned_text_clear(&step.test.prefix);
    return -1;
  }
  return 0;
}

/* Lower one complex selector (a chain) into a relative PATH node. */
static mkr_node_t *
lower_complex(css_build_t *B, const lxb_css_selector_t *first, int relative_first)
{
  steps_arr_t steps = {0};
  const lxb_css_selector_t *cstart = first;

  for (const lxb_css_selector_t *s = first; s != NULL; s = s->next) {
    const lxb_css_selector_t *nxt = s->next;
    int compound_ends = (nxt == NULL) || (nxt->combinator != LXB_CSS_SELECTOR_COMBINATOR_CLOSE);
    if (!compound_ends) continue;

    /* In relative mode the first compound honours its own combinator (so it can
     * be a child/adjacent/general-sibling of the context), not a forced descendant. */
    int is_first = (cstart == first) && !relative_first;
    lxb_css_selector_combinator_t comb = cstart->combinator;

    if (!is_first && comb == LXB_CSS_SELECTOR_COMBINATOR_SIBLING) {
      /* a + b  ->  following-sibling::*[1] / self::b  (two steps) */
      mkr_step_t fs; memset(&fs, 0, sizeof(fs));
      fs.axis = MKR_AXIS_FOLLOWING_SIBLING;
      fs.test.kind = MKR_NT_WILDCARD;
      mkr_node_t **p = (mkr_node_t **)mkr_callocarray(1, sizeof(mkr_node_t *));
      if (p == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); goto fail; }
      p[0] = cb_num(B, 1.0);
      if (p[0] == NULL) { free(p); goto fail; }
      fs.predicates = p; fs.npredicates = 1;
      if (steps_push(B, &steps, fs) != 0) { mkr_node_free(p[0]); free(p); goto fail; }
      if (emit_compound_step(B, &steps, MKR_AXIS_SELF, cstart, s) != 0) goto fail;
    } else {
      mkr_axis_t axis = axis_for_combinator(comb, is_first);
      if (emit_compound_step(B, &steps, axis, cstart, s) != 0) goto fail;
    }
    cstart = nxt;
  }

  mkr_node_t *path = cb_node(B, MKR_NK_PATH);
  if (path == NULL) goto fail;
  path->u.path.absolute = 0;
  path->u.path.steps = steps.v;
  path->u.path.nsteps = steps.n;
  return path;

fail:
  for (size_t i = 0; i < steps.n; i++) mkr_step_clear(&steps.v[i]);
  free(steps.v);
  return NULL;
}

/* Boolean expression: does SELF match the compound [cfirst..clast]? Reuses
 * fold_simple to lower the compound's simples (type -> nodetest, others ->
 * self-relative predicates), then combines as self::<type> AND pred1 AND ...
 * Used by :is()/:where()/:not(). */
/* The reverse of a forward combinator, for walking from the subject (self) back
 * to the preceding compound. Adjacent (+) is handled separately (two steps). */
static mkr_axis_t
reverse_axis(lxb_css_selector_combinator_t c)
{
  switch (c) {
    case LXB_CSS_SELECTOR_COMBINATOR_CHILD:     return MKR_AXIS_PARENT;
    case LXB_CSS_SELECTOR_COMBINATOR_FOLLOWING: return MKR_AXIS_PRECEDING_SIBLING; /* ~ */
    case LXB_CSS_SELECTOR_COMBINATOR_DESCENDANT:
    default:                                    return MKR_AXIS_ANCESTOR;
  }
}

typedef struct {
  const lxb_css_selector_t      *first, *last;
  lxb_css_selector_combinator_t  comb;   /* how this compound connects to its left neighbour */
} mkr_css_compound_t;

static mkr_node_t *
lower_complex_selftest(css_build_t *B, const lxb_css_selector_t *first)
{
  /* Split the chain into compounds (CLOSE-linked runs), left to right. */
  mkr_css_compound_t comps[MKR_CSS_MAX_COMPOUNDS];
  size_t nc = 0;
  const lxb_css_selector_t *cstart = first;
  for (const lxb_css_selector_t *s = first; s != NULL; s = s->next) {
    const lxb_css_selector_t *nxt = s->next;
    if (nxt != NULL && nxt->combinator == LXB_CSS_SELECTOR_COMBINATOR_CLOSE) continue;
    if (nc >= MKR_CSS_MAX_COMPOUNDS) {
      mkr_err_set(B->err, MKR_XPATH_ERR_LIMIT, "CSS selector too complex");
      return NULL;
    }
    comps[nc].first = cstart; comps[nc].last = s; comps[nc].comb = cstart->combinator;
    nc++;
    cstart = nxt;
  }
  if (nc == 0) { mkr_err_set(B->err, MKR_XPATH_ERR_SYNTAX, "empty CSS selector"); return NULL; }

  /* Build self::<subject> then reverse back-steps to each earlier compound. The
   * whole path is non-empty (truthy) exactly when self matches the selector. */
  steps_arr_t steps = {0};
  if (emit_compound_step(B, &steps, MKR_AXIS_SELF, comps[nc - 1].first, comps[nc - 1].last) != 0) {
    goto fail;
  }
  for (size_t i = nc - 1; i > 0; i--) {
    lxb_css_selector_combinator_t comb = comps[i].comb; /* connects comps[i] to comps[i-1] */
    if (comb == LXB_CSS_SELECTOR_COMBINATOR_SIBLING) {
      /* reverse adjacent: the immediately-preceding sibling must be comps[i-1] */
      mkr_step_t ps; memset(&ps, 0, sizeof(ps));
      ps.axis = MKR_AXIS_PRECEDING_SIBLING;
      ps.test.kind = MKR_NT_WILDCARD;
      mkr_node_t **p = (mkr_node_t **)mkr_callocarray(1, sizeof(mkr_node_t *));
      if (p == NULL) { mkr_err_set(B->err, MKR_XPATH_ERR_OOM, "oom"); goto fail; }
      p[0] = cb_num(B, 1.0);
      if (p[0] == NULL) { free(p); goto fail; }
      ps.predicates = p; ps.npredicates = 1;
      if (steps_push(B, &steps, ps) != 0) { mkr_node_free(p[0]); free(p); goto fail; }
      if (emit_compound_step(B, &steps, MKR_AXIS_SELF, comps[i - 1].first, comps[i - 1].last) != 0) {
        goto fail;
      }
    } else {
      if (emit_compound_step(B, &steps, reverse_axis(comb),
                             comps[i - 1].first, comps[i - 1].last) != 0) {
        goto fail;
      }
    }
  }

  mkr_node_t *path = cb_node(B, MKR_NK_PATH);
  if (path == NULL) goto fail;
  path->u.path.absolute = 0;
  path->u.path.steps = steps.v;
  path->u.path.nsteps = steps.n;
  return path;

fail:
  for (size_t i = 0; i < steps.n; i++) mkr_step_clear(&steps.v[i]);
  free(steps.v);
  return NULL;
}

mkr_node_t *
mkr_css_compile(mkr_verified_text_t selector, const mkr_css_ns_t *ns,
                mkr_xpath_limits_t *limits, mkr_xpath_error_t *err)
{
  css_build_t B = { limits, err, ns };

  if (css_has_leading_pipe(selector.ptr, selector.len)) {
    mkr_err_set(err, MKR_XPATH_ERR_SYNTAX,
                "CSS |el (no-namespace) type selector is not supported "
                "(Lexbor cannot distinguish it from *|el); use a bare type "
                "selector or XPath for a strict no-namespace match");
    return NULL;
  }

  if (!css_parser_ready()) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "failed to initialise CSS parser");
    return NULL;
  }

  lxb_css_selector_list_t *list =
      lxb_css_selectors_parse(g_parser, (const lxb_char_t *)selector.ptr, selector.len);
  if (list == NULL || g_parser->status != LXB_STATUS_OK) {
    lxb_css_memory_clean(g_mem);
    lxb_css_parser_clean(g_parser);
    mkr_err_set(err, MKR_XPATH_ERR_SYNTAX, "invalid CSS selector");
    return NULL;
  }

  /* Lower each comma-group (list -> list->next) to a PATH, union them. */
  mkr_node_t *acc = NULL;
  for (lxb_css_selector_list_t *g = list; g != NULL; g = g->next) {
    mkr_node_t *path = lower_complex(&B, g->first, 0);  /* top-level: descendant of context */
    if (path == NULL) { mkr_node_free(acc); acc = NULL; break; }
    acc = (acc == NULL) ? path : cb_binop(&B, MKR_OP_UNION, acc, path);
    if (acc == NULL) break;  /* cb_binop freed both on failure */
  }

  lxb_css_memory_clean(g_mem);
  lxb_css_parser_clean(g_parser);
  return acc;  /* NULL with *err set on failure */
}
