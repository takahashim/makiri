#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"

#include <lexbor/dom/dom.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/*
 * Per-instance value model: the node-DEREFERENCING half of the runtime values
 * - node string-value construction (XPath 1.0 §5), the value coercions that
 * read a node-set's first node, document-order comparison/sort, and the
 * string-value cache's node-keyed insert. Compiled once per representation
 * (HTML / XML) with MKR_NODE_* bound by the including prelude.
 *
 * Every function here is file-static: it is reachable only from the other
 * per-instance bodies (funcs / eval) in the same merged engine translation
 * unit. The representation-INDEPENDENT primitives it leans on (node-set build,
 * owned/borrowed text, str-cache + doc-order lifecycle, AST destructors) are the
 * shared, bare-named functions in mkr_xpath_shared_body.h, declared in
 * mkr_xpath_internal.h.
 */

/* Forward declarations for the two coercions used before their definition
 * (mkr_val_to_number_or_fail reads both). They are static, so there is no
 * declaration in the shared internal header to cover the forward reference. */
static double mkr_borrowed_text_to_number(mkr_borrowed_text_t t);
static double mkr_val_to_number_unchecked(const mkr_val_t *v);

/* ---------- owned-text from a steal-able buffer ---------- */

static int
mkr_owned_text_from_buf_steal(mkr_owned_text_t *out, mkr_buf_t *buf,
                              mkr_xpath_error_t *err, const char *what)
{
  if (out == NULL || buf == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "mkr_owned_text_from_buf_steal: bad args");
    return -1;
  }
  mkr_owned_text_init(out);
  size_t len = 0;
  char *p = mkr_buf_steal(buf, &len);
  if (p == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, what ? what : "out of memory stealing text buffer");
    return -1;
  }
  out->ptr = p;
  out->len = len;
  return 0;
}

/* ---------- value clone ---------- */

static int
mkr_val_clone(const mkr_val_t *src, mkr_val_t *dst, mkr_xpath_error_t *err)
{
  if (src == NULL || dst == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "mkr_val_clone: bad args");
    return -1;
  }
  memset(dst, 0, sizeof(*dst));
  dst->type = src->type;
  switch (src->type) {
  case MKR_XPATH_TYPE_STRING: {
    mkr_owned_text_t text;
    if (mkr_owned_text_from_borrowed_copy(&text, mkr_borrowed_text_from_owned(src->u.string),
                                          err, "out of memory cloning string value") != 0) return -1;
    mkr_val_set_owned_text(dst, text);
    return 0;
  }
  case MKR_XPATH_TYPE_NUMBER:
    dst->u.number = src->u.number;
    return 0;
  case MKR_XPATH_TYPE_BOOLEAN:
    dst->u.boolean = src->u.boolean;
    return 0;
  case MKR_XPATH_TYPE_NODESET: {
    size_t n = src->u.nodeset.count;
    mkr_nodeset_init(&dst->u.nodeset);
    if (n == 0) return 0;
    void **items;
    size_t items_bytes;
    if (!mkr_size_mul(n, sizeof(*items), &items_bytes)) {
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory cloning node-set");
      return -1;
    }
    items = mkr_reallocarray(NULL, n, sizeof(*items));
    if (items == NULL) {
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory cloning node-set");
      return -1;
    }
    memcpy(items, src->u.nodeset.items, items_bytes);
    dst->u.nodeset.items    = items;
    dst->u.nodeset.count    = n;
    dst->u.nodeset.capacity = n;
    return 0;
  }
  }
  mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "mkr_val_clone: unknown value type");
  return -1;
}

/* ---------- node string-value (XPath 1.0 §5) ----------
 *
 * Built into an mkr_buf_t whose `max` is the per-evaluate byte cap: append fails
 * closed with MKR_ERR_LIMIT past the cap and MKR_ERR_OOM on allocation failure,
 * so there is never a partial/truncated result. Lexbor-allocated text is freed
 * after each append (otherwise we'd leak document-arena memory on every XPath
 * that touches text content). */

/* Append `node`'s own text content. */
static mkr_status_t
append_text_content(MKR_DOM_NODE *node, mkr_buf_t *buf)
{
  mkr_status_t st;
  MKR_NODE_APPEND_OWN_TEXT(node, buf, st);
  return st;
}

