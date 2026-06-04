#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"
#include "mkr_node_access.h"

#include <lexbor/dom/dom.h>
#include <ctype.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/*
 * Runtime values: node-sets, type coercions, and node string-values.
 * Also hosts the small AST destructor helpers.
 */

/* ---------- node-set ---------- */

void
mkr_nodeset_init(mkr_nodeset_t *ns)
{
  ns->items    = NULL;
  ns->count    = 0;
  ns->capacity = 0;
}

int
mkr_nodeset_push(mkr_nodeset_t *ns, lxb_dom_node_t *node,
                mkr_xpath_limits_t *limits, mkr_xpath_error_t *err)
{
  if (node == NULL) return 0;
  if (limits != NULL && mkr_limit_check_nodeset_size(limits, ns->count + 1, err) != 0) {
    return -1;
  }
  if (mkr_grow_reserve((void **)&ns->items, &ns->capacity, ns->count + 1,
                       sizeof(*ns->items)) != MKR_OK) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory growing node-set");
    return -1;
  }
  ns->items[ns->count++] = node;
  return 0;
}

void
mkr_nodeset_clear(mkr_nodeset_t *ns)
{
  if (ns == NULL) return;
  free(ns->items);
  ns->items    = NULL;
  ns->count    = 0;
  ns->capacity = 0;
}

void
mkr_owned_text_init(mkr_owned_text_t *t)
{
  if (t == NULL) return;
  t->ptr = NULL;
  t->len = 0;
}

void
mkr_owned_text_clear(mkr_owned_text_t *t)
{
  if (t == NULL) return;
  free(t->ptr);
  t->ptr = NULL;
  t->len = 0;
}

int
mkr_borrowed_text_eq(mkr_borrowed_text_t a, mkr_borrowed_text_t b)
{
  if (a.ptr == NULL || b.ptr == NULL) return a.ptr == b.ptr;
  return a.len == b.len && memcmp(a.ptr, b.ptr, a.len) == 0;
}

/* Copy an already-valid borrowed text into owned storage. Taking
 * mkr_borrowed_text_t (not raw char*+len) keeps the type contract: an
 * mkr_owned_text_t can only be minted from text the caller has asserted valid
 * (via mkr_borrowed_text / mkr_borrowed_text_from_verified /
 * mkr_borrowed_text_from_owned), so every raw-bytes -> text entry point is
 * greppable. */
int
mkr_owned_text_from_borrowed_copy(mkr_owned_text_t *out, mkr_borrowed_text_t t,
                                  mkr_xpath_error_t *err, const char *what)
{
  if (out == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "mkr_owned_text_from_borrowed_copy: bad args");
    return -1;
  }
  mkr_owned_text_init(out);
  const char *s = t.ptr ? t.ptr : "";
  size_t len = t.ptr ? t.len : 0;
  char *p = mkr_strndup(s, len);
  if (p == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, what ? what : "out of memory copying text");
    return -1;
  }
  out->ptr = p;
  out->len = len;
  return 0;
}

int
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

void
mkr_val_set_owned_text(mkr_val_t *v, mkr_owned_text_t text)
{
  if (v == NULL) return;
  v->type = MKR_XPATH_TYPE_STRING;
  v->u.string = text;
}

/* Set +v+ to a STRING by copying a borrowed view: the engine allocates and owns
 * the copy. This is how callers outside the engine (the glue handler bridge)
 * hand a string into a value — they pass what they have, a borrowed slice, and
 * never construct an mkr_owned_text_t themselves. Keeping the copy-and-own step
 * here keeps allocation and freeing of owned strings in one layer. Returns 0 on
 * success, -1 on OOM (err populated; +v+ left untouched). */
int
mkr_val_set_borrowed_text_copy(mkr_val_t *v, mkr_borrowed_text_t text,
                               mkr_xpath_error_t *err, const char *what)
{
  if (v == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "mkr_val_set_borrowed_text_copy: bad args");
    return -1;
  }
  mkr_owned_text_t owned;
  if (mkr_owned_text_from_borrowed_copy(&owned, text, err, what) != 0) {
    return -1;
  }
  mkr_val_set_owned_text(v, owned);
  return 0;
}

/* ---------- value ---------- */

