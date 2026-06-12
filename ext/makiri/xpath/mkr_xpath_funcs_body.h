#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"

#include <lexbor/dom/dom.h>
#include <lexbor/encoding/decode.h>
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/*
 * Built-in XPath 1.0 functions. Phase 1 covers: last, position,
 * count, not, true, false, id. The remaining ~23 standard
 * functions are added in Phase 2.
 */

/* Character-counting (mkr_utf8_count_chars), character-advance
 * (mkr_utf8_advance_chars) and substring search (mkr_bytes_find) are
 * length-bounded core primitives - see core/mkr_utf8.h and core/mkr_span.h.
 * The byte-equality compares below go through mkr_bytes_eq (core/mkr_span.h). */

static int
arity_check(size_t got, size_t want_min, size_t want_max, mkr_xpath_error_t *err, const char *name)
{
  if (got < want_min || got > want_max) {
    if (want_min == want_max) {
      mkr_err_setf(err, MKR_XPATH_ERR_RUNTIME, "%s(): expected %zu argument(s), got %zu", name, want_min, got);
    } else {
      mkr_err_setf(err, MKR_XPATH_ERR_RUNTIME, "%s(): expected %zu-%zu argument(s), got %zu", name, want_min, want_max, got);
    }
    return -1;
  }
  return 0;
}

/* Require +arg+ to be a node-set; returns it, or NULL with a TYPE error naming
 * +fname+. The single front for the "argument must be a node-set" check that
 * count()/sum()/the name functions all share. */
static const mkr_nodeset_t *
require_nodeset(const mkr_val_t *arg, const char *fname, mkr_xpath_error_t *err)
{
  if (arg->type != MKR_XPATH_TYPE_NODESET) {
    mkr_err_setf(err, MKR_XPATH_ERR_TYPE, "%s(): argument must be a node-set", fname);
    return NULL;
  }
  return &arg->u.nodeset;
}

/* string-value of args[0], or of the context node when there is no argument -
 * the "optional node-set arg defaults to self, as a string" idiom shared by
 * string() / string-length() / normalize-space(). */
static int
fn_string_arg_or_self(MKR_DOM_NODE *self_node, const mkr_val_t *args, size_t nargs,
                      mkr_xpath_limits_t *L, mkr_xpath_error_t *err, mkr_owned_text_t *out)
{
  return (nargs == 0)
      ? mkr_node_to_owned_text_or_fail(self_node, L, err, out)
      : mkr_val_to_owned_text_or_fail(&args[0], L, err, out);
}

static int
fn_last(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
        mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)args;
  if (arity_check(nargs, 0, 0, err, "last") != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = (double)focus->size;
  return 0;
}

static int
fn_position(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
            mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)args;
  if (arity_check(nargs, 0, 0, err, "position") != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = (double)focus->pos;
  return 0;
}

static int
fn_count(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
         mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)focus;
  if (arity_check(nargs, 1, 1, err, "count") != 0) return -1;
  const mkr_nodeset_t *ns = require_nodeset(&args[0], "count", err);
  if (ns == NULL) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = (double)ns->count;
  return 0;
}

static int
fn_not(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
       mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)focus;
  if (arity_check(nargs, 1, 1, err, "not") != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = !mkr_val_to_boolean(&args[0]);
  return 0;
}

static int
fn_true(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
        mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)focus; (void)args;
  if (arity_check(nargs, 0, 0, err, "true") != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = 1;
  return 0;
}

static int
fn_false(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
         mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)focus; (void)args;
  if (arity_check(nargs, 0, 0, err, "false") != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = 0;
  return 0;
}

#ifndef MKR_HOST_XML
/* id() helpers are HTML-only: the XML instance's id() is the empty node-set
 * (no DTD-declared IDs, §8.6), so it never walks the tree by id attribute.
 * Guarded so the XML engine TU does not carry (and warn about) dead code. */

/* id(string|nodeset) - looks up by HTML id attribute. Walks the whole tree per
 * token, so it charges each visited node to the op budget and fails closed on
 * overrun (returns -1, *out untouched-meaningful); a hit sets *out, a miss
 * leaves *out NULL. Without this, id() over a large node-set (a token per node,
 * a tree walk per token) drives O(nodes^2) work as ~0 ops. */
static int
find_by_id(MKR_DOM_NODE *root, const char *id, size_t id_len,
           mkr_xpath_limits_t *L, mkr_xpath_error_t *err, MKR_DOM_NODE **out)
{
  *out = NULL;
  if (root == NULL || id == NULL || id_len == 0) return 0;
  MKR_DOM_NODE *n = root;
  while (n) {
    if (mkr_limit_eval_op(L, err) != 0) return -1;
    if (MKR_NODE_TYPE(n) == MKR_NTYPE_ELEMENT) {
      MKR_DOM_ELEMENT *el = MKR_NODE_AS_ELEMENT(n);
      size_t vlen = 0;
      const lxb_char_t *v = MKR_ELEM_GET_ATTRIBUTE(el, "id", 2, &vlen);
      if (v && mkr_bytes_eq(v, vlen, id, id_len)) {
        *out = n;
        return 0;
      }
    }
    if (MKR_NODE_FIRST_CHILD(n)) {
      n = MKR_NODE_FIRST_CHILD(n);
    } else {
      while (n && n != root && MKR_NODE_NEXT(n) == NULL) n = MKR_NODE_PARENT(n);
      if (n == root || n == NULL) break;
      n = MKR_NODE_NEXT(n);
    }
  }
  return 0;
}

