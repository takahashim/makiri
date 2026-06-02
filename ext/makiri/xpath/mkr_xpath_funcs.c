#include "mkr_xpath_internal.h"
#include "../core/mkr_safe.h"

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

/* Count Unicode code points in a UTF-8 string. XPath 1.0 string-length
 * and substring offsets are measured in characters, not bytes — Lexbor
 * exposes content as UTF-8 byte strings, so we walk and count leading
 * bytes (any byte that isn't a 10xxxxxx continuation). */
static size_t
utf8_char_count(const char *s)
{
  if (s == NULL) return 0;
  size_t n = 0;
  for (const unsigned char *p = (const unsigned char *)s; *p; ++p) {
    if ((*p & 0xC0) != 0x80) ++n;
  }
  return n;
}

/* Step forward by n UTF-8 characters from s; returns byte pointer.
 * Returns the trailing NUL if n exceeds the string length. */
static const char *
utf8_advance(const char *s, size_t n)
{
  const unsigned char *p = (const unsigned char *)s;
  while (n > 0 && *p) {
    do { ++p; } while ((*p & 0xC0) == 0x80);
    --n;
  }
  return (const char *)p;
}

static const char *
mkr_text_find(const char *hay, size_t hay_len, const char *needle, size_t needle_len)
{
  if (hay == NULL || needle == NULL) return NULL;
  if (needle_len == 0) return hay;
  if (needle_len > hay_len) return NULL;
  size_t last = hay_len - needle_len;
  for (size_t i = 0; i <= last; ++i) {
    if (hay[i] == needle[0] && memcmp(hay + i, needle, needle_len) == 0) {
      return hay + i;
    }
  }
  return NULL;
}

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

static int
fn_last(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
        size_t self_pos, size_t self_size,
        mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)self_node; (void)self_pos; (void)args;
  if (arity_check(nargs, 0, 0, err, "last") != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = (double)self_size;
  return 0;
}

static int
fn_position(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
            size_t self_pos, size_t self_size,
            mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)self_node; (void)self_size; (void)args;
  if (arity_check(nargs, 0, 0, err, "position") != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = (double)self_pos;
  return 0;
}

static int
fn_count(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
         size_t self_pos, size_t self_size,
         mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 1, 1, err, "count") != 0) return -1;
  if (args[0].type != MKR_XPATH_TYPE_NODESET) {
    mkr_err_set(err, MKR_XPATH_ERR_TYPE, "count(): argument must be a node-set");
    return -1;
  }
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = (double)args[0].u.nodeset.count;
  return 0;
}

static int
fn_not(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
       size_t self_pos, size_t self_size,
       mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 1, 1, err, "not") != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = !mkr_val_to_boolean(&args[0]);
  return 0;
}

static int
fn_true(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
        size_t self_pos, size_t self_size,
        mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)self_node; (void)self_pos; (void)self_size; (void)args;
  if (arity_check(nargs, 0, 0, err, "true") != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = 1;
  return 0;
}

static int
fn_false(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
         size_t self_pos, size_t self_size,
         mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)self_node; (void)self_pos; (void)self_size; (void)args;
  if (arity_check(nargs, 0, 0, err, "false") != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = 0;
  return 0;
}

/* id(string|nodeset) — looks up by HTML id attribute. */
static lxb_dom_node_t *
find_by_id(lxb_dom_node_t *root, const char *id, size_t id_len)
{
  if (root == NULL || id == NULL || id_len == 0) return NULL;
  lxb_dom_node_t *n = root;
  while (n) {
    if (n->type == LXB_DOM_NODE_TYPE_ELEMENT) {
      lxb_dom_element_t *el = (lxb_dom_element_t *)n;
      size_t vlen = 0;
      const lxb_char_t *v = lxb_dom_element_get_attribute(el, (const lxb_char_t *)"id", 2, &vlen);
      if (v && vlen == id_len && memcmp(v, id, id_len) == 0) {
        return n;
      }
    }
    if (n->first_child) {
      n = n->first_child;
    } else {
      while (n && n != root && n->next == NULL) n = n->parent;
      if (n == root || n == NULL) break;
      n = n->next;
    }
  }
  return NULL;
}