/* Append the string-value of every character-data descendant of `node`, in
 * document order. Both TEXT and CDATA-section nodes are character data (XPath
 * 1.0 §3 / §5: a CDATA section is text, not a distinct node type), so both
 * contribute - matching the text index that backs Node#text. Iterative
 * (parent-pointer) pre-order walk rather than C recursion, so an adversarially
 * deep tree cannot overflow the stack (fail-closed / no DoS); O(1) extra space.
 * Descends only into elements. */
static mkr_status_t
append_text_descendants(MKR_DOM_NODE *node, mkr_buf_t *buf)
{
  MKR_DOM_NODE *cur = MKR_NODE_FIRST_CHILD(node);
  while (cur != NULL) {
    if (MKR_NODE_TYPE(cur) == MKR_NTYPE_TEXT
        || MKR_NODE_TYPE(cur) == MKR_NTYPE_CDATA_SECTION) {
      mkr_status_t st = append_text_content(cur, buf);
      if (st != MKR_OK) return st; /* LIMIT or OOM - caller fails closed */
    }
    if (MKR_NODE_TYPE(cur) == MKR_NTYPE_ELEMENT && MKR_NODE_FIRST_CHILD(cur) != NULL) {
      cur = MKR_NODE_FIRST_CHILD(cur);
      continue;
    }
    while (cur != node && MKR_NODE_NEXT(cur) == NULL) {
      cur = MKR_NODE_PARENT(cur);
    }
    if (cur == node) return MKR_OK;
    cur = MKR_NODE_NEXT(cur);
  }
  return MKR_OK;
}

/* Build node's string-value into `buf` (cap carried by buf->max). */
static mkr_status_t
build_string_value(const MKR_DOM_NODE *node, mkr_buf_t *buf)
{
  if (node == NULL) return MKR_OK;

  switch (MKR_NODE_TYPE(node)) {
  case MKR_NTYPE_ATTRIBUTE: {
    MKR_DOM_ATTR *attr = (MKR_DOM_ATTR *)node;
    size_t vlen = 0;
    const lxb_char_t *v = MKR_ATTR_VALUE(attr, &vlen);
    return mkr_buf_append(buf, v ? (const char *)v : "", vlen);
  }
  case MKR_NTYPE_TEXT:
  case MKR_NTYPE_CDATA_SECTION:
  case MKR_NTYPE_COMMENT:
  case MKR_NTYPE_PI:
    return append_text_content((MKR_DOM_NODE *)node, buf);
  default:
    return append_text_descendants((MKR_DOM_NODE *)node, buf);
  }
}

static void
mkr_build_node_text_unchecked(const MKR_DOM_NODE *node, mkr_owned_text_t *out)
{
  /* Best-effort node string-value, used only for NUMBER coercion (its sole
   * caller): the text is parsed straight to a double, so mkr_buf's conservative
   * default ceiling (max == 0) is ample - a node whose text exceeds it was never a
   * valid number, and the build then falls back to an owned "" (-> NaN), which is
   * the correct coercion result anyway. On any failure return "" rather than NULL,
   * since callers require a non-NULL text. */
  mkr_owned_text_init(out);
  mkr_buf_t buf;
  mkr_buf_init(&buf, 0);
  if (build_string_value(node, &buf) != MKR_OK) {
    mkr_buf_free(&buf);
    (void)mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text_lit(""), NULL, NULL);
    return;
  }
  if (mkr_owned_text_from_buf_steal(out, &buf, NULL, NULL) != 0) {
    (void)mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text_lit(""), NULL, NULL);
  }
}

static int
mkr_node_to_owned_text_or_fail(const MKR_DOM_NODE *node,
                             mkr_xpath_limits_t *limits,
                             mkr_xpath_error_t *err,
                             mkr_owned_text_t *out)
{
  if (out == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "mkr_node_to_owned_text_or_fail: bad args");
    return -1;
  }
  mkr_owned_text_init(out);
  mkr_buf_t buf;
  mkr_buf_init(&buf, (limits != NULL) ? limits->max_string_bytes : 0);
  mkr_status_t st = build_string_value(node, &buf);
  if (st == MKR_ERR_LIMIT) {
    mkr_buf_free(&buf);
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT,
                "string size limit exceeded (%zu bytes) while building node string-value",
                limits->max_string_bytes);
    return -1;
  }
  if (st != MKR_OK) {
    mkr_buf_free(&buf);
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory building node string-value");
    return -1;
  }
  return mkr_owned_text_from_buf_steal(out, &buf, err, "out of memory building node string-value");
}