/*
 * Look up each whitespace-separated token in the borrowed text 's' and push
 * every hit into 'out'. Duplicates are pushed unconditionally - the caller (fn_id) deduplicates
 * the entire result via sort + adjacent pass after all tokens have been
 * processed, which is O(n log n) versus O(n^2) per-insert contains() checks.
 *
 * Length-bounded over an mkr_span_t: each token is captured as a borrowed slice
 * (mark/since) and handed to find_by_id by ptr+len, so the buffer is never
 * mutated (the earlier temp-NUL splice was vestigial - find_by_id already
 * compares by explicit length, never as a C string).
 */
static int
id_collect_from_string(mkr_borrowed_text_t s, MKR_DOM_NODE *root,
                       mkr_nodeset_t *out, mkr_xpath_context_t *ctx,
                       mkr_xpath_error_t *err)
{
  mkr_span_t sp = mkr_span(s.ptr, s.len);
  for (;;) {
    mkr_span_skip_xpath_ws(&sp);
    if (mkr_span_peek(&sp) < 0) break;
    const char *tok = mkr_span_mark(&sp);
    while (mkr_span_peek(&sp) >= 0 && !mkr_xpath_is_ws(mkr_span_peek(&sp)))
      mkr_span_skip(&sp, 1);
    size_t tok_len = mkr_span_since(&sp, tok);
    MKR_DOM_NODE *hit;
    if (find_by_id(root, tok, tok_len, mkr_ctx_limits(ctx), err, &hit) != 0) {
      return -1;
    }
    if (hit && mkr_nodeset_push(out, hit, mkr_ctx_limits(ctx), err) != 0) {
      return -1;
    }
  }
  return 0;
}
#endif /* !MKR_HOST_XML */

static int
fn_id(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
      mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 1, 1, err, "id") != 0) return -1;
  out->type = MKR_XPATH_TYPE_NODESET;
  mkr_nodeset_init(&out->u.nodeset);

#ifdef MKR_HOST_XML
  /* Host policy (§8.6): in XML an ID is an attribute DECLARED ID-typed by the
   * DTD/schema - not any attribute named "id". DTDs are rejected at parse
   * (§9.4), so a document read here carries no ID-typed attributes and id() is
   * the empty node-set. (xml:id is a separate, optional spec, not implemented.) */
  (void)ctx; (void)args;
  return 0;
#else
  MKR_DOM_NODE *root = (MKR_DOM_NODE *)mkr_ctx_document(ctx);
  if (root == NULL) return 0;

  /* XPath 1.0 §4.1: id() argument may be a node-set; in that case each
   * node's string-value is treated as IDREFS - whitespace-separated tokens.
   * For non-node-set, the value is converted to string and split likewise. */
  if (args[0].type == MKR_XPATH_TYPE_NODESET) {
    for (size_t i = 0; i < args[0].u.nodeset.count; ++i) {
      mkr_owned_text_t text;
      if (mkr_node_to_owned_text_or_fail(args[0].u.nodeset.items[i], mkr_ctx_limits(ctx), err, &text) != 0) {
        mkr_nodeset_clear(&out->u.nodeset);
        return -1;
      }
      int rc = id_collect_from_string(mkr_borrowed_text_from_owned(text), root, &out->u.nodeset, ctx, err);
      mkr_owned_text_clear(&text);
      if (rc != 0) { mkr_nodeset_clear(&out->u.nodeset); return -1; }
    }
  } else {
    mkr_owned_text_t text;
    if (mkr_val_to_owned_text_or_fail(&args[0], mkr_ctx_limits(ctx), err, &text) != 0) {
      mkr_nodeset_clear(&out->u.nodeset);
      return -1;
    }
    int rc = id_collect_from_string(mkr_borrowed_text_from_owned(text), root, &out->u.nodeset, ctx, err);
    mkr_owned_text_clear(&text);
    if (rc != 0) { mkr_nodeset_clear(&out->u.nodeset); return -1; }
  }
  /* Result is in document order with duplicates removed (XPath 1.0 §4.1). */
  mkr_nodeset_unique_sorted(ctx, &out->u.nodeset);
  return 0;
#endif /* MKR_HOST_XML */
}

/* Forward decl for two_owned_texts() defined later. */
static int two_owned_texts(mkr_xpath_context_t *ctx, mkr_val_t *args,
                           mkr_owned_text_t *sa, mkr_owned_text_t *sb,
                           mkr_xpath_error_t *err);

/* ---------- nokogiri-builtin namespace ---------- */

/*
 * css-class(haystack, needle) - true iff 'needle' is a whitespace-
 * separated token of 'haystack'. The libxml2 version of this function
 * ships in nl_xpath_context.c (builtin_css_class); we keep behavior
 * identical.
 */
static int
ws_token_match(mkr_borrowed_text_t hay, mkr_borrowed_text_t val)
{
  /* NULL ordering preserved from the original: a NULL haystack/needle is a
   * non-match BEFORE the empty-needle short-circuit (so empty val + NULL hay
   * stays 0, not 1). */
  if (hay.ptr == NULL || val.ptr == NULL) return 0;
  if (val.len == 0) return 1; /* libxml2 returns non-NULL for empty val */

  mkr_span_t sp = mkr_span(hay.ptr, hay.len);
  while (mkr_span_peek(&sp) >= 0) {
    const char *tok = mkr_span_mark(&sp);
    while (mkr_span_peek(&sp) >= 0 && !mkr_xpath_is_ws(mkr_span_peek(&sp)))
      mkr_span_skip(&sp, 1);
    if (mkr_bytes_eq(tok, mkr_span_since(&sp, tok), val.ptr, val.len)) {
      return 1;
    }
    mkr_span_skip_xpath_ws(&sp);
  }
  return 0;
}