/*
 * Look up each whitespace-separated token in 's' (mutated) and push
 * every hit into 'out'. Duplicates are pushed unconditionally — the
 * caller (fn_id) deduplicates the entire result via sort + adjacent
 * pass after all tokens have been processed, which is O(n log n)
 * versus O(n^2) per-insert contains() checks.
 */
static int
id_collect_from_string(char *s, lxb_dom_node_t *root, mkr_nodeset_t *out,
                       mkr_xpath_context_t *ctx, mkr_xpath_error_t *err)
{
  if (s == NULL) return 0;
  char *p = s;
  while (*p) {
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    if (*p == '\0') break;
    char *tok = p;
    while (*p && *p != ' ' && *p != '\t' && *p != '\n' && *p != '\r') p++;
    size_t tok_len = (size_t)(p - tok);
    char saved = *p;
    *p = '\0';
    lxb_dom_node_t *hit = find_by_id(root, tok, tok_len);
    if (hit) {
      if (mkr_nodeset_push(out, hit, mkr_ctx_limits(ctx), err) != 0) {
        return -1;
      }
    }
    if (saved == '\0') break;
    *p++ = saved;
  }
  return 0;
}

static int
fn_id(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
      size_t self_pos, size_t self_size,
      mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 1, 1, err, "id") != 0) return -1;
  out->type = MKR_XPATH_TYPE_NODESET;
  mkr_nodeset_init(&out->u.nodeset);

  lxb_dom_node_t *root = (lxb_dom_node_t *)mkr_ctx_document(ctx);
  if (root == NULL) return 0;

  /* XPath 1.0 §4.1: id() argument may be a node-set; in that case each
   * node's string-value is treated as IDREFS — whitespace-separated tokens.
   * For non-node-set, the value is converted to string and split likewise. */
  if (args[0].type == MKR_XPATH_TYPE_NODESET) {
    for (size_t i = 0; i < args[0].u.nodeset.count; ++i) {
      mkr_owned_text_t text;
      if (mkr_node_string_text_or_fail(args[0].u.nodeset.items[i], mkr_ctx_limits(ctx), err, &text) != 0) {
        mkr_nodeset_clear(&out->u.nodeset);
        return -1;
      }
      int rc = id_collect_from_string(text.ptr, root, &out->u.nodeset, ctx, err);
      mkr_owned_text_clear(&text);
      if (rc != 0) { mkr_nodeset_clear(&out->u.nodeset); return -1; }
    }
  } else {
    mkr_owned_text_t text;
    if (mkr_val_to_owned_text_or_fail(&args[0], mkr_ctx_limits(ctx), err, &text) != 0) {
      mkr_nodeset_clear(&out->u.nodeset);
      return -1;
    }
    int rc = id_collect_from_string(text.ptr, root, &out->u.nodeset, ctx, err);
    mkr_owned_text_clear(&text);
    if (rc != 0) { mkr_nodeset_clear(&out->u.nodeset); return -1; }
  }
  /* Result is in document order with duplicates removed (XPath 1.0 §4.1). */
  mkr_nodeset_unique_sorted(ctx, &out->u.nodeset);
  return 0;
}

/* Forward decl for two_owned_texts() defined later. */
static int two_owned_texts(mkr_xpath_context_t *ctx, mkr_val_t *args,
                           mkr_owned_text_t *sa, mkr_owned_text_t *sb,
                           mkr_xpath_error_t *err);

/* ---------- nokogiri-builtin namespace ---------- */

/*
 * css-class(haystack, needle) — true iff 'needle' is a whitespace-
 * separated token of 'haystack'. The libxml2 version of this function
 * ships in nl_xpath_context.c (builtin_css_class); we keep behavior
 * identical.
 */
