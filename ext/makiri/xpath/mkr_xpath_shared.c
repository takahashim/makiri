/* mkr_xpath_shared.c - the representation-independent engine primitives.
 *
 * Compiled exactly ONCE (one normal .c, not a monomorphized body): none of
 * these functions dereferences a DOM node. They move node *pointers* (node-set
 * build/clone/free), own/compare engine strings, manage the per-eval
 * string-value cache and document-order-index lifecycles, and walk/destroy the
 * AST. A pointer is a pointer whichever representation it points at, so the
 * machine code is identical for HTML and XML -- one shared copy, not two.
 *
 * Contrast the engine bodies (mkr_xpath_{value,funcs,eval}_body.h): those are
 * .h files precisely because they ARE compiled twice (mkr_xpath_engine_html.c /
 * _xml.c include them with MKR_NODE_* bound to each representation). This file
 * is compiled once, so its code lives directly in the .c -- there is nothing to
 * include twice. mkr_xpath_internal.h is included WITHOUT a prelude, so
 * MKR_DOM_NODE stays the neutral `void` default; the node pointers below are
 * never dereferenced, so void* is exact.
 *
 * The driver (mkr_xpath.c), the parser/lexer, AND both engine instances call
 * these by their bare names. Two are extern rather than file-static
 * (mkr_str_cache_index_put, mkr_str_cache_reindex): the string-value cache
 * splits its pure index bookkeeping (here) from its node-dereferencing insert
 * (mkr_get_cached_node_text, in the per-instance value body), so both sides
 * share the one index implementation. Pointer hashing is mkr_ptr_hash
 * (core/mkr_hash.h) - the single pointer hash for every pointer-keyed index.
 */
#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* Pointer hashing for the str-cache + doc-order index is shared: mkr_ptr_hash
 * (core/mkr_hash.h), the one pointer hash for every pointer-keyed index. */

/* ---------- node-set ---------- */

void
mkr_nodeset_init(mkr_nodeset_t *ns)
{
  ns->items    = NULL;
  ns->count    = 0;
  ns->capacity = 0;
}

int
mkr_nodeset_push(mkr_nodeset_t *ns, MKR_DOM_NODE *node,
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

/* ---------- owned / borrowed text ---------- */

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

void
mkr_val_set_owned_text(mkr_val_t *v, mkr_owned_text_t text)
{
  if (v == NULL) return;
  v->type = MKR_XPATH_TYPE_STRING;
  v->u.string = text;
}

/* Set +v+ to a STRING by copying a borrowed view: the engine allocates and owns
 * the copy. This is how callers outside the engine (the glue handler bridge)
 * hand a string into a value - they pass what they have, a borrowed slice, and
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

/* ---------- per-evaluate document-order index (lifecycle) ---------- */

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

/* ---------- per-evaluation string-value cache (lifecycle + index) ---------- */

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
 * index must have room (callers grow/rehash first). Extern: shared by the pure
 * reindex below and the node-dereferencing insert in the per-instance body. */
void
mkr_str_cache_index_put(mkr_str_cache_t *c, size_t idx)
{
  size_t mask = c->bucket_cap - 1;
  size_t j = mkr_ptr_hash(c->entries[idx].node) & mask;
  while (c->buckets[j] != 0) {
    j = (j + 1) & mask;
  }
  c->buckets[j] = idx + 1;
}

/* Rebuild the index from entries[0, count). Returns -1 on OOM. Extern: see above. */
int
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
  /* 0-arg only - these read no input. */
  if (nargs == 0) {
    return strcmp(name, "true") == 0 || strcmp(name, "false") == 0;
  }
  /* n-arg pure functions - all args must themselves be CI (checked
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
     * not leak - recurse so any pure sub-expressions still get marks. */
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
 * first step's predicate list - only the child step's.
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