static int
fn_nokogiri_css_class(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
                     mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 2, 2, err, "nokogiri-builtin:css-class") != 0) return -1;
  mkr_owned_text_t hay, needle;
  if (two_owned_texts(ctx, args, &hay, &needle, err) != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = ws_token_match(mkr_borrowed_text_from_owned(hay),
                                 mkr_borrowed_text_from_owned(needle));
  mkr_owned_text_clear(&hay);
  mkr_owned_text_clear(&needle);
  return 0;
}

/*
 * local-name-is(elementName) - true iff the context node's qualified
 * name (for HTML this is the lowercase local name) equals the argument.
 */
static int
fn_nokogiri_local_name_is(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
                          mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  if (arity_check(nargs, 1, 1, err, "nokogiri-builtin:local-name-is") != 0) return -1;
  mkr_owned_text_t want;
  if (mkr_val_to_owned_text_or_fail(&args[0], mkr_ctx_limits(ctx), err, &want) != 0) return -1;

  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = 0;
  if (focus->node != NULL) {
    size_t got_len = 0;
    const lxb_char_t *got = MKR_ELEM_QUALIFIED_NAME(focus->node, &got_len);
    if (got != NULL) {
      out->u.boolean = mkr_bytes_eq(got, got_len, want.ptr, want.len);
    }
  }
  mkr_owned_text_clear(&want);
  return 0;
}

typedef struct {
  const char     *name;
  mkr_func_impl_t  fn;
} fn_entry_t;

static const fn_entry_t fn_table_nokogiri_builtin[] = {
  { "css-class",     fn_nokogiri_css_class     },
  { "local-name-is", fn_nokogiri_local_name_is },
  { NULL,            NULL                      },
};

#ifdef MKR_HOST_XML
/* ---------- internal: untyped :*-of-type position (XML, CSS-only) ---------- *
 * Two elements are the same "type" iff they share an expanded name (local name
 * + namespace URI). These back the CSS-lowered untyped of-type pseudo-classes;
 * see MKR_FN_OF_TYPE_POS in mkr_xpath_internal.h for why they cannot be a pure
 * XPath expression. focus->node is the context element being filtered. */
static int
mkr_xml_same_type(MKR_DOM_NODE *a, MKR_DOM_NODE *b)
{
  size_t al = 0, bl = 0;
  const lxb_char_t *an = MKR_ELEM_LOCAL_NAME(a, &al);
  const lxb_char_t *bn = MKR_ELEM_LOCAL_NAME(b, &bl);
  if (!mkr_bytes_eq(an, al, bn, bl)) return 0;
  size_t au = 0, bu = 0;
  const char *ap = MKR_NODE_NS_URI(a, NULL, &au);
  const char *bp = MKR_NODE_NS_URI(b, NULL, &bu);
  return mkr_bytes_eq(ap, au, bp, bu);
}

/* 1-based position of +self+ among its same-type element siblings; forward walks
 * preceding siblings (position from the start), else following (from the end). */
static double
mkr_xml_of_type_pos(MKR_DOM_NODE *self, int forward)
{
  if (self == NULL || MKR_NODE_TYPE(self) != MKR_NTYPE_ELEMENT) return 0.0;
  long pos = 1;
  for (MKR_DOM_NODE *s = forward ? MKR_NODE_PREV(self) : MKR_NODE_NEXT(self);
       s != NULL; s = forward ? MKR_NODE_PREV(s) : MKR_NODE_NEXT(s)) {
    if (MKR_NODE_TYPE(s) == MKR_NTYPE_ELEMENT && mkr_xml_same_type(self, s)) pos++;
  }
  return (double)pos;
}

static int
fn_xml_of_type_pos(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
                   mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)args; (void)nargs; (void)err;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = mkr_xml_of_type_pos(focus->node, 1);
  return 0;
}

static int
fn_xml_of_type_pos_last(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
                        mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)args; (void)nargs; (void)err;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = mkr_xml_of_type_pos(focus->node, 0);
  return 0;
}
#endif /* MKR_HOST_XML */

static int
fn_boolean(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
           mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)focus;
  if (arity_check(nargs, 1, 1, err, "boolean") != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = mkr_val_to_boolean(&args[0]);
  return 0;
}

/* ---------- string functions ---------- */

static int
fn_string(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
          mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  if (arity_check(nargs, 0, 1, err, "string") != 0) return -1;
  out->type = MKR_XPATH_TYPE_STRING;
  mkr_owned_text_t text;
  if (fn_string_arg_or_self(focus->node, args, nargs, mkr_ctx_limits(ctx), err, &text) != 0) return -1;
  mkr_val_set_owned_text(out, text);
  return 0;
}