void
mkr_val_clear(mkr_val_t *v)
{
  if (v == NULL) return;
  switch (v->type) {
  case MKR_XPATH_TYPE_NODESET:
    mkr_nodeset_clear(&v->u.nodeset);
    break;
  case MKR_XPATH_TYPE_STRING:
    mkr_owned_text_clear(&v->u.string);
    break;
  default:
    break;
  }
  memset(v, 0, sizeof(*v));
}

int
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
    lxb_dom_node_t **items;
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

/* ---------- node string-value (XPath 1.0 §5) ---------- */

/* ---------- node string-value (XPath 1.0 §5) ----------
 *
 * Built into an mkr_buf_t whose `max` is the per-evaluate byte cap: append fails
 * closed with MKR_ERR_LIMIT past the cap and MKR_ERR_OOM on allocation failure,
 * so there is never a partial/truncated result. Lexbor-allocated text is freed
 * after each append (otherwise we'd leak document-arena memory on every XPath
 * that touches text content). */

/* Append `node`'s own text content. */
static mkr_status_t
append_text_content(lxb_dom_node_t *node, mkr_buf_t *buf)
{
  size_t tlen = 0;
  lxb_char_t *t = lxb_dom_node_text_content(node, &tlen);
  if (t == NULL) return MKR_OK;
  mkr_status_t st = mkr_buf_append(buf, t, tlen);
  lxb_dom_document_destroy_text(node->owner_document, t);
  return st;
}

/* Append the string-value of every TEXT descendant of `node`, in document
 * order. Iterative (parent-pointer) pre-order walk rather than C recursion, so
 * an adversarially deep tree cannot overflow the stack (fail-closed / no DoS);
 * O(1) extra space. Descends only into elements, matching the original. */