static int
ws_token_match(const char *str, const char *val, size_t val_len)
{
  if (str == NULL || val == NULL) return 0;
  if (val_len == 0) return 1; /* libxml2 returns non-NULL for empty val */

  const char *p = str;
  while (*p) {
    const char *tok = p;
    while (*p && !(*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r')) p++;
    size_t tok_len = (size_t)(p - tok);
    if (tok_len == val_len && tok_len > 0 && tok[0] == val[0]
        && memcmp(tok, val, val_len) == 0) {
      return 1;
    }
    /* skip whitespace */
    while (*p && (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r')) p++;
  }
  return 0;
}

static int
fn_nokogiri_css_class(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
                     size_t self_pos, size_t self_size,
                     mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 2, 2, err, "nokogiri-builtin:css-class") != 0) return -1;
  mkr_owned_text_t hay, needle;
  if (two_owned_texts(ctx, args, &hay, &needle, err) != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = ws_token_match(hay.ptr, needle.ptr, needle.len);
  mkr_owned_text_clear(&hay);
  mkr_owned_text_clear(&needle);
  return 0;
}

/*
 * local-name-is(elementName) — true iff the context node's qualified
 * name (for HTML this is the lowercase local name) equals the argument.
 */
static int
fn_nokogiri_local_name_is(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
                          size_t self_pos, size_t self_size,
                          mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_pos; (void)self_size;
  if (arity_check(nargs, 1, 1, err, "nokogiri-builtin:local-name-is") != 0) return -1;
  mkr_owned_text_t want;
  if (mkr_val_to_owned_text_or_fail(&args[0], mkr_ctx_limits(ctx), err, &want) != 0) return -1;

  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = 0;
  if (self_node != NULL) {
    size_t got_len = 0;
    const lxb_char_t *got = mkr_dom_node_name_qualified(self_node, &got_len);
    if (got != NULL) {
      out->u.boolean = (got_len == want.len && memcmp(got, want.ptr, want.len) == 0);
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

static int
fn_boolean(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
           size_t self_pos, size_t self_size,
           mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 1, 1, err, "boolean") != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = mkr_val_to_boolean(&args[0]);
  return 0;
}

/* ---------- string functions ---------- */

static int
fn_string(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
          size_t self_pos, size_t self_size,
          mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_pos; (void)self_size;
  if (arity_check(nargs, 0, 1, err, "string") != 0) return -1;
  out->type = MKR_XPATH_TYPE_STRING;
  if (nargs == 0) {
    size_t len = 0;
    char *s = mkr_node_string_value_or_fail(self_node, mkr_ctx_limits(ctx), err, &len);
    mkr_val_set_owned_string(out, s, len);
  } else {
    mkr_owned_text_t text;
    if (mkr_val_to_owned_text_or_fail(&args[0], mkr_ctx_limits(ctx), err, &text) != 0) return -1;
    mkr_val_set_owned_string(out, text.ptr, text.len);
  }
  return out->u.string == NULL ? -1 : 0;
}

static int
fn_concat(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
          size_t self_pos, size_t self_size,
          mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
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
  mkr_val_set_owned_string(out, buf, total);
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
fn_starts_with(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
               size_t self_pos, size_t self_size,
               mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 2, 2, err, "starts-with") != 0) return -1;
  mkr_owned_text_t s, t;
  if (two_owned_texts(ctx, args, &s, &t, err) != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = (s.len >= t.len && memcmp(s.ptr, t.ptr, t.len) == 0);
  mkr_owned_text_clear(&s);
  mkr_owned_text_clear(&t);
  return 0;
}

static int
fn_contains(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
            size_t self_pos, size_t self_size,
            mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 2, 2, err, "contains") != 0) return -1;
  mkr_owned_text_t s, t;
  if (two_owned_texts(ctx, args, &s, &t, err) != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = (mkr_text_find(s.ptr, s.len, t.ptr, t.len) != NULL);
  mkr_owned_text_clear(&s);
  mkr_owned_text_clear(&t);
  return 0;
}

static int
fn_substring_before(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
                    size_t self_pos, size_t self_size,
                    mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 2, 2, err, "substring-before") != 0) return -1;
  mkr_owned_text_t s, t;
  if (two_owned_texts(ctx, args, &s, &t, err) != 0) return -1;
  const char *found = (t.len == 0) ? NULL : mkr_text_find(s.ptr, s.len, t.ptr, t.len);
  size_t n = (found != NULL) ? (size_t)(found - s.ptr) : 0;
  char *p = mkr_strndup(s.ptr, n); /* n == 0 yields an owned "" */
  mkr_owned_text_clear(&s);
  mkr_owned_text_clear(&t);
  if (p == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in substring-before");
    return -1;
  }
  mkr_val_set_owned_string(out, p, n);
  return 0;
}

static int
fn_substring_after(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
                   size_t self_pos, size_t self_size,
                   mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 2, 2, err, "substring-after") != 0) return -1;
  mkr_owned_text_t s, t;
  if (two_owned_texts(ctx, args, &s, &t, err) != 0) return -1;
  size_t out_len;
  char *p;
  if (t.len == 0) {
    out_len = s.len;
    p = mkr_strndup(s.ptr, s.len);
  } else {
    const char *found = mkr_text_find(s.ptr, s.len, t.ptr, t.len);
    out_len = found ? s.len - (size_t)((found + t.len) - s.ptr) : 0;
    p = found ? mkr_strndup(found + t.len, out_len) : mkr_strdup("");
  }
  mkr_owned_text_clear(&s);
  mkr_owned_text_clear(&t);
  if (p == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in substring-after");
    return -1;
  }
  mkr_val_set_owned_string(out, p, out_len);
  return 0;
}