static int
mkr_val_to_owned_text_or_fail(const mkr_val_t *v,
                              mkr_xpath_limits_t *limits,
                              mkr_xpath_error_t *err,
                              mkr_owned_text_t *out)
{
  if (out == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "mkr_val_to_owned_text_or_fail: bad args");
    return -1;
  }
  mkr_owned_text_init(out);
  if (v == NULL) {
    return mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text_lit(""), err, "out of memory converting value to string");
  }
  switch (v->type) {
  case MKR_XPATH_TYPE_STRING: {
    mkr_borrowed_text_t text = mkr_borrowed_text_from_owned(v->u.string);
    if (text.ptr == NULL) text.len = 0;
    if (limits != NULL && mkr_limit_check_string_bytes(limits, text.len, err) != 0) return -1;
    return mkr_owned_text_from_borrowed_copy(out, text,
                                             err, "out of memory copying string value");
  }
  case MKR_XPATH_TYPE_BOOLEAN:
    return v->u.boolean
      ? mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text_lit("true"), err, "out of memory converting boolean to string")
      : mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text_lit("false"), err, "out of memory converting boolean to string");
  case MKR_XPATH_TYPE_NUMBER: {
    double d = v->u.number;
    if (isnan(d)) {
      return mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text_lit("NaN"), err, "out of memory converting number to string");
    }
    if (isinf(d)) {
      return d < 0
        ? mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text_lit("-Infinity"), err, "out of memory converting number to string")
        : mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text_lit("Infinity"), err, "out of memory converting number to string");
    }
    if (d == 0.0) {
      return mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text_lit("0"), err, "out of memory converting number to string");
    }
    char buf[64];
    int n;
    if (d == floor(d) && fabs(d) < 1e15) {
      n = snprintf(buf, sizeof(buf), "%lld", (long long)d);
    } else {
      n = snprintf(buf, sizeof(buf), "%.15g", d);
    }
    if (n < 0 || (size_t)n >= sizeof(buf)) {
      mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "number string conversion overflow");
      return -1;
    }
    char *p = mkr_strndup(buf, (size_t)n);
    if (p == NULL) { mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory converting number to string"); return -1; }
    *out = mkr_owned_text(p, (size_t)n);
    return 0;
  }
  case MKR_XPATH_TYPE_NODESET:
    if (v->u.nodeset.count == 0) {
      return mkr_owned_text_from_borrowed_copy(out, mkr_borrowed_text_lit(""), err, "out of memory");
    }
    /* XPath 1.0 §4.2: string(node-set) = string-value of first node in doc order. */
    return mkr_node_to_owned_text_or_fail(v->u.nodeset.items[0], limits, err, out);
  }
  mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "unknown value type");
  return -1;
}

static int
mkr_val_to_number_or_fail(const mkr_val_t *v,
                         mkr_xpath_limits_t *limits,
                         mkr_xpath_error_t *err,
                         double *out)
{
  if (v == NULL || out == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "mkr_val_to_number_or_fail: bad args");
    return -1;
  }
  if (v->type == MKR_XPATH_TYPE_NODESET) {
    if (v->u.nodeset.count == 0) {
      *out = (double)NAN;
      return 0;
    }
    mkr_owned_text_t text;
    if (mkr_node_to_owned_text_or_fail(v->u.nodeset.items[0], limits, err, &text) != 0) return -1;
    *out = mkr_borrowed_text_to_number(mkr_borrowed_text_from_owned(text));
    mkr_owned_text_clear(&text);
    return 0;
  }
  *out = mkr_val_to_number_unchecked(v);
  return 0;
}

/* ---------- coercions ---------- */

/* Skip XPath S whitespace - (#x20 | #x9 | #xD | #xA) ONLY (XPath 1.0 §3.7). NOT
 * C isspace(), which would also swallow #xB (\v) and #xC (\f); those are not
 * XPath whitespace, so a string padded with them must coerce to NaN, not parse. */