static int
fn_concat(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
          mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (nargs < 2) {
    mkr_err_set(err, MKR_XPATH_ERR_RUNTIME, "concat(): expected at least 2 arguments");
    return -1;
  }
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  mkr_owned_text_t *parts = mkr_callocarray(nargs, sizeof(*parts));
  if (parts == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in concat()");
    return -1;
  }
  size_t total = 0;
  for (size_t i = 0; i < nargs; ++i) {
    if (mkr_val_to_owned_text_or_fail(&args[i], L, err, &parts[i]) != 0) {
      for (size_t j = 0; j < i; ++j) mkr_owned_text_clear(&parts[j]);
      free(parts);
      return -1;
    }
    if (!mkr_size_add(total, parts[i].len, &total)) {
      for (size_t j = 0; j <= i; ++j) mkr_owned_text_clear(&parts[j]);
      free(parts);
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "concat() size overflow");
      return -1;
    }
    if (mkr_limit_check_string_bytes(L, total, err) != 0) {
      for (size_t j = 0; j <= i; ++j) mkr_owned_text_clear(&parts[j]);
      free(parts);
      return -1;
    }
  }
  char *buf = mkr_str_alloc(total);
  if (buf == NULL) {
    for (size_t i = 0; i < nargs; ++i) mkr_owned_text_clear(&parts[i]);
    free(parts);
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in concat()");
    return -1;
  }
  size_t off = 0;
  for (size_t i = 0; i < nargs; ++i) {
    memcpy(buf + off, parts[i].ptr, parts[i].len);
    off += parts[i].len;
    mkr_owned_text_clear(&parts[i]);
  }
  buf[off] = '\0';
  free(parts);
  mkr_val_set_owned_text(out, mkr_owned_text(buf, total));
  return 0;
}

/* Helper: pull two string operands. Returns 0 + populated *sa, *sb,
 * or -1 with err set and any partially-allocated argument freed. */
static int
two_owned_texts(mkr_xpath_context_t *ctx, mkr_val_t *args,
                mkr_owned_text_t *sa, mkr_owned_text_t *sb,
                mkr_xpath_error_t *err)
{
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  if (mkr_val_to_owned_text_or_fail(&args[0], L, err, sa) != 0) return -1;
  if (mkr_val_to_owned_text_or_fail(&args[1], L, err, sb) != 0) {
    mkr_owned_text_clear(sa);
    return -1;
  }
  return 0;
}

static int
fn_starts_with(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
               mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 2, 2, err, "starts-with") != 0) return -1;
  mkr_owned_text_t s, t;
  if (two_owned_texts(ctx, args, &s, &t, err) != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  mkr_span_t ss = mkr_span(s.ptr, s.len);
  out->u.boolean = mkr_span_starts(&ss, t.ptr, t.len);
  mkr_owned_text_clear(&s);
  mkr_owned_text_clear(&t);
  return 0;
}

static int
fn_contains(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
            mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 2, 2, err, "contains") != 0) return -1;
  mkr_owned_text_t s, t;
  if (two_owned_texts(ctx, args, &s, &t, err) != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  size_t idx;
  out->u.boolean = mkr_bytes_find(s.ptr, s.len, t.ptr, t.len, &idx);
  mkr_owned_text_clear(&s);
  mkr_owned_text_clear(&t);
  return 0;
}

static int
fn_substring_before(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
                    mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 2, 2, err, "substring-before") != 0) return -1;
  mkr_owned_text_t s, t;
  if (two_owned_texts(ctx, args, &s, &t, err) != 0) return -1;
  /* substring-before(s, t): bytes of s up to the first occurrence of t, or ""
   * when t is empty or absent. */
  size_t n = 0, idx;
  if (t.len != 0 && mkr_bytes_find(s.ptr, s.len, t.ptr, t.len, &idx)) n = idx;
  char *p = mkr_strndup(s.ptr, n); /* n == 0 yields an owned "" */
  mkr_owned_text_clear(&s);
  mkr_owned_text_clear(&t);
  if (p == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in substring-before");
    return -1;
  }
  mkr_val_set_owned_text(out, mkr_owned_text(p, n));
  return 0;
}

static int
fn_substring_after(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
                   mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 2, 2, err, "substring-after") != 0) return -1;
  mkr_owned_text_t s, t;
  if (two_owned_texts(ctx, args, &s, &t, err) != 0) return -1;
  size_t out_len;
  char *p;
  if (t.len == 0) {
    out_len = s.len;
    p = mkr_strndup(s.ptr, s.len);
  } else {
    size_t idx;
    if (mkr_bytes_find(s.ptr, s.len, t.ptr, t.len, &idx)) {
      out_len = s.len - (idx + t.len);
      p = mkr_strndup(s.ptr + idx + t.len, out_len);
    } else {
      out_len = 0;
      p = mkr_strdup("");
    }
  }
  mkr_owned_text_clear(&s);
  mkr_owned_text_clear(&t);
  if (p == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in substring-after");
    return -1;
  }
  mkr_val_set_owned_text(out, mkr_owned_text(p, out_len));
  return 0;
}

/* substring(s, start[, length]) - XPath positions are 1-based and round
 * to nearest; out-of-range positions clip to the string. */