/* substring(s, start[, length]) — XPath positions are 1-based and round
 * to nearest; out-of-range positions clip to the string. */
static int
fn_substring(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
             size_t self_pos, size_t self_size,
             mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 2, 3, err, "substring") != 0) return -1;
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  mkr_owned_text_t s;
  if (mkr_val_to_owned_text_or_fail(&args[0], L, err, &s) != 0) return -1;
  double start_d, end_d;
  if (mkr_val_to_number_or_fail(&args[1], L, err, &start_d) != 0) { mkr_owned_text_clear(&s); return -1; }
  size_t s_chars = utf8_char_count(s.ptr);
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
      const char *byte_start = utf8_advance(s.ptr, (size_t)(start - 1));
      const char *byte_end   = utf8_advance(byte_start, (size_t)(end - start));
      out_len = (size_t)(byte_end - byte_start);
      p = mkr_strndup(byte_start, out_len);
    }
  }
  mkr_owned_text_clear(&s);
  if (p == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in substring");
    return -1;
  }
  mkr_val_set_owned_string(out, p, out_len);
  return 0;
}

static int
fn_string_length(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
                 size_t self_pos, size_t self_size,
                 mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_pos; (void)self_size;
  if (arity_check(nargs, 0, 1, err, "string-length") != 0) return -1;
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  mkr_owned_text_t s;
  int rc = (nargs == 0)
              ? mkr_node_string_text_or_fail(self_node, L, err, &s)
              : mkr_val_to_owned_text_or_fail(&args[0], L, err, &s);
  if (rc != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = (double)utf8_char_count(s.ptr);
  mkr_owned_text_clear(&s);
  return 0;
}

/* normalize-space: collapse runs of whitespace, trim leading/trailing */
static int
fn_normalize_space(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
                   size_t self_pos, size_t self_size,
                   mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_pos; (void)self_size;
  if (arity_check(nargs, 0, 1, err, "normalize-space") != 0) return -1;
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  mkr_owned_text_t s;
  int rc = (nargs == 0)
              ? mkr_node_string_text_or_fail(self_node, L, err, &s)
              : mkr_val_to_owned_text_or_fail(&args[0], L, err, &s);
  if (rc != 0) return -1;
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
  mkr_val_set_owned_string(out, buf, out_i);
  return 0;
}

static int
fn_translate(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
             size_t self_pos, size_t self_size,
             mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
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
   * UTF-8, so a decode error here should not happen — but if it does, fail
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
  mkr_val_set_owned_string(out, out_s, out_len);
  if (out->u.string == NULL) {
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
fn_number(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
          size_t self_pos, size_t self_size,
          mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_pos; (void)self_size;
  if (arity_check(nargs, 0, 1, err, "number") != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  if (nargs == 0) {
    mkr_val_t self_v = { .type = MKR_XPATH_TYPE_NODESET };
    mkr_nodeset_init(&self_v.u.nodeset);
    if (self_node && mkr_nodeset_push(&self_v.u.nodeset, self_node, L, err) != 0) {
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
fn_sum(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
       size_t self_pos, size_t self_size,
       mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 1, 1, err, "sum") != 0) return -1;
  if (args[0].type != MKR_XPATH_TYPE_NODESET) {
    mkr_err_set(err, MKR_XPATH_ERR_TYPE, "sum(): argument must be a node-set");
    return -1;
  }
  /* mkr_get_cached_node_text consults ctx->limits internally for the
   * cache-miss path; no need to thread max_string_bytes here. */
  double total = 0.0;
  for (size_t i = 0; i < args[0].u.nodeset.count; ++i) {
    mkr_borrowed_text_t s;
    if (mkr_get_cached_node_text(ctx, args[0].u.nodeset.items[i], &s, err) != 0) {
      return -1;
    }
    total += mkr_borrowed_text_to_number(s);
  }
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = total;
  return 0;
}

static int
fn_floor(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
         size_t self_pos, size_t self_size,
         mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 1, 1, err, "floor") != 0) return -1;
  double d;
  if (mkr_val_to_number_or_fail(&args[0], mkr_ctx_limits(ctx), err, &d) != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = floor(d);
  return 0;
}

static int
fn_ceiling(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
           size_t self_pos, size_t self_size,
           mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 1, 1, err, "ceiling") != 0) return -1;
  double d;
  if (mkr_val_to_number_or_fail(&args[0], mkr_ctx_limits(ctx), err, &d) != 0) return -1;
  out->type = MKR_XPATH_TYPE_NUMBER;
  out->u.number = ceil(d);
  return 0;
}

/* XPath 1.0 round(): round to nearest integer, .5 rounds toward +inf. */
static int
fn_round(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
         size_t self_pos, size_t self_size,
         mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_node; (void)self_pos; (void)self_size;
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
static lxb_dom_node_t *
name_func_target(mkr_val_t *args, size_t nargs, lxb_dom_node_t *self_node, mkr_xpath_error_t *err, const char *fname)
{
  if (nargs == 0) return self_node;
  if (args[0].type != MKR_XPATH_TYPE_NODESET) {
    mkr_err_setf(err, MKR_XPATH_ERR_TYPE, "%s(): argument must be a node-set", fname);
    return NULL;
  }
  if (args[0].u.nodeset.count == 0) return NULL;
  return args[0].u.nodeset.items[0];
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
  mkr_val_set_owned_string(out, s, 0);
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
  mkr_val_set_owned_string(out, s, len);
  return 0;
}

static int
fn_local_name(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
              size_t self_pos, size_t self_size,
              mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 0, 1, err, "local-name") != 0) return -1;
  out->type = MKR_XPATH_TYPE_STRING;
  out->u.string = NULL;
  lxb_dom_node_t *n = name_func_target(args, nargs, self_node, err, "local-name");
  if (err->status != MKR_XPATH_OK) return -1;
  if (n == NULL ||
      (n->type != LXB_DOM_NODE_TYPE_ELEMENT &&
       n->type != LXB_DOM_NODE_TYPE_ATTRIBUTE &&
       n->type != LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION)) {
    return set_empty_string(out, err, "local-name");
  }
  size_t len = 0;
  const lxb_char_t *name;
  if (n->type == LXB_DOM_NODE_TYPE_ATTRIBUTE) {
    name = lxb_dom_attr_local_name((lxb_dom_attr_t *)n, &len);
  } else if (n->type == LXB_DOM_NODE_TYPE_ELEMENT) {
    name = lxb_dom_element_local_name((lxb_dom_element_t *)n, &len);
  } else {
    name = lxb_dom_node_name(n, &len);
  }
  return set_bytes_string(out, name, len, err, "local-name");
}

static int
fn_namespace_uri(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
                 size_t self_pos, size_t self_size,
                 mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)self_pos; (void)self_size;
  if (arity_check(nargs, 0, 1, err, "namespace-uri") != 0) return -1;
  out->type = MKR_XPATH_TYPE_STRING;
  out->u.string = NULL;
  lxb_dom_node_t *n = name_func_target(args, nargs, self_node, err, "namespace-uri");
  if (err->status != MKR_XPATH_OK) return -1;
  if (n == NULL ||
      (n->type != LXB_DOM_NODE_TYPE_ELEMENT && n->type != LXB_DOM_NODE_TYPE_ATTRIBUTE)) {
    return set_empty_string(out, err, "namespace-uri");
  }
  if (n->ns == 0) {
    return set_empty_string(out, err, "namespace-uri");
  }
  lxb_dom_document_t *doc = mkr_ctx_document(ctx);
  if (doc == NULL || doc->ns == NULL) {
    return set_empty_string(out, err, "namespace-uri");
  }
  size_t len = 0;
  const lxb_char_t *uri = lxb_ns_by_id(doc->ns, n->ns, &len);
  return set_bytes_string(out, uri, len, err, "namespace-uri");
}