static void
mkr_span_skip_xpath_ws(mkr_span_t *s)
{
  for (;;) {
    int c = mkr_span_peek(s);
    if (c == ' ' || c == '\t' || c == '\r' || c == '\n') mkr_span_skip(s, 1);
    else break;
  }
}

/* string -> number coercion (XPath 1.0 §4.4): optional leading whitespace, an
 * optional single '-' (NO whitespace between it and the digits, and NO '+'),
 * then a Number, then optional trailing whitespace - anything else is NaN. The
 * Number scan/convert uses the same grammar-exact, locale-independent helpers as
 * the lexer, so "0x10" / "1e3" / "INF" all coerce to NaN (the extent stops
 * before x/e and the trailing garbage trips the end check). All reads go through
 * the bounded span. */
static double
mkr_borrowed_text_to_number(mkr_borrowed_text_t t)
{
  if (t.ptr == NULL) return (double)NAN;
  mkr_span_t s = mkr_span(t.ptr, t.len);

  mkr_span_skip_xpath_ws(&s);

  int neg = 0;
  if (mkr_span_peek(&s) == '-') { neg = 1; mkr_span_skip(&s, 1); }

  const char *mark = mkr_span_mark(&s);
  size_t extent = mkr_xpath_number_extent(mark, mkr_span_left(&s));
  if (extent == 0) return (double)NAN;
  double d = mkr_xpath_number_from_extent(mark, extent);
  mkr_span_skip(&s, extent);

  mkr_span_skip_xpath_ws(&s);
  if (mkr_span_peek(&s) != -1) return (double)NAN; /* trailing garbage */

  return neg ? -d : d;
}

static double
mkr_val_to_number_unchecked(const mkr_val_t *v)
{
  switch (v->type) {
  case MKR_XPATH_TYPE_NUMBER:
    return v->u.number;
  case MKR_XPATH_TYPE_BOOLEAN:
    return v->u.boolean ? 1.0 : 0.0;
  case MKR_XPATH_TYPE_STRING:
    return mkr_borrowed_text_to_number(mkr_borrowed_text_from_owned(v->u.string));
  case MKR_XPATH_TYPE_NODESET: {
    if (v->u.nodeset.count == 0) return (double)NAN;
    /* string-value of first node in document order */
    mkr_owned_text_t text;
    mkr_build_node_text_unchecked(v->u.nodeset.items[0], &text);
    double d = mkr_borrowed_text_to_number(mkr_borrowed_text_from_owned(text));
    mkr_owned_text_clear(&text);
    return d;
  }
  }
  return (double)NAN;
}

static int
mkr_val_to_boolean(const mkr_val_t *v)
{
  switch (v->type) {
  case MKR_XPATH_TYPE_BOOLEAN:
    return v->u.boolean;
  case MKR_XPATH_TYPE_NUMBER:
    return !(v->u.number == 0.0 || isnan(v->u.number));
  case MKR_XPATH_TYPE_STRING:
    return v->u.string.ptr != NULL && v->u.string.ptr[0] != '\0';
  case MKR_XPATH_TYPE_NODESET:
    return v->u.nodeset.count > 0;
  }
  return 0;
}

/* ---------- document order ---------- */

/*
 * Treat an attribute node as positioned "with" its owner element for
 * cross-subtree comparisons; only when both belong to the same element
 * does the attribute-vs-attribute or attribute-vs-descendant rule kick in.
 */
static const MKR_DOM_NODE *
anchor_for_cmp(const MKR_DOM_NODE *n)
{
  if (MKR_NODE_TYPE(n) == MKR_NTYPE_ATTRIBUTE) {
    return MKR_NODE_PARENT(n) ? MKR_NODE_PARENT(n) : n;
  }
  return n;
}

static int
depth_of(const MKR_DOM_NODE *n)
{
  int d = 0;
  while (MKR_NODE_PARENT(n)) { d++; n = MKR_NODE_PARENT(n); }
  return d;
}