static mkr_status_t
append_text_descendants(lxb_dom_node_t *node, mkr_buf_t *buf)
{
  lxb_dom_node_t *cur = MKR_NODE_FIRST_CHILD(node);
  while (cur != NULL) {
    if (MKR_NODE_TYPE(cur) == MKR_NTYPE_TEXT) {
      mkr_status_t st = append_text_content(cur, buf);
      if (st != MKR_OK) return st; /* LIMIT or OOM — caller fails closed */
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
build_string_value(const lxb_dom_node_t *node, mkr_buf_t *buf)
{
  if (node == NULL) return MKR_OK;

  switch (MKR_NODE_TYPE(node)) {
  case MKR_NTYPE_ATTRIBUTE: {
    lxb_dom_attr_t *attr = (lxb_dom_attr_t *)node;
    size_t vlen = 0;
    const lxb_char_t *v = MKR_ATTR_VALUE(attr, &vlen);
    return mkr_buf_append(buf, v ? (const char *)v : "", vlen);
  }
  case MKR_NTYPE_TEXT:
  case MKR_NTYPE_CDATA_SECTION:
  case MKR_NTYPE_COMMENT:
  case MKR_NTYPE_PI:
    return append_text_content((lxb_dom_node_t *)node, buf);
  default:
    return append_text_descendants((lxb_dom_node_t *)node, buf);
  }
}

static void
mkr_build_node_text_unchecked(const lxb_dom_node_t *node, mkr_owned_text_t *out)
{
  /* Uncapped, best-effort: callers (number/string coercion) require a non-NULL
   * text, so on any failure fall back to an owned "" rather than NULL. */
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

int
mkr_node_to_owned_text_or_fail(const lxb_dom_node_t *node,
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

int
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

int
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

double
mkr_borrowed_text_to_number(mkr_borrowed_text_t t)
{
  if (t.ptr == NULL) return (double)NAN;
  const char *s = t.ptr;
  while (*s && isspace((unsigned char)*s)) s++;
  if (*s == '\0') return (double)NAN;
  char *end = NULL;
  double d = strtod(s, &end);
  if (end == s) return (double)NAN;
  while (*end && isspace((unsigned char)*end)) end++;
  if (*end != '\0') return (double)NAN;
  return d;
}

double
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

int
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
static const lxb_dom_node_t *
anchor_for_cmp(const lxb_dom_node_t *n)
{
  if (MKR_NODE_TYPE(n) == MKR_NTYPE_ATTRIBUTE) {
    return MKR_NODE_PARENT(n) ? MKR_NODE_PARENT(n) : n;
  }
  return n;
}

static int
depth_of(const lxb_dom_node_t *n)
{
  int d = 0;
  while (MKR_NODE_PARENT(n)) { d++; n = MKR_NODE_PARENT(n); }
  return d;
}

static int
doc_order_cmp(const lxb_dom_node_t *a, const lxb_dom_node_t *b)
{
  if (a == b) return 0;
  const lxb_dom_node_t *aa = anchor_for_cmp(a);
  const lxb_dom_node_t *bb = anchor_for_cmp(b);

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
      for (const lxb_dom_attr_t *at = MKR_ELEM_FIRST_ATTR((const lxb_dom_element_t *)aa);
           at != NULL; at = MKR_ATTR_NEXT(at)) {
        if ((const lxb_dom_node_t *)at == a) return -1;
        if ((const lxb_dom_node_t *)at == b) return 1;
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
    /* Different documents/roots — undefined; keep stable. */
    return 0;
  }
  const lxb_dom_node_t *fa = aa, *fb = bb;
  for (;;) {
    fa = fa ? MKR_NODE_NEXT(fa) : NULL;
    fb = fb ? MKR_NODE_NEXT(fb) : NULL;
    if (fa == bb) return -1;            /* bb lies after aa -> aa first */
    if (fb == aa) return 1;             /* aa lies after bb -> bb first */
    if (fa == NULL && fb == NULL) return 0; /* unreachable for same-parent nodes */
  }
}

/* ---------- per-evaluate document-order index ---------- */

static uint32_t
pointer_hash(const void *p)
{
  uintptr_t x = (uintptr_t)p;
  /* SplitMix-style mixing — cheap and good enough for pointer keys. */
  x = (x ^ (x >> 16)) * 0x9E3779B9u;
  x = (x ^ (x >> 13)) * 0x85EBCA6Bu;
  return (uint32_t)(x ^ (x >> 16));
}

void
mkr_doc_order_index_init(mkr_doc_order_index_t *idx)
{
  idx->buckets = NULL;
  idx->cap     = 0;
  idx->count   = 0;
  idx->built   = 0;
}

void
mkr_doc_order_index_clear(mkr_doc_order_index_t *idx)
{
  if (idx == NULL) return;
  free(idx->buckets);
  idx->buckets = NULL;
  idx->cap     = 0;
  idx->count   = 0;
  idx->built   = 0;
}

/* Insert (node, ord) into the open-addressing table. Grows when load
 * factor exceeds 3/4. Returns 0 on success, -1 on OOM. */
static int
order_index_insert(mkr_doc_order_index_t *idx, const lxb_dom_node_t *node, size_t ord)
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
        size_t j = pointer_hash(old_buckets[i].node) & mask;
        while (idx->buckets[j].node != NULL) j = (j + 1) & mask;
        idx->buckets[j].node = old_buckets[i].node;
        idx->buckets[j].ord  = old_buckets[i].ord;
        idx->count++;
      }
    }
    free(old_buckets);
  }
  size_t mask = idx->cap - 1;
  size_t j = pointer_hash(node) & mask;
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
order_index_lookup(const mkr_doc_order_index_t *idx, const lxb_dom_node_t *node,
                   size_t *out_ord)
{
  if (idx->cap == 0) return -1;
  size_t mask = idx->cap - 1;
  size_t j = pointer_hash(node) & mask;
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
order_index_walk(mkr_doc_order_index_t *idx, lxb_dom_node_t *root, size_t *next_ord)
{
  lxb_dom_node_t *cur = root;
  while (cur != NULL) {
    /* Visit (pre-order): the node, then its attributes before any child. */
    if (order_index_insert(idx, cur, (*next_ord)++) != 0) return -1;
    if (MKR_NODE_TYPE(cur) == MKR_NTYPE_ELEMENT) {
      lxb_dom_element_t *el = (lxb_dom_element_t *)cur;
      for (lxb_dom_attr_t *a = MKR_ELEM_FIRST_ATTR(el); a != NULL; a = MKR_ATTR_NEXT(a)) {
        if (order_index_insert(idx, (lxb_dom_node_t *)a, (*next_ord)++) != 0) return -1;
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
order_index_build(mkr_doc_order_index_t *idx, lxb_dom_node_t *root,
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
doc_order_cmp_ctx(mkr_xpath_context_t *ctx, const lxb_dom_node_t *a, const lxb_dom_node_t *b)
{
  if (a == b) return 0;
  if (ctx == NULL) return doc_order_cmp(a, b);
  mkr_doc_order_index_t *idx = mkr_ctx_order_index(ctx);
  if (idx == NULL || !idx->built) return doc_order_cmp(a, b);
  size_t oa, ob;
  if (order_index_lookup(idx, a, &oa) != 0) return doc_order_cmp(a, b);
  if (order_index_lookup(idx, b, &ob) != 0) return doc_order_cmp(a, b);
  /* Safe comparison — compare, don't subtract (unsigned difference wraps). */
  if (oa < ob) return -1;
  if (oa > ob) return 1;
  return 0;
}

/* Bottom-up merge sort. Threading ctx through avoids the qsort_r /
 * thread-local hack and keeps everything reentrant. Stable as a
 * bonus: ties (same ord — only possible for synthesised nodes that
 * weren't in the index) preserve insertion order. */
static void
ms_merge(lxb_dom_node_t **arr, lxb_dom_node_t **tmp,
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
ms_sort(lxb_dom_node_t **arr, lxb_dom_node_t **tmp,
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
  const lxb_dom_node_t *a = *(const lxb_dom_node_t * const *)pa;
  const lxb_dom_node_t *b = *(const lxb_dom_node_t * const *)pb;
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

void
mkr_nodeset_sort_doc_order(mkr_xpath_context_t *ctx, mkr_nodeset_t *ns)
{
  if (ns == NULL || ns->count < 2) return;

  /* Lazy build of the doc-order index. Only worth doing when the sort
   * itself is large enough to amortise the full-doc walk; smaller
   * sorts fall through to parent-chain compares via doc_order_cmp_ctx
   * (which sees an unbuilt index and dispatches accordingly). */
  mkr_doc_order_index_t *idx = mkr_ctx_order_index(ctx);
  if (idx != NULL && !idx->built && ns->count >= MKR_INDEX_BUILD_MIN) {
    lxb_dom_node_t *root = (lxb_dom_node_t *)mkr_ctx_document(ctx);
    mkr_xpath_error_t ierr = {0};
    (void)order_index_build(idx, root, &ierr);
    mkr_xpath_error_clear(&ierr); /* index is best-effort; on OOM we fall through to parent-chain cmp */
  }

  lxb_dom_node_t **tmp = mkr_reallocarray(NULL, ns->count, sizeof(*tmp));
  if (tmp == NULL) {
    /* Fall back to in-place qsort with parent-chain compare (slow but
     * correct). Should be a very rare path. */
    qsort(ns->items, ns->count, sizeof(ns->items[0]), doc_order_qsort_cb_fallback);
    return;
  }
  ms_sort(ns->items, tmp, 0, ns->count, ctx);
  free(tmp);
}

void
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

/* ---------- per-evaluation string-value cache ---------- */

void
mkr_str_cache_init(mkr_str_cache_t *c)
{
  c->entries    = NULL;
  c->count      = 0;
  c->cap        = 0;
  c->buckets    = NULL;
  c->bucket_cap = 0;
}

/* Insert entry index `idx` (keyed by entries[idx].node) into the index. The
 * index must have room (callers grow/rehash first). */
static void
mkr_str_cache_index_put(mkr_str_cache_t *c, size_t idx)
{
  size_t mask = c->bucket_cap - 1;
  size_t j = pointer_hash(c->entries[idx].node) & mask;
  while (c->buckets[j] != 0) {
    j = (j + 1) & mask;
  }
  c->buckets[j] = idx + 1;
}

/* Rebuild the index from entries[0, count). Returns -1 on OOM. */
static int
mkr_str_cache_reindex(mkr_str_cache_t *c, size_t bucket_cap)
{
  size_t *buckets = mkr_callocarray(bucket_cap, sizeof(*buckets));
  if (buckets == NULL) return -1;
  free(c->buckets);
  c->buckets    = buckets;
  c->bucket_cap = bucket_cap;
  for (size_t i = 0; i < c->count; ++i) {
    mkr_str_cache_index_put(c, i);
  }
  return 0;
}

void
mkr_str_cache_truncate(mkr_str_cache_t *c, size_t target_count)
{
  if (c == NULL || target_count >= c->count) return;
  for (size_t i = target_count; i < c->count; ++i) {
    free(c->entries[i].str);
  }
  c->count = target_count;
  /* Drop the removed nodes from the index. A full truncate just clears it;
   * a partial one (nested-eval snapshot restore) rebuilds from what remains. */
  if (c->buckets != NULL) {
    if (target_count == 0) {
      size_t buckets_bytes;
      if (!mkr_size_mul(c->bucket_cap, sizeof(*c->buckets), &buckets_bytes)) {
        free(c->buckets);
        c->buckets = NULL;
        c->bucket_cap = 0;
        return;
      }
      memset(c->buckets, 0, buckets_bytes);
    } else {
      mkr_str_cache_reindex(c, c->bucket_cap);
    }
  }
}

void
mkr_str_cache_clear(mkr_str_cache_t *c)
{
  if (c == NULL) return;
  for (size_t i = 0; i < c->count; ++i) {
    free(c->entries[i].str);
  }
  free(c->entries);
  free(c->buckets);
  c->entries    = NULL;
  c->count      = 0;
  c->cap        = 0;
  c->buckets    = NULL;
  c->bucket_cap = 0;
}

int
mkr_get_cached_node_text(mkr_xpath_context_t *ctx,
                         lxb_dom_node_t *node,
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
    size_t j = pointer_hash(node) & mask;
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
  c->count++;

  *out = mkr_borrowed_text_from_owned(text);
  return 0;
}

/* ---------- AST destructors ---------- */

void
mkr_step_clear(mkr_step_t *s)
{
  if (s == NULL) return;
  mkr_owned_text_clear(&s->test.prefix);
  mkr_owned_text_clear(&s->test.local);
  mkr_owned_text_clear(&s->test.pi_target);
  for (size_t i = 0; i < s->npredicates; ++i) {
    mkr_node_free(s->predicates[i]);
  }
  free(s->predicates);
  memset(s, 0, sizeof(*s));
}

/* ---------- AST hoisting helpers ---------- */

/* Pure XPath 1.0 built-ins safe to hoist when all args are CI. Listed
 * explicitly to keep the set conservative. Functions that read the
 * context node (last/position, 0-arg string/normalize-space/local-
 * name/etc., lang) or that may depend on dynamic state (id, handler-
 * routed) are intentionally absent. */
static int
is_pure_builtin_name(const char *name, size_t nargs)
{
  if (name == NULL) return 0;
  /* 0-arg only — these read no input. */
  if (nargs == 0) {
    return strcmp(name, "true") == 0 || strcmp(name, "false") == 0;
  }
  /* n-arg pure functions — all args must themselves be CI (checked
   * by the caller). */
  static const char *pure_names[] = {
    "count", "string-length", "number", "boolean", "not",
    "floor", "ceiling", "round", "sum",
    "concat", "starts-with", "contains",
    "substring-before", "substring-after", "substring",
    "translate",
    NULL,
  };
  for (size_t i = 0; pure_names[i]; ++i) {
    if (strcmp(pure_names[i], name) == 0) return 1;
  }
  return 0;
}

static void
mark_step_predicates(mkr_step_t *s)
{
  for (size_t i = 0; i < s->npredicates; ++i) {
    mkr_mark_context_independent(s->predicates[i]);
  }
}

void
mkr_mark_context_independent(mkr_node_t *n)
{
  if (n == NULL) return;
  int ci = 0;
  switch (n->kind) {
  case MKR_NK_LITERAL_STR:
  case MKR_NK_LITERAL_NUM:
    ci = 1;
    break;
  case MKR_NK_VARREF:
    /* Conservative: variables not hoisted even though XPath 1.0 says
     * they're fixed per evaluation. */
    ci = 0;
    break;
  case MKR_NK_FNCALL: {
    /* Recurse first so subtrees get their own CI marks even when this
     * call itself is not hoistable. */
    for (size_t i = 0; i < n->u.fncall.nargs; ++i) {
      mkr_mark_context_independent(n->u.fncall.args[i]);
    }
    if (n->u.fncall.prefix.ptr != NULL) {
      ci = 0; /* Handler-routed or namespaced builtins → non-CI. */
      break;
    }
    if (!is_pure_builtin_name(n->u.fncall.name.ptr, n->u.fncall.nargs)) {
      ci = 0;
      break;
    }
    ci = 1;
    for (size_t i = 0; i < n->u.fncall.nargs; ++i) {
      if (!n->u.fncall.args[i]->is_context_independent) { ci = 0; break; }
    }
    break;
  }
  case MKR_NK_UNARY:
    mkr_mark_context_independent(n->u.unary.expr);
    ci = n->u.unary.expr ? n->u.unary.expr->is_context_independent : 0;
    break;
  case MKR_NK_BINOP:
    mkr_mark_context_independent(n->u.binop.lhs);
    mkr_mark_context_independent(n->u.binop.rhs);
    ci = (n->u.binop.lhs && n->u.binop.lhs->is_context_independent)
      && (n->u.binop.rhs && n->u.binop.rhs->is_context_independent);
    break;
  case MKR_NK_PATH:
    /* Absolute path is CI: seed is the document root regardless of
     * outer context. Relative paths use the outer context node and
     * are not hoistable. Predicates inside the path are evaluated
     * against the path's own context, so their position()/last() do
     * not leak — recurse so any pure sub-expressions still get marks. */
    ci = n->u.path.absolute ? 1 : 0;
    for (size_t i = 0; i < n->u.path.nsteps; ++i) {
      mark_step_predicates(&n->u.path.steps[i]);
    }
    break;
  case MKR_NK_FILTER:
    /* Conservative: filter expressions are not hoisted in v1. */
    ci = 0;
    mkr_mark_context_independent(n->u.filter.expr);
    for (size_t i = 0; i < n->u.filter.npreds; ++i) {
      mkr_mark_context_independent(n->u.filter.preds[i]);
    }
    for (size_t i = 0; i < n->u.filter.npath; ++i) {
      mark_step_predicates(&n->u.filter.path_steps[i]);
    }
    break;
  }
  n->is_context_independent = (uint8_t)ci;
}

static void
clear_memos_step(mkr_step_t *s)
{
  for (size_t i = 0; i < s->npredicates; ++i) {
    mkr_node_clear_memos(s->predicates[i]);
  }
}

/* ---------- peephole: //X fusion ---------- */

/*
 * Collapse pairs of consecutive steps:
 *   (axis=descendant-or-self, test=node(), no predicates)
 *   (axis=child,              test=*,      no predicates)
 * into a single
 *   (axis=descendant,         test=*,      no predicates)
 *
 * The fusion is safe per XPath 1.0 only when the child step has no
 * predicates: otherwise '//X[1]' would change meaning ("first X per
 * parent" vs "first X in doc order"). The synthesised // step always
 * has no predicates by construction, so we don't need to check the
 * first step's predicate list — only the child step's.
 */
static void
fuse_descendant_or_self_steps(mkr_step_t *steps, size_t *nsteps_ptr)
{
  if (steps == NULL || *nsteps_ptr < 2) return;
  size_t nsteps = *nsteps_ptr;
  size_t w = 0, r = 0;
  while (r < nsteps) {
    if (r + 1 < nsteps
        && steps[r].axis == MKR_AXIS_DESCENDANT_OR_SELF
        && steps[r].test.kind == MKR_NT_NODE
        && steps[r].test.prefix.ptr == NULL
        && steps[r].npredicates == 0
        && steps[r + 1].axis == MKR_AXIS_CHILD
        && steps[r + 1].npredicates == 0) {
      /* Drop the desc-or-self step and promote the child step. */
      mkr_step_clear(&steps[r]);
      steps[w] = steps[r + 1];
      memset(&steps[r + 1], 0, sizeof(steps[r + 1]));
      steps[w].axis = MKR_AXIS_DESCENDANT;
      w++;
      r += 2;
    } else {
      if (w != r) {
        steps[w] = steps[r];
        memset(&steps[r], 0, sizeof(steps[r]));
      }
      w++;
      r++;
    }
  }
  *nsteps_ptr = w;
}

void
mkr_apply_peephole(mkr_node_t *n)
{
  if (n == NULL) return;
  switch (n->kind) {
  case MKR_NK_FNCALL:
    for (size_t i = 0; i < n->u.fncall.nargs; ++i) mkr_apply_peephole(n->u.fncall.args[i]);
    break;
  case MKR_NK_UNARY:
    mkr_apply_peephole(n->u.unary.expr);
    break;
  case MKR_NK_BINOP:
    mkr_apply_peephole(n->u.binop.lhs);
    mkr_apply_peephole(n->u.binop.rhs);
    break;
  case MKR_NK_PATH:
    fuse_descendant_or_self_steps(n->u.path.steps, &n->u.path.nsteps);
    for (size_t i = 0; i < n->u.path.nsteps; ++i) {
      for (size_t j = 0; j < n->u.path.steps[i].npredicates; ++j) {
        mkr_apply_peephole(n->u.path.steps[i].predicates[j]);
      }
    }
    break;
  case MKR_NK_FILTER:
    mkr_apply_peephole(n->u.filter.expr);
    for (size_t i = 0; i < n->u.filter.npreds; ++i) mkr_apply_peephole(n->u.filter.preds[i]);
    fuse_descendant_or_self_steps(n->u.filter.path_steps, &n->u.filter.npath);
    for (size_t i = 0; i < n->u.filter.npath; ++i) {
      for (size_t j = 0; j < n->u.filter.path_steps[i].npredicates; ++j) {
        mkr_apply_peephole(n->u.filter.path_steps[i].predicates[j]);
      }
    }
    break;
  default:
    break;
  }
}

void
mkr_node_clear_memos(mkr_node_t *n)
{
  if (n == NULL) return;
  if (n->memoized) {
    mkr_val_clear(&n->memo_value);
    n->memoized = 0;
  }
  switch (n->kind) {
  case MKR_NK_FNCALL:
    for (size_t i = 0; i < n->u.fncall.nargs; ++i) mkr_node_clear_memos(n->u.fncall.args[i]);
    break;
  case MKR_NK_UNARY:
    mkr_node_clear_memos(n->u.unary.expr);
    break;
  case MKR_NK_BINOP:
    mkr_node_clear_memos(n->u.binop.lhs);
    mkr_node_clear_memos(n->u.binop.rhs);
    break;
  case MKR_NK_PATH:
    for (size_t i = 0; i < n->u.path.nsteps; ++i) clear_memos_step(&n->u.path.steps[i]);
    break;
  case MKR_NK_FILTER:
    mkr_node_clear_memos(n->u.filter.expr);
    for (size_t i = 0; i < n->u.filter.npreds; ++i) mkr_node_clear_memos(n->u.filter.preds[i]);
    for (size_t i = 0; i < n->u.filter.npath; ++i)  clear_memos_step(&n->u.filter.path_steps[i]);
    break;
  default:
    break;
  }
}

void
mkr_node_free(mkr_node_t *n)
{
  if (n == NULL) return;
  /* Free any memoized value first (idempotent). */
  if (n->memoized) {
    mkr_val_clear(&n->memo_value);
    n->memoized = 0;
  }
  switch (n->kind) {
  case MKR_NK_LITERAL_STR:
    mkr_owned_text_clear(&n->u.literal);
    break;
  case MKR_NK_LITERAL_NUM:
    break;
  case MKR_NK_VARREF:
    mkr_owned_text_clear(&n->u.varref.prefix);
    mkr_owned_text_clear(&n->u.varref.name);
    break;
  case MKR_NK_FNCALL:
    mkr_owned_text_clear(&n->u.fncall.prefix);
    mkr_owned_text_clear(&n->u.fncall.name);
    for (size_t i = 0; i < n->u.fncall.nargs; ++i) {
      mkr_node_free(n->u.fncall.args[i]);
    }
    free(n->u.fncall.args);
    break;
  case MKR_NK_UNARY:
    mkr_node_free(n->u.unary.expr);
    break;
  case MKR_NK_BINOP:
    mkr_node_free(n->u.binop.lhs);
    mkr_node_free(n->u.binop.rhs);
    break;
  case MKR_NK_PATH:
    for (size_t i = 0; i < n->u.path.nsteps; ++i) {
      mkr_step_clear(&n->u.path.steps[i]);
    }
    free(n->u.path.steps);
    break;
  case MKR_NK_FILTER:
    mkr_node_free(n->u.filter.expr);
    for (size_t i = 0; i < n->u.filter.npreds; ++i) {
      mkr_node_free(n->u.filter.preds[i]);
    }
    free(n->u.filter.preds);
    for (size_t i = 0; i < n->u.filter.npath; ++i) {
      mkr_step_clear(&n->u.filter.path_steps[i]);
    }
    free(n->u.filter.path_steps);
    break;
  }
  free(n);
}