static int
fn_name(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
        size_t self_pos, size_t self_size,
        mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 0, 1, err, "name") != 0) return -1;
  out->type = MKR_XPATH_TYPE_STRING;
  out->u.string = NULL;
  lxb_dom_node_t *n = name_func_target(args, nargs, self_node, err, "name");
  if (err->status != MKR_XPATH_OK) return -1;
  if (n == NULL ||
      (n->type != LXB_DOM_NODE_TYPE_ELEMENT &&
       n->type != LXB_DOM_NODE_TYPE_ATTRIBUTE &&
       n->type != LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION)) {
    return set_empty_string(out, err, "name");
  }
  /* In HTML the qualified name matches the local name; this also avoids
   * surfacing the LXB_NS_HTML prefix that would otherwise confuse users. */
  size_t len = 0;
  const lxb_char_t *name;
  if (n->type == LXB_DOM_NODE_TYPE_ATTRIBUTE) {
    name = lxb_dom_attr_qualified_name((lxb_dom_attr_t *)n, &len);
  } else {
    name = mkr_dom_node_name_qualified(n, &len);
  }
  return set_bytes_string(out, name, len, err, "name");
}

/* ---------- boolean / language ---------- */

static int
fn_lang(mkr_xpath_context_t *ctx, lxb_dom_node_t *self_node,
        size_t self_pos, size_t self_size,
        mkr_val_t *args, size_t nargs, mkr_val_t *out, mkr_xpath_error_t *err)
{
  (void)ctx; (void)self_pos; (void)self_size;
  if (arity_check(nargs, 1, 1, err, "lang") != 0) return -1;
  mkr_owned_text_t want;
  if (mkr_val_to_owned_text_or_fail(&args[0], mkr_ctx_limits(ctx), err, &want) != 0) return -1;
  out->type = MKR_XPATH_TYPE_BOOLEAN;
  out->u.boolean = 0;
  /* Walk up the ancestor chain looking for an @xml:lang or @lang. */
  for (lxb_dom_node_t *p = self_node; p != NULL; p = p->parent) {
    if (p->type != LXB_DOM_NODE_TYPE_ELEMENT) continue;
    lxb_dom_element_t *el = (lxb_dom_element_t *)p;
    size_t vlen = 0;
    const lxb_char_t *v = lxb_dom_element_get_attribute(el, (const lxb_char_t *)"lang", 4, &vlen);
    if (v == NULL) {
      v = lxb_dom_element_get_attribute(el, (const lxb_char_t *)"xml:lang", 8, &vlen);
    }
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
  { NULL,               NULL                },
};

mkr_func_impl_t
mkr_lookup_function(const char *ns_uri, const char *local_name)
{
  if (local_name == NULL) return NULL;
  if (ns_uri != NULL) {
    /* Nokogiri-compatible builtins live in MKR_NS_NOKOGIRI_BUILTIN_URI.
     * Other registered namespaces resolve to user-defined functions —
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
  /* Default namespace — XPath 1.0 standard library. */
  for (size_t i = 0; fn_table[i].name; ++i) {
    if (strcmp(fn_table[i].name, local_name) == 0) return fn_table[i].fn;
  }
  return NULL;
}