static int
doc_order_cmp(const MKR_DOM_NODE *a, const MKR_DOM_NODE *b)
{
  if (a == b) return 0;
  const MKR_DOM_NODE *aa = anchor_for_cmp(a);
  const MKR_DOM_NODE *bb = anchor_for_cmp(b);

  /* If the anchors are the same element, decide by node type. A non-attribute
   * node that anchors to the same element E can ONLY be E itself: any other
   * node (a child/descendant) anchors to itself, not to E, so it would not
   * reach this branch (the attribute-vs-descendant case is handled below by
   * the depth-normalisation walk). Per XPath 1.0 §5.1 document order is
   * "element, then its attribute nodes, then its children", so an attribute
   * comes AFTER its own owner element. */
  if (aa == bb) {
    int a_attr = (MKR_NODE_TYPE(a) == MKR_NTYPE_ATTRIBUTE);
    int b_attr = (MKR_NODE_TYPE(b) == MKR_NTYPE_ATTRIBUTE);
    if (a_attr && !b_attr) return 1;  /* b is the owner element E; a (its attr) follows */
    if (b_attr && !a_attr) return -1; /* a is the owner element E; b (its attr) follows */
    /* Both attributes of the same element: relative order is
     * implementation-defined. Use insertion order via attr linked list. */
    if (a_attr && b_attr) {
      for (const MKR_DOM_ATTR *at = MKR_ELEM_FIRST_ATTR((const MKR_DOM_ELEMENT *)aa);
           at != NULL; at = MKR_ATTR_NEXT(at)) {
        if ((const MKR_DOM_NODE *)at == a) return -1;
        if ((const MKR_DOM_NODE *)at == b) return 1;
      }
      return 0;
    }
    /* aa == bb but neither is an attribute means a == b, handled above. */
    return 0;
  }

  int da = depth_of(aa), db = depth_of(bb);
  while (da > db) { aa = MKR_NODE_PARENT(aa); da--; }
  while (db > da) { bb = MKR_NODE_PARENT(bb); db--; }
  if (aa == bb) {
    /* One is ancestor of the other; ancestor comes first. */
    return (aa == anchor_for_cmp(a)) ? -1 : 1;
  }
  while (MKR_NODE_PARENT(aa) != MKR_NODE_PARENT(bb)) {
    aa = MKR_NODE_PARENT(aa);
    bb = MKR_NODE_PARENT(bb);
  }
  /* Resolve sibling order. Scan outward from aa and bb in lockstep (via ->next)
   * rather than forward from parent->first_child: the cost is then O(distance
   * between aa and bb), not O(distance from the first child. The latter is
   * quadratic when sorting nodes that sit deep in a wide, flat parent (e.g. a
   * predicate result picking scattered <li> from a 2000-child <ul>), which the
   * doc-order index would only avoid once a single sort reaches its build
   * threshold. */
  if (MKR_NODE_PARENT(aa) == NULL) {
    /* Different documents/roots - undefined; keep stable. */
    return 0;
  }
  const MKR_DOM_NODE *fa = aa, *fb = bb;
  for (;;) {
    fa = fa ? MKR_NODE_NEXT(fa) : NULL;
    fb = fb ? MKR_NODE_NEXT(fb) : NULL;
    if (fa == bb) return -1;            /* bb lies after aa -> aa first */
    if (fb == aa) return 1;             /* aa lies after bb -> bb first */
    if (fa == NULL && fb == NULL) return 0; /* unreachable for same-parent nodes */
  }
}

/* ---------- per-evaluate document-order index (build/lookup/sort) ---------- */

/* Insert (node, ord) into the open-addressing table. Grows when load
 * factor exceeds 3/4. Returns 0 on success, -1 on OOM. */
static int
order_index_insert(mkr_doc_order_index_t *idx, const MKR_DOM_NODE *node, size_t ord)
{
  if (idx->cap == 0 || idx->count * 4 >= idx->cap * 3) {
    size_t new_cap = 256;
    if (idx->cap != 0 && !mkr_size_mul(idx->cap, 2, &new_cap)) {
      return -1; /* overflow */
    }
    void *new_buckets = mkr_callocarray(new_cap, sizeof(*idx->buckets));
    if (new_buckets == NULL) return -1;
    /* Rehash. */
    typeof(idx->buckets) old_buckets = idx->buckets;
    size_t               old_cap     = idx->cap;
    idx->buckets = new_buckets;
    idx->cap     = new_cap;
    idx->count   = 0;
    for (size_t i = 0; i < old_cap; ++i) {
      if (old_buckets[i].node != NULL) {
        size_t mask = new_cap - 1;
        size_t j = mkr_ptr_hash(old_buckets[i].node) & mask;
        while (idx->buckets[j].node != NULL) j = (j + 1) & mask;
        idx->buckets[j].node = old_buckets[i].node;
        idx->buckets[j].ord  = old_buckets[i].ord;
        idx->count++;
      }
    }
    free(old_buckets);
  }
  size_t mask = idx->cap - 1;
  size_t j = mkr_ptr_hash(node) & mask;
  while (idx->buckets[j].node != NULL) {
    if (idx->buckets[j].node == node) return 0; /* already present */
    j = (j + 1) & mask;
  }
  idx->buckets[j].node = node;
  idx->buckets[j].ord  = ord;
  idx->count++;
  return 0;
}

