#include "mkr_xpath_internal.h"
#include "../core/mkr_safe.h"

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
  if (ns->count == ns->capacity) {
    size_t newcap = ns->capacity ? ns->capacity * 2 : 8;
    lxb_dom_node_t **p = realloc(ns->items, newcap * sizeof(*p));
    if (p == NULL) {
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory growing node-set");
      return -1;
    }
    ns->items    = p;
    ns->capacity = newcap;
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
    free(v->u.string);
    v->u.string = NULL;
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
    const char *s = src->u.string ? src->u.string : "";
    char *p = strdup(s);
    if (p == NULL) {
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory cloning string value");
      return -1;
    }
    dst->u.string = p;
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
    lxb_dom_node_t **items = malloc(n * sizeof(*items));
    if (items == NULL) {
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory cloning node-set");
      return -1;
    }
    memcpy(items, src->u.nodeset.items, n * sizeof(*items));
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
  lxb_dom_node_t *cur = node->first_child;
  while (cur != NULL) {
    if (cur->type == LXB_DOM_NODE_TYPE_TEXT) {
      mkr_status_t st = append_text_content(cur, buf);
      if (st != MKR_OK) return st; /* LIMIT or OOM — caller fails closed */
    }
    if (cur->type == LXB_DOM_NODE_TYPE_ELEMENT && cur->first_child != NULL) {
      cur = cur->first_child;
      continue;
    }
    while (cur != node && cur->next == NULL) {
      cur = cur->parent;
    }
    if (cur == node) return MKR_OK;
    cur = cur->next;
  }
  return MKR_OK;
}

/* Build node's string-value into `buf` (cap carried by buf->max). */
static mkr_status_t
build_string_value(const lxb_dom_node_t *node, mkr_buf_t *buf)
{
  if (node == NULL) return MKR_OK;

  switch (node->type) {
  case LXB_DOM_NODE_TYPE_ATTRIBUTE: {
    lxb_dom_attr_t *attr = (lxb_dom_attr_t *)node;
    size_t vlen = 0;
    const lxb_char_t *v = lxb_dom_attr_value(attr, &vlen);
    return mkr_buf_append(buf, v ? (const char *)v : "", vlen);
  }
  case LXB_DOM_NODE_TYPE_TEXT:
  case LXB_DOM_NODE_TYPE_CDATA_SECTION:
  case LXB_DOM_NODE_TYPE_COMMENT:
  case LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION:
    return append_text_content((lxb_dom_node_t *)node, buf);
  default:
    return append_text_descendants((lxb_dom_node_t *)node, buf);
  }
}

char *
mkr_build_node_string_value_unchecked(const lxb_dom_node_t *node)
{
  /* Uncapped, best-effort: callers (number/string coercion) require a non-NULL
   * C string, so on any failure fall back to an owned "" rather than NULL. */
  mkr_buf_t buf;
  mkr_buf_init(&buf, 0);
  if (build_string_value(node, &buf) != MKR_OK) {
    mkr_buf_free(&buf);
    return strdup("");
  }
  char *s = mkr_buf_steal(&buf, NULL);
  return s != NULL ? s : strdup("");
}

char *
mkr_node_string_value_or_fail(const lxb_dom_node_t *node,
                             mkr_xpath_limits_t *limits,
                             mkr_xpath_error_t *err)
{
  mkr_buf_t buf;
  mkr_buf_init(&buf, (limits != NULL) ? limits->max_string_bytes : 0);
  mkr_status_t st = build_string_value(node, &buf);
  if (st == MKR_ERR_LIMIT) {
    mkr_buf_free(&buf);
    mkr_err_setf(err, MKR_XPATH_ERR_LIMIT,
                "string size limit exceeded (%zu bytes) while building node string-value",
                limits->max_string_bytes);
    return NULL;
  }
  if (st != MKR_OK) {
    mkr_buf_free(&buf);
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory building node string-value");
    return NULL;
  }
  char *s = mkr_buf_steal(&buf, NULL);
  if (s == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory building node string-value");
    return NULL;
  }
  return s;
}