static int
fn_substring(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
             mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 2, 3, err, "substring") != 0) return -1;
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  mkr_owned_text_t s;
  if (mkr_val_to_owned_text_or_fail(&args[0], L, err, &s) != 0) return -1;
  double start_d, end_d;
  if (mkr_val_to_number_or_fail(&args[1], L, err, &start_d) != 0) { mkr_owned_text_clear(&s); return -1; }
  size_t s_chars = mkr_utf8_count_chars(s.ptr, s.len);
  if (nargs == 3) {
    double len_d;
    if (mkr_val_to_number_or_fail(&args[2], L, err, &len_d) != 0) { mkr_owned_text_clear(&s); return -1; }
    end_d = start_d + len_d;
  } else {
    end_d = (double)s_chars + 1;
  }

  size_t out_len = 0;
  char *p;
  if (start_d != start_d || end_d != end_d) {
    p = mkr_strdup(""); /* NaN start/length -> "" */
  } else {
    /* XPath 1.0 §4.2: positions are 1-based character offsets, rounded.
     * substring("12345", 2, 3) → "234"; out-of-range silently clipped.
     * Round, then clamp to the valid range [1, s_chars+1] AS DOUBLES before the
     * cast: start_d/end_d can be ±Infinity or exceed long's range (e.g.
     * `substring(s, 1 div 0)`), where a direct (long)floor(...) is undefined
     * behaviour. NaN is already handled above. */
    double imax_d = (double)s_chars + 1.0;
    double rstart = floor(start_d + 0.5);
    double rend   = floor(end_d + 0.5);
    if (rstart < 1.0)    rstart = 1.0;
    if (rstart > imax_d) rstart = imax_d;
    if (rend < 1.0)      rend = 1.0;
    if (rend > imax_d)   rend = imax_d;
    long start = (long)rstart;
    long end   = (long)rend;
    if (end <= start) {
      p = mkr_strdup("");
    } else {
      size_t start_off = mkr_utf8_advance_chars(s.ptr, s.len, (size_t)(start - 1));
      size_t end_off   = start_off + mkr_utf8_advance_chars(s.ptr + start_off,
                                                            s.len - start_off,
                                                            (size_t)(end - start));
      out_len = end_off - start_off;
      p = mkr_strndup(s.ptr + start_off, out_len);
    }
  }
  mkr_owned_text_clear(&s);
  if (p == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in substring");
    return -1;
  }
  mkr_val_set_owned_text(out, mkr_owned_text(p, out_len));
  return 0;
}

static int
fn_string_length(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
                 mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  if (arity_check(nargs, 0, 1, err, "string-length") != 0) return -1;
  mkr_owned_text_t s;
  if (fn_string_arg_or_self(focus->node, args, nargs, mkr_ctx_limits(ctx), err, &s) != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = (double)mkr_utf8_count_chars(s.ptr, s.len);
  mkr_owned_text_clear(&s);
  return 0;
}

/* normalize-space: collapse runs of whitespace, trim leading/trailing */
static int
fn_normalize_space(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
                   mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  if (arity_check(nargs, 0, 1, err, "normalize-space") != 0) return -1;
  mkr_owned_text_t s;
  if (fn_string_arg_or_self(focus->node, args, nargs, mkr_ctx_limits(ctx), err, &s) != 0) return -1;
  char  *buf = mkr_str_alloc(s.len);
  if (buf == NULL) { mkr_owned_text_clear(&s); mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in normalize-space"); return -1; }
  size_t out_i = 0;
  int in_space = 1;
  for (size_t i = 0; i < s.len; ++i) {
    char c = s.ptr[i];
    int is_ws = (c == ' ' || c == '\t' || c == '\n' || c == '\r');
    if (is_ws) {
      if (!in_space && out_i > 0) buf[out_i++] = ' ';
      in_space = 1;
    } else {
      buf[out_i++] = c;
      in_space = 0;
    }
  }
  if (out_i > 0 && buf[out_i - 1] == ' ') out_i--;
  buf[out_i] = '\0';
  mkr_owned_text_clear(&s);
  mkr_val_set_owned_text(out, mkr_owned_text(buf, out_i));
  return 0;
}

static int
fn_translate(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
             mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 3, 3, err, "translate") != 0) return -1;
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  mkr_owned_text_t s, from, to;
  if (mkr_val_to_owned_text_or_fail(&args[0], L, err, &s) != 0) return -1;
  if (mkr_val_to_owned_text_or_fail(&args[1], L, err, &from) != 0) { mkr_owned_text_clear(&s); return -1; }
  if (mkr_val_to_owned_text_or_fail(&args[2], L, err, &to) != 0) {
    mkr_owned_text_clear(&s); mkr_owned_text_clear(&from); return -1;
  }

  /* translate() works on CHARACTERS (Unicode code points), not bytes: each
   * code point of `s` that appears in `from` is replaced by the code point at
   * the same position in `to`, or removed if `from` is longer than `to`. We
   * decode with Lexbor's UTF-8 decoder (input is valid UTF-8 from the DOM or a
   * string literal) and, to avoid re-encoding, copy the original byte spans of
   * the matched `to` / passed-through `s` characters. */
  /* `from` code points, and `to` characters as (offset, byte length) spans. */
  lxb_codepoint_t *from_cp = mkr_reallocarray(NULL, from.len + 1, sizeof(*from_cp));
  size_t *to_off = mkr_reallocarray(NULL, to.len + 1, sizeof(*to_off));
  size_t *to_clen = mkr_reallocarray(NULL, to.len + 1, sizeof(*to_clen));
  /* Output: capped (a multibyte replacement can expand the result past the
   * limit even when the input is within it, e.g. "a" -> "😀"), grown
   * overflow-safe; append fails closed with LIMIT/OOM. */
  mkr_buf_t buf;
  mkr_buf_init(&buf, L->max_string_bytes);
  if (from_cp == NULL || to_off == NULL || to_clen == NULL) {
    free(from_cp); free(to_off); free(to_clen);
    mkr_owned_text_clear(&s); mkr_owned_text_clear(&from); mkr_owned_text_clear(&to);
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in translate");
    return -1;
  }

  /* The string-literal lexer validates UTF-8 and DOM string-values are valid
   * UTF-8, so a decode error here should not happen - but if it does, fail
   * closed (RUNTIME error) rather than silently truncating the result. */
  size_t from_n = 0;
  for (const lxb_char_t *p = (const lxb_char_t *)from.ptr, *e = p + from.len; p < e;) {
    lxb_codepoint_t cp = lxb_encoding_decode_valid_utf_8_single(&p, e);
    if (cp == LXB_ENCODING_DECODE_ERROR) goto decode_fail;
    from_cp[from_n++] = cp;
  }
  size_t to_n = 0;
  for (const lxb_char_t *p = (const lxb_char_t *)to.ptr, *e = p + to.len; p < e;) {
    const lxb_char_t *c = p;
    if (lxb_encoding_decode_valid_utf_8_single(&p, e) == LXB_ENCODING_DECODE_ERROR) goto decode_fail;
    to_off[to_n]  = (size_t)(c - (const lxb_char_t *)to.ptr);
    to_clen[to_n] = (size_t)(p - c);
    to_n++;
  }

  for (const lxb_char_t *p = (const lxb_char_t *)s.ptr, *e = p + s.len; p < e;) {
    const lxb_char_t *c = p;
    lxb_codepoint_t cp = lxb_encoding_decode_valid_utf_8_single(&p, e);
    if (cp == LXB_ENCODING_DECODE_ERROR) goto decode_fail;
    size_t clen = (size_t)(p - c);

    const char *emit = NULL;
    size_t emit_len = 0;
    size_t k = from_n;
    for (size_t fi = 0; fi < from_n; ++fi) {
      if (from_cp[fi] == cp) { k = fi; break; }
    }
    if (k == from_n) {            /* not in `from`: keep the character */
      emit = (const char *)c; emit_len = clen;
    } else if (k < to_n) {        /* replace with the to[k] character */
      emit = to.ptr + to_off[k]; emit_len = to_clen[k];
    }                             /* else: k >= to_n -> drop the character */

    if (emit_len != 0) {
      mkr_status_t st = mkr_buf_append(&buf, emit, emit_len);
      if (st != MKR_OK) {
        mkr_buf_free(&buf);
        free(from_cp); free(to_off); free(to_clen);
        mkr_owned_text_clear(&s); mkr_owned_text_clear(&from); mkr_owned_text_clear(&to);
        if (st == MKR_ERR_LIMIT) {
          mkr_err_setf(err, MKR_XPATH_ERR_LIMIT,
                       "string size limit exceeded (%zu bytes) in translate()",
                       L->max_string_bytes);
        } else {
          mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in translate");
        }
        return -1;
      }
    }
  }

  free(from_cp); free(to_off); free(to_clen);
  mkr_owned_text_clear(&s); mkr_owned_text_clear(&from); mkr_owned_text_clear(&to);
  size_t out_len = 0;
  char *out_s = mkr_buf_steal(&buf, &out_len);
  mkr_val_set_owned_text(out, mkr_owned_text(out_s, out_len));
  if (out->u.string.ptr == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in translate");
    return -1;
  }
  return 0;

decode_fail:
  mkr_buf_free(&buf);
  free(from_cp); free(to_off); free(to_clen);
  mkr_owned_text_clear(&s); mkr_owned_text_clear(&from); mkr_owned_text_clear(&to);
  mkr_err_set(err, MKR_XPATH_ERR_RUNTIME, "invalid UTF-8 in translate() argument");
  return -1;
}