static int
order_index_lookup(const mkr_doc_order_index_t *idx, const MKR_DOM_NODE *node,
                   size_t *out_ord)
{
  if (idx->cap == 0) return -1;
  size_t mask = idx->cap - 1;
  size_t j = mkr_ptr_hash(node) & mask;
  while (idx->buckets[j].node != NULL) {
    if (idx->buckets[j].node == node) {
      if (out_ord) *out_ord = idx->buckets[j].ord;
      return 0;
    }
    j = (j + 1) & mask;
  }
  return -1;
}

/* DFS pre-order: assign ordinal to the element, then its attributes
 * (in linked-list order, before children), then descendants. This
 * matches doc_order_cmp's attribute placement.
 *
 * Iterative (parent-pointer) walk rather than C recursion, so an adversarially
 * deep tree cannot overflow the stack (fail-closed / no DoS); O(1) extra space.
 * The traversal stays within the subtree rooted at `root` (it never follows
 * root->next). */
static int
order_index_walk(mkr_doc_order_index_t *idx, MKR_DOM_NODE *root, size_t *next_ord)
{
  MKR_DOM_NODE *cur = root;
  while (cur != NULL) {
    /* Visit (pre-order): the node, then its attributes before any child. */
    if (order_index_insert(idx, cur, (*next_ord)++) != 0) return -1;
    if (MKR_NODE_TYPE(cur) == MKR_NTYPE_ELEMENT) {
      MKR_DOM_ELEMENT *el = (MKR_DOM_ELEMENT *)cur;
      for (MKR_DOM_ATTR *a = MKR_ELEM_FIRST_ATTR(el); a != NULL; a = MKR_ATTR_NEXT(a)) {
        if (order_index_insert(idx, (MKR_DOM_NODE *)a, (*next_ord)++) != 0) return -1;
      }
    }
    if (MKR_NODE_FIRST_CHILD(cur) != NULL) {
      cur = MKR_NODE_FIRST_CHILD(cur);
      continue;
    }
    while (cur != root && MKR_NODE_NEXT(cur) == NULL) {
      cur = MKR_NODE_PARENT(cur);
    }
    if (cur == root) break;
    cur = MKR_NODE_NEXT(cur);
  }
  return 0;
}

static int
order_index_build(mkr_doc_order_index_t *idx, MKR_DOM_NODE *root,
                  mkr_xpath_error_t *err)
{
  if (idx->built) return 0;
  if (root == NULL) return -1;
  size_t next_ord = 0;
  if (order_index_walk(idx, root, &next_ord) != 0) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory building document order index");
    mkr_doc_order_index_clear(idx);
    return -1;
  }
  idx->built = 1;
  return 0;
}

/* Indexed comparator. Falls back to doc_order_cmp on any miss
 * (e.g., synthesised nodes or cross-document compares). */
static int
doc_order_cmp_ctx(mkr_xpath_context_t *ctx, const MKR_DOM_NODE *a, const MKR_DOM_NODE *b)
{
  if (a == b) return 0;
  if (ctx == NULL) return doc_order_cmp(a, b);
  mkr_doc_order_index_t *idx = mkr_ctx_order_index(ctx);
  if (idx == NULL || !idx->built) return doc_order_cmp(a, b);
  size_t oa, ob;
  if (order_index_lookup(idx, a, &oa) != 0) return doc_order_cmp(a, b);
  if (order_index_lookup(idx, b, &ob) != 0) return doc_order_cmp(a, b);
  /* Safe comparison - compare, don't subtract (unsigned difference wraps). */
  if (oa < ob) return -1;
  if (oa > ob) return 1;
  return 0;
}