char *
mkr_val_to_string_or_fail(const mkr_val_t *v,
                         mkr_xpath_limits_t *limits,
                         mkr_xpath_error_t *err)
{
  if (v == NULL) {
    char *e = strdup("");
    if (e == NULL) mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory converting value to string");
    return e;
  }
  switch (v->type) {
  case MKR_XPATH_TYPE_STRING: {
    const char *s = v->u.string ? v->u.string : "";
    if (limits != NULL && mkr_limit_check_string_bytes(limits, strlen(s), err) != 0) return NULL;
    char *p = strdup(s);
    if (p == NULL) {
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory copying string value");
      return NULL;
    }
    return p;
  }
  case MKR_XPATH_TYPE_BOOLEAN: {
    char *p = strdup(v->u.boolean ? "true" : "false");
    if (p == NULL) mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory converting boolean to string");
    return p;
  }
  case MKR_XPATH_TYPE_NUMBER: {
    char *p = mkr_val_to_string_unchecked(v);
    if (p == NULL) {
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory converting number to string");
      return NULL;
    }
    return p;
  }
  case MKR_XPATH_TYPE_NODESET:
    if (v->u.nodeset.count == 0) {
      char *p = strdup("");
      if (p == NULL) mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory");
      return p;
    }
    /* XPath 1.0 §4.2: string(node-set) = string-value of first node in doc order. */
    return mkr_node_string_value_or_fail(v->u.nodeset.items[0], limits, err);
  }
  mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "unknown value type");
  return NULL;
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
    char *s = mkr_node_string_value_or_fail(v->u.nodeset.items[0], limits, err);
    if (s == NULL) return -1;
    mkr_val_t tmp = { .type = MKR_XPATH_TYPE_STRING, .u = { .string = s } };
    *out = mkr_val_to_number_unchecked(&tmp);
    free(s);
    return 0;
  }
  *out = mkr_val_to_number_unchecked(v);
  return 0;
}

/* ---------- coercions ---------- */