/* ---------- number functions ---------- */

static int
fn_number(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
          mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  if (arity_check(nargs, 0, 1, err, "number") != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  if (nargs == 0) {
    mkr_val_t self_v = { .type = MKR_XPATH_TYPE_NODESET };
    mkr_nodeset_init(&self_v.u.nodeset);
    if (focus->node && mkr_nodeset_push(&self_v.u.nodeset, focus->node, L, err) != 0) {
      mkr_nodeset_clear(&self_v.u.nodeset);
      return -1;
    }
    int rc = mkr_val_to_number_or_fail(&self_v, L, err, &out->u.number);
    mkr_nodeset_clear(&self_v.u.nodeset);
    return rc;
  }
  return mkr_val_to_number_or_fail(&args[0], L, err, &out->u.number);
}

static int
fn_sum(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
       mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 1, 1, err, "sum") != 0) return -1;
  const mkr_nodeset_t *ns = require_nodeset(&args[0], "sum", err);
  if (ns == NULL) return -1;
  /* mkr_get_cached_node_text consults ctx->limits internally for the
   * cache-miss path; no need to thread max_string_bytes here. */
  double total = 0.0;
  for (size_t i = 0; i < ns->count; ++i) {
    if (mkr_limit_eval_op(mkr_ctx_limits(ctx), err) != 0) return -1;
    mkr_borrowed_text_t s;
    if (mkr_get_cached_node_text(ctx, ns->items[i], &s, err) != 0) {
      return -1;
    }
    total += mkr_borrowed_text_to_number(s);
  }
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = total;
  return 0;
}

static int
fn_floor(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
         mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 1, 1, err, "floor") != 0) return -1;
  double d;
  if (mkr_val_to_number_or_fail(&args[0], mkr_ctx_limits(ctx), err, &d) != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = floor(d);
  return 0;
}

static int
fn_ceiling(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
           mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 1, 1, err, "ceiling") != 0) return -1;
  double d;
  if (mkr_val_to_number_or_fail(&args[0], mkr_ctx_limits(ctx), err, &d) != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = ceil(d);
  return 0;
}