/* Bottom-up merge sort. Threading ctx through avoids the qsort_r /
 * thread-local hack and keeps everything reentrant. Stable as a
 * bonus: ties (same ord - only possible for synthesised nodes that
 * weren't in the index) preserve insertion order. */
static void
ms_merge(void **arr, void **tmp,
         size_t lo, size_t mid, size_t hi, mkr_xpath_context_t *ctx)
{
  size_t i = lo, j = mid, k = lo;
  while (i < mid && j < hi) {
    if (doc_order_cmp_ctx(ctx, arr[i], arr[j]) <= 0) tmp[k++] = arr[i++];
    else                                              tmp[k++] = arr[j++];
  }
  while (i < mid) tmp[k++] = arr[i++];
  while (j < hi)  tmp[k++] = arr[j++];
  for (size_t x = lo; x < hi; ++x) arr[x] = tmp[x];
}

static void
ms_sort(void **arr, void **tmp,
        size_t lo, size_t hi, mkr_xpath_context_t *ctx)
{
  if (hi - lo < 2) return;
  size_t mid = lo + (hi - lo) / 2;
  ms_sort(arr, tmp, lo, mid, ctx);
  ms_sort(arr, tmp, mid, hi, ctx);
  ms_merge(arr, tmp, lo, mid, hi, ctx);
}

/* qsort fallback used only when tmp-buffer allocation fails. */
static int
doc_order_qsort_cb_fallback(const void *pa, const void *pb)
{
  const MKR_DOM_NODE *a = *(const MKR_DOM_NODE * const *)pa;
  const MKR_DOM_NODE *b = *(const MKR_DOM_NODE * const *)pb;
  return doc_order_cmp(a, b);
}

/* Threshold for building the doc-order index. Below this we expect
 * N log N parent-chain compares to be cheaper than the O(D) full-doc
 * walk that the index requires (D = total nodes in document, which is
 * typically 6000+ on real pages). Empirically the crossover sits
 * somewhere between N=100 and N=300 on coffee.html; we pick a safe
 * point that keeps small unions and reverse-axis dedups off the slow
 * build path. Once the index IS built (e.g., by a larger sort earlier
 * in the same evaluate), subsequent small sorts naturally reuse it. */
#define MKR_INDEX_BUILD_MIN 200

static void
mkr_nodeset_sort_doc_order(mkr_xpath_context_t *ctx, mkr_nodeset_t *ns)
{
  if (ns == NULL || ns->count < 2) return;

  /* Already-sorted fast path. A relative step over a multi-node context
   * (e.g. the child step of //li/a or //a:entry/a:title) collects its
   * forward-axis results context-by-context in document order, so when the
   * contexts are non-nested the concatenation is ALREADY in document order and
   * the O(n log n) sort is pure waste. An O(n) scan confirms it: if every
   * adjacent pair is already in order we return without sorting (and without
   * building the doc-order index). Reverse axes and interleaved (nested-context)
   * results fail the scan early and fall through to the full sort below. The
   * scan uses the same comparator the sort would, so it can only skip work,
   * never change the result. This is the libxml2-parity win for multi-step
   * paths, where the sort otherwise dominates (profiled). */
  int already_ordered = 1;
  for (size_t i = 1; i < ns->count; ++i) {
    if (doc_order_cmp_ctx(ctx, ns->items[i - 1], ns->items[i]) > 0) {
      already_ordered = 0;
      break;
    }
  }
  if (already_ordered) return;

  /* Lazy build of the doc-order index. Only worth doing when the sort
   * itself is large enough to amortise the full-doc walk; smaller
   * sorts fall through to parent-chain compares via doc_order_cmp_ctx
   * (which sees an unbuilt index and dispatches accordingly). */
  mkr_doc_order_index_t *idx = mkr_ctx_order_index(ctx);
  if (idx != NULL && !idx->built && ns->count >= MKR_INDEX_BUILD_MIN) {
    MKR_DOM_NODE *root = (MKR_DOM_NODE *)mkr_ctx_document(ctx);
    mkr_xpath_error_t ierr = {0};
    (void)order_index_build(idx, root, &ierr);
    mkr_xpath_error_clear(&ierr); /* index is best-effort; on OOM we fall through to parent-chain cmp */
  }

  void **tmp = mkr_reallocarray(NULL, ns->count, sizeof(*tmp));
  if (tmp == NULL) {
    /* Fall back to in-place qsort with parent-chain compare (slow but
     * correct). Should be a very rare path. */
    qsort(ns->items, ns->count, sizeof(ns->items[0]), doc_order_qsort_cb_fallback);
    return;
  }
  ms_sort(ns->items, tmp, 0, ns->count, ctx);
  free(tmp);
}