double
mkr_val_to_number_unchecked(const mkr_val_t *v)
{
  switch (v->type) {
  case MKR_XPATH_TYPE_NUMBER:
    return v->u.number;
  case MKR_XPATH_TYPE_BOOLEAN:
    return v->u.boolean ? 1.0 : 0.0;
  case MKR_XPATH_TYPE_STRING: {
    if (v->u.string == NULL) return (double)NAN;
    const char *s = v->u.string;
    while (*s && isspace((unsigned char)*s)) s++;
    if (*s == '\0') return (double)NAN;
    char *end = NULL;
    double d = strtod(s, &end);
    if (end == s) return (double)NAN;
    while (*end && isspace((unsigned char)*end)) end++;
    if (*end != '\0') return (double)NAN;
    return d;
  }
  case MKR_XPATH_TYPE_NODESET: {
    if (v->u.nodeset.count == 0) return (double)NAN;
    /* string-value of first node in document order */
    char *s = mkr_build_node_string_value_unchecked(v->u.nodeset.items[0]);
    mkr_val_t tmp = { .type = MKR_XPATH_TYPE_STRING, .u = { .string = s } };
    double d = mkr_val_to_number_unchecked(&tmp);
    free(s);
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
    return v->u.string != NULL && v->u.string[0] != '\0';
  case MKR_XPATH_TYPE_NODESET:
    return v->u.nodeset.count > 0;
  }
  return 0;
}

char *
mkr_val_to_string_unchecked(const mkr_val_t *v)
{
  switch (v->type) {
  case MKR_XPATH_TYPE_STRING:
    return strdup(v->u.string ? v->u.string : "");
  case MKR_XPATH_TYPE_BOOLEAN:
    return strdup(v->u.boolean ? "true" : "false");
  case MKR_XPATH_TYPE_NUMBER: {
    double d = v->u.number;
    if (isnan(d)) return strdup("NaN");
    if (isinf(d)) return strdup(d < 0 ? "-Infinity" : "Infinity");
    if (d == 0.0) return strdup("0");
    /* XPath number → string: integer values render without trailing zeros. */
    char buf[64];
    if (d == floor(d) && fabs(d) < 1e15) {
      snprintf(buf, sizeof(buf), "%lld", (long long)d);
    } else {
      snprintf(buf, sizeof(buf), "%.15g", d);
    }
    return strdup(buf);
  }
  case MKR_XPATH_TYPE_NODESET:
    if (v->u.nodeset.count == 0) return strdup("");
    return mkr_build_node_string_value_unchecked(v->u.nodeset.items[0]);
  }
  return strdup("");
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
  if (n->type == LXB_DOM_NODE_TYPE_ATTRIBUTE) {
    return n->parent ? n->parent : n;
  }
  return n;
}

static int
depth_of(const lxb_dom_node_t *n)
{
  int d = 0;
  while (n->parent) { d++; n = n->parent; }
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
    int a_attr = (a->type == LXB_DOM_NODE_TYPE_ATTRIBUTE);
    int b_attr = (b->type == LXB_DOM_NODE_TYPE_ATTRIBUTE);
    if (a_attr && !b_attr) return 1;  /* b is the owner element E; a (its attr) follows */
    if (b_attr && !a_attr) return -1; /* a is the owner element E; b (its attr) follows */
    /* Both attributes of the same element: relative order is
     * implementation-defined. Use insertion order via attr linked list. */
    if (a_attr && b_attr) {
      for (const lxb_dom_attr_t *at = ((const lxb_dom_element_t *)aa)->first_attr;
           at != NULL; at = at->next) {
        if ((const lxb_dom_node_t *)at == a) return -1;
        if ((const lxb_dom_node_t *)at == b) return 1;
      }
      return 0;
    }
    /* aa == bb but neither is an attribute means a == b, handled above. */
    return 0;
  }

  int da = depth_of(aa), db = depth_of(bb);
  while (da > db) { aa = aa->parent; da--; }
  while (db > da) { bb = bb->parent; db--; }
  if (aa == bb) {
    /* One is ancestor of the other; ancestor comes first. */
    return (aa == anchor_for_cmp(a)) ? -1 : 1;
  }
  while (aa->parent != bb->parent) {
    aa = aa->parent;
    bb = bb->parent;
  }
  /* Walk sibling chain from common parent. */
  const lxb_dom_node_t *parent = aa->parent;
  if (parent == NULL) {
    /* Different documents/roots — undefined; keep stable. */
    return 0;
  }
  for (const lxb_dom_node_t *s = parent->first_child; s != NULL; s = s->next) {
    if (s == aa) return -1;
    if (s == bb) return 1;
  }
  return 0;
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
order_index_insert(mkr_doc_order_index_t *idx, const lxb_dom_node_t *node, uint32_t ord)
{
  if (idx->cap == 0 || idx->count * 4 >= idx->cap * 3) {
    size_t new_cap = idx->cap ? idx->cap * 2 : 256;
    void *new_buckets = calloc(new_cap, sizeof(*idx->buckets));
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
                   uint32_t *out_ord)
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
order_index_walk(mkr_doc_order_index_t *idx, lxb_dom_node_t *root, uint32_t *next_ord)
{
  lxb_dom_node_t *cur = root;
  while (cur != NULL) {
    /* Visit (pre-order): the node, then its attributes before any child. */
    if (order_index_insert(idx, cur, (*next_ord)++) != 0) return -1;
    if (cur->type == LXB_DOM_NODE_TYPE_ELEMENT) {
      lxb_dom_element_t *el = (lxb_dom_element_t *)cur;
      for (lxb_dom_attr_t *a = el->first_attr; a != NULL; a = a->next) {
        if (order_index_insert(idx, (lxb_dom_node_t *)a, (*next_ord)++) != 0) return -1;
      }
    }
    if (cur->first_child != NULL) {
      cur = cur->first_child;
      continue;
    }
    while (cur != root && cur->next == NULL) {
      cur = cur->parent;
    }
    if (cur == root) break;
    cur = cur->next;
  }
  return 0;
}

static int
order_index_build(mkr_doc_order_index_t *idx, lxb_dom_node_t *root,
                  mkr_xpath_error_t *err)
{
  if (idx->built) return 0;
  if (root == NULL) return -1;
  uint32_t next_ord = 0;
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
  uint32_t oa, ob;
  if (order_index_lookup(idx, a, &oa) != 0) return doc_order_cmp(a, b);
  if (order_index_lookup(idx, b, &ob) != 0) return doc_order_cmp(a, b);
  /* Safe comparison — no subtraction (would overflow at 2^31). */
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

  lxb_dom_node_t **tmp = malloc(ns->count * sizeof(*tmp));
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
  c->buckets[j] = (uint32_t)(idx + 1);
}

/* Rebuild the index from entries[0, count). Returns -1 on OOM. */
static int
mkr_str_cache_reindex(mkr_str_cache_t *c, size_t bucket_cap)
{
  uint32_t *buckets = calloc(bucket_cap, sizeof(*buckets));
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
      memset(c->buckets, 0, c->bucket_cap * sizeof(*c->buckets));
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
mkr_get_cached_node_string(mkr_xpath_context_t *ctx,
                          lxb_dom_node_t *node,
                          const char **out_str,
                          size_t *out_len,
                          mkr_xpath_error_t *err)
{
  /* Contract: ctx is non-NULL when called from the evaluator (the only
   * intended caller). A NULL ctx is a programming error; surface it. */
  mkr_str_cache_t *c = mkr_ctx_str_cache(ctx);
  if (c == NULL) {
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL,
               "mkr_get_cached_node_string called without a context");
    return -1;
  }

  /* O(1) lookup via the pointer-keyed index. */
  if (c->bucket_cap != 0) {
    size_t mask = c->bucket_cap - 1;
    size_t j = pointer_hash(node) & mask;
    while (c->buckets[j] != 0) {
      mkr_str_cache_entry_t *e = &c->entries[c->buckets[j] - 1];
      if (e->node == node) {
        *out_str = e->str;
        if (out_len) *out_len = e->len;
        return 0;
      }
      j = (j + 1) & mask;
    }
  }

  char *s = mkr_node_string_value_or_fail(node, mkr_ctx_limits(ctx), err);
  if (s == NULL) return -1;
  size_t len = strlen(s);

  if (c->count == c->cap) {
    size_t new_cap = c->cap ? c->cap * 2 : 16;
    mkr_str_cache_entry_t *p = realloc(c->entries, new_cap * sizeof(*p));
    if (p == NULL) {
      free(s);
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory in node string cache");
      return -1;
    }
    c->entries = p;
    c->cap     = new_cap;
  }
  c->entries[c->count].node = node;
  c->entries[c->count].str  = s;
  c->entries[c->count].len  = len;

  /* Grow / build the index, keeping load factor <= 1/2. */
  if (c->bucket_cap == 0 || (c->count + 1) * 2 > c->bucket_cap) {
    size_t new_bucket_cap = c->bucket_cap ? c->bucket_cap * 2 : 64;
    if (mkr_str_cache_reindex(c, new_bucket_cap) != 0) {
      free(s);
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory indexing node string cache");
      return -1;
    }
  }
  mkr_str_cache_index_put(c, c->count);
  c->count++;

  *out_str = s;
  if (out_len) *out_len = len;
  return 0;
}

/* ---------- AST destructors ---------- */

void
mkr_step_clear(mkr_step_t *s)
{
  if (s == NULL) return;
  free(s->test.prefix);
  free(s->test.local);
  free(s->test.pi_target);
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
    if (n->u.fncall.prefix != NULL) {
      ci = 0; /* Handler-routed or namespaced builtins → non-CI. */
      break;
    }
    if (!is_pure_builtin_name(n->u.fncall.name, n->u.fncall.nargs)) {
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
        && steps[r].test.prefix == NULL
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
    free(n->u.literal_str);
    break;
  case MKR_NK_LITERAL_NUM:
    break;
  case MKR_NK_VARREF:
    free(n->u.varref.prefix);
    free(n->u.varref.name);
    break;
  case MKR_NK_FNCALL:
    free(n->u.fncall.prefix);
    free(n->u.fncall.name);
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