/* XPath 1.0 round(): round to nearest integer, .5 rounds toward +inf. */
static int
fn_round(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
         mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)focus;
  if (arity_check(nargs, 1, 1, err, "round") != 0) return -1;
  double d;
  if (mkr_val_to_number_or_fail(&args[0], mkr_ctx_limits(ctx), err, &d) != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  if (d != d) out->u.number = d; /* NaN */
  else out->u.number = floor(d + 0.5);
  return 0;
}

/* ---------- node-set name functions ---------- */

/* Helper: pull the first node of a nodeset arg, or the self node when
 * the arg list is empty. */
static MKR_DOM_NODE *
name_func_target(mkr_val_t *args, size_t nargs, MKR_DOM_NODE *self_node, mkr_xpath_error_t *err, const char *fname)
{
  if (nargs == 0) return self_node;
  const mkr_nodeset_t *ns = require_nodeset(&args[0], fname, err);
  if (ns == NULL) return NULL;
  if (ns->count == 0) return NULL;
  return ns->items[0];
}

/* Set out to an empty string and report OOM if strdup fails. */
static int
set_empty_string(mkr_val_t *out, mkr_xpath_error_t *err, const char *fn_name)
{
  char *s = mkr_strdup("");
  if (s == NULL) {
    mkr_err_setf(err, MKR_XPATH_ERR_OOM, "out of memory in %s()", fn_name);
    return -1;
  }
  mkr_val_set_owned_text(out, mkr_owned_text(s, 0));
  return 0;
}

/* Copy a Lexbor-owned (possibly NULL/length-0) byte run into a freshly
 * malloc'd C string. NULL src is treated as empty. OOM raises through err. */
static int
set_bytes_string(mkr_val_t *out, const lxb_char_t *src, size_t len,
                 mkr_xpath_error_t *err, const char *fn_name)
{
  char *s = mkr_str_alloc(len);
  if (s == NULL) {
    mkr_err_setf(err, MKR_XPATH_ERR_OOM, "out of memory in %s()", fn_name);
    return -1;
  }
  if (src != NULL && len > 0) memcpy(s, src, len);
  s[len] = '\0';
  mkr_val_set_owned_text(out, mkr_owned_text(s, len));
  return 0;
}

static int
fn_local_name(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
              mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx;
  if (arity_check(nargs, 0, 1, err, "local-name") != 0) return -1;
  /* Every return path below sets out via set_empty_string / set_bytes_string;
   * an error return leaves the caller's zero-initialised (clearable) out. */
  MKR_DOM_NODE *n = name_func_target(args, nargs, focus->node, err, "local-name");
  if (err->status != MKR_XPATH_OK) return -1;
  if (n == NULL ||
      (MKR_NODE_TYPE(n) != MKR_NTYPE_ELEMENT &&
       MKR_NODE_TYPE(n) != MKR_NTYPE_ATTRIBUTE &&
       MKR_NODE_TYPE(n) != MKR_NTYPE_PI)) {
    return set_empty_string(out, err, "local-name");
  }
  size_t len = 0;
  const lxb_char_t *name;
  if (MKR_NODE_TYPE(n) == MKR_NTYPE_ATTRIBUTE) {
    name = MKR_ATTR_LOCAL_NAME(n, &len);
  } else if (MKR_NODE_TYPE(n) == MKR_NTYPE_ELEMENT) {
    name = MKR_ELEM_LOCAL_NAME(n, &len);
  } else {
    name = MKR_NODE_PI_NAME(n, &len);
  }
  return set_bytes_string(out, name, len, err, "local-name");
}

static int
fn_namespace_uri(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
                 mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  if (arity_check(nargs, 0, 1, err, "namespace-uri") != 0) return -1;
  MKR_DOM_NODE *n = name_func_target(args, nargs, focus->node, err, "namespace-uri");
  if (err->status != MKR_XPATH_OK) return -1;
  if (n == NULL ||
      (MKR_NODE_TYPE(n) != MKR_NTYPE_ELEMENT && MKR_NODE_TYPE(n) != MKR_NTYPE_ATTRIBUTE)) {
    return set_empty_string(out, err, "namespace-uri");
  }
  if (MKR_NODE_NS_ID(n) == 0) {
    return set_empty_string(out, err, "namespace-uri");
  }
  size_t len = 0;
  const char *uri = MKR_NODE_NS_URI(n, mkr_ctx_document(ctx), &len);
  if (uri == NULL) {
    return set_empty_string(out, err, "namespace-uri");
  }
  return set_bytes_string(out, (const lxb_char_t *)uri, len, err, "namespace-uri");
}

static int
fn_name(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
        mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx;
  if (arity_check(nargs, 0, 1, err, "name") != 0) return -1;
  MKR_DOM_NODE *n = name_func_target(args, nargs, focus->node, err, "name");
  if (err->status != MKR_XPATH_OK) return -1;
  if (n == NULL ||
      (MKR_NODE_TYPE(n) != MKR_NTYPE_ELEMENT &&
       MKR_NODE_TYPE(n) != MKR_NTYPE_ATTRIBUTE &&
       MKR_NODE_TYPE(n) != MKR_NTYPE_PI)) {
    return set_empty_string(out, err, "name");
  }
  /* In HTML the qualified name matches the local name; this also avoids
   * surfacing the LXB_NS_HTML prefix that would otherwise confuse users. */
  size_t len = 0;
  const lxb_char_t *name;
  if (MKR_NODE_TYPE(n) == MKR_NTYPE_ATTRIBUTE) {
    name = MKR_ATTR_QUALIFIED_NAME(n, &len);
  } else if (MKR_NODE_TYPE(n) == MKR_NTYPE_ELEMENT) {
    name = MKR_ELEM_QUALIFIED_NAME(n, &len);
  } else {
    name = MKR_NODE_PI_NAME(n, &len);   /* a PI's expanded-name is (null, target) */
  }
  return set_bytes_string(out, name, len, err, "name");
}