static void
mkr_nodeset_unique_sorted(mkr_xpath_context_t *ctx, mkr_nodeset_t *ns)
{
  if (ns == NULL || ns->count < 2) return;
  mkr_nodeset_sort_doc_order(ctx, ns);
  size_t w = 1;
  for (size_t r = 1; r < ns->count; ++r) {
    if (ns->items[r] != ns->items[r - 1]) {
      ns->items[w++] = ns->items[r];
    }
  }
  ns->count = w;
}

/* ---------- string-value cache: node-keyed insert (dereferences `node`) ---------- */

static int
mkr_get_cached_node_text(mkr_xpath_context_t *ctx,
                         MKR_DOM_NODE *node,
                         mkr_borrowed_text_t *out,
                         mkr_xpath_error_t *err)
{
  if (out == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "mkr_get_cached_node_text: bad args");
    return -1;
  }
  *out = mkr_borrowed_text(NULL, 0);
  /* Contract: ctx is non-NULL when called from the evaluator (the only
   * intended caller). A NULL ctx is a programming error; surface it. */
  mkr_str_cache_t *c = mkr_ctx_str_cache(ctx);
  if (c == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL,
               "mkr_get_cached_node_text called without a context");
    return -1;
  }

  /* O(1) lookup via the pointer-keyed index. */
  if (c->bucket_cap != 0) {
    size_t mask = c->bucket_cap - 1;
    size_t j = mkr_ptr_hash(node) & mask;
    while (c->buckets[j] != 0) {
      mkr_str_cache_entry_t *e = &c->entries[c->buckets[j] - 1];
      if (e->node == node) {
        *out = mkr_borrowed_text(e->str, e->len);
        return 0;
      }
      j = (j + 1) & mask;
    }
  }

  mkr_owned_text_t text;
  if (mkr_node_to_owned_text_or_fail(node, mkr_ctx_limits(ctx), err, &text) != 0) return -1;

  if (mkr_grow_reserve((void **)&c->entries, &c->cap, c->count + 1,
                       sizeof(*c->entries)) != MKR_OK) {
    mkr_owned_text_clear(&text);
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in node string cache");
    return -1;
  }

  /* Enforce a total cap on the cached string bytes (fail-closed). */
  size_t new_total;
  if (!mkr_size_add(c->total_bytes, text.len, &new_total)
      || mkr_limit_check_string_bytes(mkr_ctx_limits(ctx), new_total, err) != 0) {
    mkr_owned_text_clear(&text);
    return -1;
  }

  c->entries[c->count].node = node;
  c->entries[c->count].str  = text.ptr;
  c->entries[c->count].len  = text.len;

  /* Grow / build the index, keeping load factor <= 1/2. */
  if (c->bucket_cap == 0 || (c->count + 1) * 2 > c->bucket_cap) {
    size_t new_bucket_cap = 64;
    if (c->bucket_cap != 0 && !mkr_size_mul(c->bucket_cap, 2, &new_bucket_cap)) {
      mkr_owned_text_clear(&text);
      c->entries[c->count].node = NULL;
      c->entries[c->count].str = NULL;
      c->entries[c->count].len = 0;
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "node string cache index overflow");
      return -1;
    }
    if (mkr_str_cache_reindex(c, new_bucket_cap) != 0) {
      mkr_owned_text_clear(&text);
      c->entries[c->count].node = NULL;
      c->entries[c->count].str = NULL;
      c->entries[c->count].len = 0;
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory indexing node string cache");
      return -1;
    }
  }
  mkr_str_cache_index_put(c, c->count);
  c->total_bytes += text.len;
  c->count++;

  *out = mkr_borrowed_text_from_owned(text);
  return 0;
}