/* ---------- boolean / language ---------- */

static int
fn_lang(mkr_xpath_context_t *ctx, const mkr_focus_t *focus,
        mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx;
  if (arity_check(nargs, 1, 1, err, "lang") != 0) return -1;
  mkr_owned_text_t want;
  if (mkr_val_to_owned_text_or_fail(&args[0], mkr_ctx_limits(ctx), err, &want) != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = 0;
  /* Walk up the ancestor chain looking for the host's language attribute.
     Host policy (§8.6): XPath 1.0 lang() is xml:lang based; HTML uses the `lang`
     attribute (with xml:lang accepted as a fallback). */
  for (MKR_DOM_NODE *p = focus->node; p != NULL; p = MKR_NODE_PARENT(p)) {
    if (MKR_NODE_TYPE(p) != MKR_NTYPE_ELEMENT) continue;
    MKR_DOM_ELEMENT *el = MKR_NODE_AS_ELEMENT(p);
    size_t vlen = 0;
#ifdef MKR_HOST_XML
    const lxb_char_t *v = MKR_ELEM_GET_ATTRIBUTE(el, "xml:lang", 8, &vlen);
#else
    const lxb_char_t *v = MKR_ELEM_GET_ATTRIBUTE(el, "lang", 4, &vlen);
    if (v == NULL) {
      v = MKR_ELEM_GET_ATTRIBUTE(el, "xml:lang", 8, &vlen);
    }
#endif /* MKR_HOST_XML */
    if (v == NULL) continue;
    /* Compare prefix (case-insensitive) up to '-'. */
    if (vlen >= want.len) {
      int match = 1;
      for (size_t i = 0; i < want.len; ++i) {
        char a = (char)v[i]; char b = want.ptr[i];
        if (a >= 'A' && a <= 'Z') a = (char)(a + 32);
        if (b >= 'A' && b <= 'Z') b = (char)(b + 32);
        if (a != b) { match = 0; break; }
      }
      if (match && (vlen == want.len || v[want.len] == '-')) {
        out->u.boolean = 1;
        break;
      }
    }
  }
  mkr_owned_text_clear(&want);
  return 0;
}

static const fn_entry_t fn_table[] = {
  /* node-set */
  { "last",             fn_last             },
  { "position",         fn_position         },
  { "count",            fn_count            },
  { "id",               fn_id               },
  { "local-name",       fn_local_name       },
  { "namespace-uri",    fn_namespace_uri    },
  { "name",             fn_name             },
  /* string */
  { "string",           fn_string           },
  { "concat",           fn_concat           },
  { "starts-with",      fn_starts_with      },
  { "contains",         fn_contains         },
  { "substring-before", fn_substring_before },
  { "substring-after",  fn_substring_after  },
  { "substring",        fn_substring        },
  { "string-length",    fn_string_length    },
  { "normalize-space",  fn_normalize_space  },
  { "translate",        fn_translate        },
  /* boolean */
  { "not",              fn_not              },
  { "true",             fn_true             },
  { "false",            fn_false            },
  { "boolean",          fn_boolean          },
  { "lang",             fn_lang             },
  /* number */
  { "number",           fn_number           },
  { "sum",              fn_sum              },
  { "floor",            fn_floor            },
  { "ceiling",          fn_ceiling          },
  { "round",            fn_round            },
#ifdef MKR_HOST_XML
  /* CSS-only internal hooks for untyped :*-of-type; the \x01-prefixed names are
   * unreachable from a user XPath expression (the lexer cannot produce them). */
  { MKR_FN_OF_TYPE_POS,      fn_xml_of_type_pos      },
  { MKR_FN_OF_TYPE_POS_LAST, fn_xml_of_type_pos_last },
#endif
  { NULL,               NULL                },
};

/* File-static: the function table is engine-internal. eval_fncall (later in the
 * merged engine TU) resolves through it; nothing outside the engine does. */
static mkr_func_impl_t
mkr_lookup_function(const char *ns_uri, const char *local_name)
{
  if (local_name == NULL) return NULL;
  if (ns_uri != NULL) {
    /* Nokogiri-compatible builtins live in MKR_NS_NOKOGIRI_BUILTIN_URI.
     * Other registered namespaces resolve to user-defined functions -
     * not yet implemented (Phase 3 TODO: custom function registry). */
    if (strcmp(ns_uri, MKR_NS_NOKOGIRI_BUILTIN_URI) == 0) {
      for (size_t i = 0; fn_table_nokogiri_builtin[i].name; ++i) {
        if (strcmp(fn_table_nokogiri_builtin[i].name, local_name) == 0) {
          return fn_table_nokogiri_builtin[i].fn;
        }
      }
    }
    return NULL;
  }
  /* Default namespace - XPath 1.0 standard library. */
  for (size_t i = 0; fn_table[i].name; ++i) {
    if (strcmp(fn_table[i].name, local_name) == 0) return fn_table[i].fn;
  }
  return NULL;
}
