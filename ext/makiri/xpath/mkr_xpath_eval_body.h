#include "mkr_xpath_internal.h"
#include "../core/mkr_core.h"

#include <lexbor/dom/dom.h>
#include <lexbor/ns/ns.h>
#include <lexbor/tag/tag.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

/*
 * XPath 1.0 evaluator. Walks the AST against a Lexbor DOM via the
 * native context. Phase 1 implements: child / attribute / self /
 * descendant-or-self / parent axes; predicates with position; the
 * full set of XPath 1.0 binary operators (semantically); arithmetic;
 * and a small built-in function table (see mkr_xpath_funcs.c).
 */

/* ---------- forward decls ---------- */

static int eval_node(mkr_xpath_context_t *ctx, const mkr_node_t *n,
                     MKR_DOM_NODE *self_node, size_t self_pos, size_t self_size,
                     mkr_val_t *out, mkr_xpath_error_t *err);

/* ---------- node test ---------- */


/* Resolve a node's namespace URI string. Returns "" for nodes with no
 * explicit namespace (LXB_NS__UNDEF). For other ids it looks up the URI
 * in the document's ns hash. */
static mkr_borrowed_text_t
node_ns_text(MKR_DOM_NODE *node, MKR_DOM_DOCUMENT *doc)
{
  size_t len = 0;
  const char *uri = MKR_NODE_NS_URI(node, doc, &len);
  return uri ? mkr_borrowed_text(uri, len) : mkr_borrowed_text_lit("");
}

/* Host-specific element/attribute name-test match (§8.6). The principal-node-type
 * filter has already passed (node is an element, or an attribute on the attribute
 * axis); this decides whether its name and namespace match +test+.
 *
 * HTML uses the qualified-name model: an unprefixed test compares the node's
 * qualified name (== local name for HTML, which has no element prefixes) and, in
 * strict mode, restricts elements to the HTML/no namespace; a prefixed test
 * compares the local name plus the namespace URI bound to the prefix. The XML
 * instance defines MKR_HOST_XML and provides its own local-name + ns_uri match. */
static int
name_test_match(const mkr_nodetest_t *test, MKR_DOM_NODE *node,
                mkr_axis_t axis, mkr_xpath_context_t *ctx)
{
#ifdef MKR_HOST_XML
  /* XML name match (local name + namespace URI). An unprefixed test matches a
   * no-namespace node only (a prefixed or default-namespaced element needs a
   * registered prefix, §8.6); a prefixed test matches the local name plus the
   * URI bound to the prefix. There is no qualified-name buffer on the custom
   * node, so the comparison is always against the local name. */
  size_t got_len = 0;
  const char *got = (axis == MKR_AXIS_ATTRIBUTE)
                      ? MKR_ATTR_LOCAL_NAME(node, &got_len)
                      : MKR_ELEM_LOCAL_NAME(node, &got_len);
  if (got == NULL || test->local.ptr == NULL) return 0;
  if (!mkr_borrowed_text_eq(mkr_borrowed_text_from_owned(test->local),
                            mkr_borrowed_text(got, got_len))) return 0;

  size_t node_uri_len = 0;
  const char *node_uri = MKR_NODE_NS_URI(node, mkr_ctx_document(ctx), &node_uri_len);
  if (test->prefix.ptr != NULL) {
    size_t want_uri_len = 0;
    const char *want_uri = mkr_ctx_lookup_ns(ctx, test->prefix.ptr, test->prefix.len, &want_uri_len);
    if (want_uri == NULL) return 0;
    if (want_uri_len != node_uri_len
        || (want_uri_len && memcmp(want_uri, node_uri, want_uri_len) != 0)) return 0;
  } else if (node_uri_len != 0 && !mkr_ctx_unprefixed_lax(ctx)) {
    /* unprefixed test, strict: the node must be in no namespace */
    return 0;
  }
  return 1;
#else
  /* HTML name match (qualified-name model). */
  size_t got_len = 0;
  const lxb_char_t *got;
  if (test->prefix.ptr != NULL) {
    got = (axis == MKR_AXIS_ATTRIBUTE) ? MKR_ATTR_LOCAL_NAME(node, &got_len)
                                       : MKR_ELEM_LOCAL_NAME(node, &got_len);
  } else {
    got = (axis == MKR_AXIS_ATTRIBUTE) ? MKR_ATTR_QUALIFIED_NAME(node, &got_len)
                                       : MKR_ELEM_QUALIFIED_NAME(node, &got_len);
  }
  if (got == NULL || test->local.ptr == NULL) return 0;
  if (!mkr_borrowed_text_eq(mkr_borrowed_text_from_owned(test->local),
                            mkr_borrowed_text((const char *)got, got_len))) return 0;

  if (test->prefix.ptr != NULL) {
    size_t want_uri_len = 0;
    const char *want_uri = mkr_ctx_lookup_ns(ctx, test->prefix.ptr, test->prefix.len, &want_uri_len);
    if (want_uri == NULL) return 0; /* unknown prefix -> non-match (step driver reports) */
    mkr_borrowed_text_t node_uri = node_ns_text(node, mkr_ctx_document(ctx));
    if (want_uri_len != node_uri.len || memcmp(want_uri, node_uri.ptr, want_uri_len) != 0) return 0;
  } else if (!mkr_ctx_unprefixed_lax(ctx)
             && axis != MKR_AXIS_ATTRIBUTE
             && MKR_NODE_IS_FOREIGN_NS(node)) {
    /* strict mode: unprefixed ELEMENT tests resolve in the HTML namespace, so a
     * foreign (SVG/MathML) element needs a prefix. Attributes are exempt (an
     * unprefixed attribute test matches by no-namespace local name; the
     * qualified-name compare above already excludes prefixed foreign attrs). */
    return 0;
  }
  return 1;
#endif
}

static int
node_principal_match(const mkr_nodetest_t *test, MKR_DOM_NODE *node,
                     mkr_axis_t axis, mkr_xpath_context_t *ctx)
{
  switch (test->kind) {
  case MKR_NT_NODE:
    /* XPath 1.0 §5 data model: only element, attribute, text,
     * namespace, processing-instruction, comment, and the root are
     * model nodes. Lexbor additionally exposes DOCUMENT_TYPE, ENTITY,
     * ENTITY_REFERENCE, and NOTATION nodes — none belong in the
     * model, so node() must not match them. DocumentFragment IS
     * matched: it acts as the root for fragment-rooted contexts,
     * so '.' / 'self::node()' over a fragment has to see it. */
    switch (MKR_NODE_TYPE(node)) {
    case MKR_NTYPE_DOCUMENT_TYPE:
    case MKR_NTYPE_ENTITY:
    case MKR_NTYPE_ENTITY_REFERENCE:
    case MKR_NTYPE_NOTATION:
      return 0;
    default:
      return 1;
    }
  case MKR_NT_TEXT:
    return MKR_NODE_TYPE(node) == MKR_NTYPE_TEXT
        || MKR_NODE_TYPE(node) == MKR_NTYPE_CDATA_SECTION;
  case MKR_NT_COMMENT:
    return MKR_NODE_TYPE(node) == MKR_NTYPE_COMMENT;
  case MKR_NT_PI:
    if (MKR_NODE_TYPE(node) != MKR_NTYPE_PI) return 0;
    if (test->pi_target.ptr == NULL) return 1;
    {
      size_t nlen = 0;
      const lxb_char_t *nm = MKR_NODE_PI_NAME(node, &nlen);
      return mkr_borrowed_text_eq(mkr_borrowed_text_from_owned(test->pi_target),
                         mkr_borrowed_text((const char *)nm, nlen));
    }
  case MKR_NT_WILDCARD: {
    if (axis == MKR_AXIS_NAMESPACE)
      return 0; /* see internal.h §6 */
    /* Principal node type. */
    if (axis == MKR_AXIS_ATTRIBUTE) {
      if (MKR_NODE_TYPE(node) != MKR_NTYPE_ATTRIBUTE) return 0;
    } else if (MKR_NODE_TYPE(node) != MKR_NTYPE_ELEMENT) {
      return 0;
    }
    /* `*` matches any namespace; `prefix:*` matches only the namespace bound
     * to prefix (internal.h §5). Unknown prefix is reported up front by the
     * step driver; here it is a non-match. */
    if (test->prefix.ptr != NULL) {
      size_t want_uri_len = 0;
      const char *want_uri = mkr_ctx_lookup_ns(ctx, test->prefix.ptr, test->prefix.len, &want_uri_len);
      if (want_uri == NULL) return 0;
      mkr_borrowed_text_t node_uri = node_ns_text(node, mkr_ctx_document(ctx));
      if (want_uri_len != node_uri.len || memcmp(want_uri, node_uri.ptr, want_uri_len) != 0) return 0;
    }
    return 1;
  }
  case MKR_NT_NAME:
    /* Principal node type filter. */
    if (axis == MKR_AXIS_ATTRIBUTE) {
      if (MKR_NODE_TYPE(node) != MKR_NTYPE_ATTRIBUTE) return 0;
    } else if (MKR_NODE_TYPE(node) != MKR_NTYPE_ELEMENT) {
      return 0;
    }
    /* The local-name + namespace match for an element/attribute name test is
     * host-specific (HTML's qualified-name model vs XML's local+namespace-URI
     * model, §8.6), so it lives in name_test_match — defined per instance. */
    return name_test_match(test, node, axis, ctx);
  }
  return 0;
}

/* ---------- axis walkers ---------- */

static int
walk_axis(mkr_axis_t axis, MKR_DOM_NODE *context,
          int (*visit)(MKR_DOM_NODE *n, void *u), void *u)
{
  switch (axis) {
  case MKR_AXIS_SELF:
    if (visit(context, u)) return 1;
    return 0;
  case MKR_AXIS_PARENT: {
    MKR_DOM_NODE *p = MKR_NODE_PARENT(context);
    if (p && visit(p, u)) return 1;
    return 0;
  }
  case MKR_AXIS_CHILD:
    for (MKR_DOM_NODE *c = MKR_NODE_FIRST_CHILD(context); c != NULL; c = MKR_NODE_NEXT(c)) {
      if (visit(c, u)) return 1;
    }
    return 0;
  case MKR_AXIS_ATTRIBUTE: {
    if (MKR_NODE_TYPE(context) != MKR_NTYPE_ELEMENT) return 0;
    MKR_DOM_ELEMENT *el = (MKR_DOM_ELEMENT *)context;
    for (MKR_DOM_ATTR *a = MKR_ELEM_FIRST_ATTR(el); a != NULL; a = MKR_ATTR_NEXT(a)) {
      if (visit((MKR_DOM_NODE *)a, u)) return 1;
    }
    return 0;
  }
  case MKR_AXIS_DESCENDANT_OR_SELF: {
    /* DFS pre-order. */
    if (visit(context, u)) return 1;
    MKR_DOM_NODE *n = MKR_NODE_FIRST_CHILD(context);
    while (n != NULL && n != context) {
      if (visit(n, u)) return 1;
      if (MKR_NODE_FIRST_CHILD(n)) {
        n = MKR_NODE_FIRST_CHILD(n);
      } else {
        while (n != context && MKR_NODE_NEXT(n) == NULL) n = MKR_NODE_PARENT(n);
        if (n == context) break;
        n = MKR_NODE_NEXT(n);
      }
    }
    return 0;
  }
  case MKR_AXIS_DESCENDANT: {
    MKR_DOM_NODE *n = MKR_NODE_FIRST_CHILD(context);
    while (n != NULL && n != context) {
      if (visit(n, u)) return 1;
      if (MKR_NODE_FIRST_CHILD(n)) {
        n = MKR_NODE_FIRST_CHILD(n);
      } else {
        while (n != context && MKR_NODE_NEXT(n) == NULL) n = MKR_NODE_PARENT(n);
        if (n == context) break;
        n = MKR_NODE_NEXT(n);
      }
    }
    return 0;
  }
  case MKR_AXIS_ANCESTOR:
    for (MKR_DOM_NODE *p = MKR_NODE_PARENT(context); p != NULL; p = MKR_NODE_PARENT(p)) {
      if (visit(p, u)) return 1;
    }
    return 0;
  case MKR_AXIS_ANCESTOR_OR_SELF:
    for (MKR_DOM_NODE *p = context; p != NULL; p = MKR_NODE_PARENT(p)) {
      if (visit(p, u)) return 1;
    }
    return 0;
  case MKR_AXIS_FOLLOWING_SIBLING:
    for (MKR_DOM_NODE *s = MKR_NODE_NEXT(context); s != NULL; s = MKR_NODE_NEXT(s)) {
      if (visit(s, u)) return 1;
    }
    return 0;
  case MKR_AXIS_PRECEDING_SIBLING:
    for (MKR_DOM_NODE *s = MKR_NODE_PREV(context); s != NULL; s = MKR_NODE_PREV(s)) {
      if (visit(s, u)) return 1;
    }
    return 0;
  case MKR_AXIS_FOLLOWING: {
    /* Start at the next node in doc order after context's subtree. */
    MKR_DOM_NODE *cur = context;
    while (cur != NULL && MKR_NODE_NEXT(cur) == NULL) cur = MKR_NODE_PARENT(cur);
    if (cur == NULL) return 0;
    cur = MKR_NODE_NEXT(cur);
    while (cur != NULL) {
      if (visit(cur, u)) return 1;
      if (MKR_NODE_FIRST_CHILD(cur)) {
        cur = MKR_NODE_FIRST_CHILD(cur);
      } else {
        while (cur != NULL && MKR_NODE_NEXT(cur) == NULL) cur = MKR_NODE_PARENT(cur);
        if (cur != NULL) cur = MKR_NODE_NEXT(cur);
      }
    }
    return 0;
  }
  case MKR_AXIS_PRECEDING: {
    /* Walk backward in doc order, skipping ancestors of context.
     * Emission order is reverse-doc (closest preceding node first).
     *
     * When we climb to a parent, that parent may or may not be an
     * ancestor of context: it's an ancestor only when we're climbing
     * the chain from context itself, not when we're climbing back out
     * of a preceding sibling's subtree. */
    MKR_DOM_NODE *cur = context;
    while (cur != NULL) {
      if (MKR_NODE_PREV(cur)) {
        cur = MKR_NODE_PREV(cur);
        while (MKR_NODE_LAST_CHILD(cur)) cur = MKR_NODE_LAST_CHILD(cur);
        if (visit(cur, u)) return 1;
      } else {
        cur = MKR_NODE_PARENT(cur);
        if (cur == NULL) return 0;
        /* Is cur an ancestor of the original context? */
        int is_ancestor = 0;
        for (MKR_DOM_NODE *p = MKR_NODE_PARENT(context); p != NULL; p = MKR_NODE_PARENT(p)) {
          if (p == cur) { is_ancestor = 1; break; }
        }
        if (!is_ancestor) {
          if (visit(cur, u)) return 1;
        }
      }
    }
    return 0;
  }
  case MKR_AXIS_NAMESPACE:
    /* See mkr_xpath_internal.h §6: not implemented for HTML where
     * Lexbor doesn't expose DOM-level namespace nodes. The step
     * driver rejects this before reaching the walker. */
    return 0;
  default:
    return 0;
  }
}

/* ---------- predicates ---------- */

/*
 * Fast path for the two most common predicate shapes, [@name] and
 * [@name = 'literal']. The generic evaluator services these by building a
 * throwaway node-set (malloc/free) per context node for the attribute path plus
 * a string-cache insert; recognising the shape lets us filter with a direct
 * attribute lookup instead. Both shapes are boolean (no position dependence),
 * so this is a pure per-node filter — identical result to the generic path.
 */
typedef struct {
  const char *name;
  size_t      name_len;
  const char *value;
  size_t      value_len;
  int         has_value; /* 0 => [@name]; 1 => [@name = 'value'] */
} mkr_attr_pred_t;

/* Shape A: a relative path that is a single unprefixed attribute name test with
 * no predicates — i.e. `@name`. Fills name/name_len; returns 1 on match. */
static int
mkr_match_attr_step(const mkr_node_t *n, const char **name, size_t *name_len)
{
  if (n == NULL || n->kind != MKR_NK_PATH || n->u.path.absolute
      || n->u.path.nsteps != 1) {
    return 0;
  }
  const mkr_step_t *s = &n->u.path.steps[0];
  if (s->axis != MKR_AXIS_ATTRIBUTE || s->npredicates != 0
      || s->test.kind != MKR_NT_NAME || s->test.prefix.ptr != NULL
      || s->test.local.ptr == NULL) {
    return 0;
  }
  *name = s->test.local.ptr;
  *name_len = s->test.local.len;
  return 1;
}

/* Recognise [@name] or [@name='lit'] / ['lit'=@name]. Returns 1 + fills out. */
static int
mkr_match_attr_pred(const mkr_node_t *p, mkr_attr_pred_t *out)
{
  if (mkr_match_attr_step(p, &out->name, &out->name_len)) {
    out->has_value = 0;
    return 1;
  }
  if (p == NULL || p->kind != MKR_NK_BINOP || p->u.binop.op != MKR_OP_EQ) {
    return 0;
  }
  const mkr_node_t *lhs = p->u.binop.lhs;
  const mkr_node_t *rhs = p->u.binop.rhs;
  const mkr_node_t *attr_side, *lit_side;
  if (lhs != NULL && lhs->kind == MKR_NK_LITERAL_STR) {
    lit_side = lhs; attr_side = rhs;
  } else if (rhs != NULL && rhs->kind == MKR_NK_LITERAL_STR) {
    lit_side = rhs; attr_side = lhs;
  } else {
    return 0;
  }
  if (!mkr_match_attr_step(attr_side, &out->name, &out->name_len)) {
    return 0;
  }
  out->value     = lit_side->u.literal.ptr ? lit_side->u.literal.ptr : "";
  out->value_len = lit_side->u.literal.len;
  out->has_value = 1;
  return 1;
}

/* Find an element's attribute whose qualified name equals +name+ exactly
 * (case-SENSITIVE), or NULL. We scan rather than calling
 * lxb_dom_element_get_attribute, because that does an HTML case-INsensitive
 * lookup — which would make `[@Id]` match `id`, diverging from XPath 1.0, from
 * Nokogiri::HTML5, AND from Makiri's own case-sensitive attribute-axis name
 * test (node_principal_match compares the qualified name byte-for-byte). The
 * fast path only handles unprefixed names, matching that comparison. */
static MKR_DOM_ATTR *
mkr_attr_by_qualified_name(MKR_DOM_ELEMENT *el, const char *name, size_t name_len)
{
  for (MKR_DOM_ATTR *a = MKR_ELEM_FIRST_ATTR(el); a != NULL; a = MKR_ATTR_NEXT(a)) {
    size_t qlen = 0;
    const lxb_char_t *q = MKR_ATTR_QUALIFIED_NAME(a, &qlen);
    if (q != NULL && qlen == name_len && memcmp(q, name, name_len) == 0) {
      return a;
    }
  }
  return NULL;
}

/* Filter `inout` by a recognised attribute predicate into `kept`. */
static int
mkr_filter_attr_pred(mkr_xpath_context_t *ctx, const mkr_attr_pred_t *ap,
                     const mkr_nodeset_t *inout, mkr_nodeset_t *kept,
                     mkr_xpath_error_t *err)
{
  for (size_t i = 0; i < inout->count; ++i) {
    MKR_DOM_NODE *n = inout->items[i];
    int keep = 0;
    if (MKR_NODE_TYPE(n) == MKR_NTYPE_ELEMENT) {
      MKR_DOM_ATTR *a =
          mkr_attr_by_qualified_name(MKR_NODE_AS_ELEMENT(n), ap->name, ap->name_len);
      if (a != NULL) {
        if (!ap->has_value) {
          keep = 1;
        } else {
          size_t got_len = 0;
          const lxb_char_t *got = MKR_ATTR_VALUE(a, &got_len);
          if (got == NULL) {
            got_len = 0;
          }
          keep = (got_len == ap->value_len
                  && (ap->value_len == 0
                      || memcmp(got, ap->value, ap->value_len) == 0));
        }
      }
    }
    if (keep && mkr_nodeset_push(kept, n, mkr_ctx_limits(ctx), err) != 0) {
      return -1;
    }
  }
  return 0;
}

static int
apply_predicates(mkr_xpath_context_t *ctx,
                 mkr_node_t **preds, size_t npreds,
                 mkr_nodeset_t *inout,
                 mkr_xpath_error_t *err)
{
  for (size_t p = 0; p < npreds; ++p) {
    mkr_nodeset_t kept;
    mkr_nodeset_init(&kept);

    /* Specialise [@name] / [@name='lit'] — a position-independent filter, so
     * applying it per predicate (even amid others) matches the generic path. */
    mkr_attr_pred_t ap;
    if (mkr_match_attr_pred(preds[p], &ap)) {
      if (mkr_filter_attr_pred(ctx, &ap, inout, &kept, err) != 0) {
        mkr_nodeset_clear(&kept);
        return -1;
      }
      mkr_nodeset_clear(inout);
      *inout = kept;
      continue;
    }

    size_t size = inout->count;
    for (size_t i = 0; i < size; ++i) {
      mkr_val_t v = {0};
      if (eval_node(ctx, preds[p], inout->items[i], i + 1, size, &v, err) != 0) {
        mkr_nodeset_clear(&kept);
        return -1;
      }
      int keep;
      if (v.type == MKR_XPATH_TYPE_NUMBER) {
        double d = v.u.number;
        keep = (d == (double)(i + 1));
      } else {
        keep = mkr_val_to_boolean(&v);
      }
      mkr_val_clear(&v);
      if (keep) {
        if (mkr_nodeset_push(&kept, inout->items[i], mkr_ctx_limits(ctx), err) != 0) {
          mkr_nodeset_clear(&kept);
          return -1;
        }
      }
    }
    mkr_nodeset_clear(inout);
    *inout = kept;
  }
  return 0;
}

/* ---------- step ---------- */

typedef struct {
  mkr_nodeset_t       *out;
  const mkr_step_t    *step;
  mkr_xpath_context_t *ctx;
  mkr_xpath_error_t   *err;
  int                 aborted; /* set when push or match failed; the walk
                                * is short-circuited and the step driver
                                * propagates the error. */
} step_visit_t;

static int
step_visit_cb(MKR_DOM_NODE *n, void *u)
{
  step_visit_t *st = (step_visit_t *)u;
  if (node_principal_match(&st->step->test, n, st->step->axis, st->ctx)) {
    if (mkr_nodeset_push(st->out, n, mkr_ctx_limits(st->ctx), st->err) != 0) {
      st->aborted = 1;
      return 1; /* stop the walk; eval_step inspects st.aborted */
    }
  }
  return 0;
}

/*
 * Whether walking 'axis' from multiple distinct context nodes can yield
 * the same node more than once. For axes that always produce a node-set
 * unique to each starting node (when starting from distinct nodes), we
 * can skip the dedup pass entirely.
 *
 * - child, attribute, self: each result has a single parent / single
 *   anchor, so distinct contexts → distinct results.
 * - parent: multiple contexts can share the same parent.
 * - descendant / descendant-or-self: distinct contexts in
 *   ancestor-descendant relation overlap.
 * - ancestor / ancestor-or-self / following / preceding /
 *   following-sibling / preceding-sibling: can overlap.
 * - namespace: TODO Phase 2.
 */
static int
axis_can_alias(mkr_axis_t a)
{
  switch (a) {
  case MKR_AXIS_CHILD:
  case MKR_AXIS_ATTRIBUTE:
  case MKR_AXIS_SELF:
    return 0;
  default:
    return 1;
  }
}

/*
 * 13 axes — Phase 2b adds the remaining six. The namespace axis stays
 * unimplemented for now (spec section 6 in mkr_xpath_internal.h).
 */
static int
axis_is_implemented(mkr_axis_t a)
{
  switch (a) {
  case MKR_AXIS_CHILD:
  case MKR_AXIS_DESCENDANT:
  case MKR_AXIS_DESCENDANT_OR_SELF:
  case MKR_AXIS_PARENT:
  case MKR_AXIS_SELF:
  case MKR_AXIS_ATTRIBUTE:
  case MKR_AXIS_ANCESTOR:
  case MKR_AXIS_ANCESTOR_OR_SELF:
  case MKR_AXIS_FOLLOWING:
  case MKR_AXIS_PRECEDING:
  case MKR_AXIS_FOLLOWING_SIBLING:
  case MKR_AXIS_PRECEDING_SIBLING:
    return 1;
  case MKR_AXIS_NAMESPACE:
  default:
    return 0;
  }
}

static const char *
axis_name_str(mkr_axis_t a)
{
  switch (a) {
  case MKR_AXIS_ANCESTOR:           return "ancestor";
  case MKR_AXIS_ANCESTOR_OR_SELF:   return "ancestor-or-self";
  case MKR_AXIS_FOLLOWING:          return "following";
  case MKR_AXIS_PRECEDING:          return "preceding";
  case MKR_AXIS_FOLLOWING_SIBLING:  return "following-sibling";
  case MKR_AXIS_PRECEDING_SIBLING:  return "preceding-sibling";
  case MKR_AXIS_NAMESPACE:          return "namespace";
  default:                         return "axis";
  }
}

/*
 * "Reverse axis" in XPath 1.0 sense: walker emits nodes in
 * reverse-document order, and proximity position() counts from the
 * context node outward. The step driver applies predicates within the
 * axis-natural order (so [1] = closest), then sorts the merged result
 * into document order.
 */
static int
is_reverse_axis(mkr_axis_t a)
{
  switch (a) {
  case MKR_AXIS_ANCESTOR:
  case MKR_AXIS_ANCESTOR_OR_SELF:
  case MKR_AXIS_PRECEDING:
  case MKR_AXIS_PRECEDING_SIBLING:
    return 1;
  default:
    return 0;
  }
}

/*
 * Fast path for `//tag` — a descendant, unprefixed, predicate-free name-test
 * whose context is exactly the document node. `descendant::tag` from the
 * document is precisely "every element whose name is tag", which the
 * document's element index already groups by tag id in document order, so we
 * answer it from the index instead of walking the whole tree.
 *
 * Returns 1 if it filled +result+ (caller skips the walk), 0 if the shape or
 * context does not qualify (caller falls back to the walk), -1 on error.
 *
 * Correctness: only taken for pure-HTML documents (mkr_element_index_has_foreign
 * is false) — in foreign content an element's qualified name need not equal its
 * tag's canonical name, so a match could live in another tag bucket. Each
 * candidate is still re-checked with node_principal_match, so the result is
 * byte-identical to the walk (this also makes case/normalization quirks in the
 * tag-name lookup harmless: a non-matching candidate is simply dropped).
 */
static int
try_descendant_tag_index(mkr_xpath_context_t *ctx, const mkr_step_t *step,
                         const mkr_nodeset_t *context_set, mkr_nodeset_t *result,
                         mkr_xpath_error_t *err)
{
  if (step->axis != MKR_AXIS_DESCENDANT
      || step->test.kind != MKR_NT_NAME
      || step->test.prefix.ptr != NULL
      || step->test.local.ptr == NULL
      || context_set->count != 1
      || context_set->items[0] != (MKR_DOM_NODE *)mkr_ctx_document(ctx)) {
    return 0;
  }
  void *eidx = mkr_ctx_element_index(ctx);
  mkr_tag_index_lookup_t  lookup      = mkr_ctx_tag_lookup(ctx);
  mkr_tag_index_foreign_t has_foreign = mkr_ctx_tag_has_foreign(ctx);
  if (eidx == NULL || lookup == NULL || has_foreign == NULL || has_foreign(eidx)) {
    return 0;
  }
  MKR_DOM_DOCUMENT *doc = mkr_ctx_document(ctx);
  if (doc == NULL) {
    return 0;
  }
  lxb_tag_id_t tag = MKR_DOC_TAG_ID_BY_NAME(doc, step->test.local.ptr,
                                            step->test.local.len);
  /* The index covers only Lexbor's static tag-id range; a custom element's tag
   * id is a (huge) pointer value and is not indexed. For such a name, fall back
   * to the tree walk so those elements are still found. */
  if (tag == LXB_TAG__UNDEF || (uintptr_t)tag >= LXB_TAG__LAST_ENTRY) {
    return 0;
  }
  size_t cnt = 0;
  MKR_DOM_NODE *const *bucket = lookup(eidx, tag, &cnt);
  for (size_t i = 0; i < cnt; ++i) {
    MKR_DOM_NODE *n = bucket[i];
    if (node_principal_match(&step->test, n, step->axis, ctx)) {
      if (mkr_nodeset_push(result, n, mkr_ctx_limits(ctx), err) != 0) {
        return -1;
      }
    }
  }
  return 1;
}

/* ---------------------------------------------------------------------------
 * at_xpath() first-match short-circuit (Node#at_xpath / Node#at).
 *
 * Node#at_xpath wants only the first node in document order; today it builds the
 * whole node-set and takes [0]. For the common "find a descendant by name (plus
 * a simple attribute predicate)" shapes we can instead walk the subtree in
 * document order and stop at the first match — the XPath-side analogue of
 * at_css's lxb_selectors MATCH_FIRST.
 *
 * Recognised shapes (after the parser's // peephole, see mkr_apply_peephole):
 *     //X        .//X          -> PATH [ {descendant, X} ]
 *     //X[@a..]  .//X[@a..]    -> PATH [ {desc-or-self,node()}, {child, X, preds} ]
 *     descendant::X[@a..]      -> PATH [ {descendant, X, preds} ]
 * where X is an UNPREFIXED name / '*' / node-type test and every predicate is a
 * position-independent [@name] / [@name='lit'] (exactly mkr_match_attr_pred's
 * shape). Each denotes "the strict descendants of <start> matching the
 * test+predicates, in document order", so the first node the pre-order walk
 * reaches IS node-set[0] of the full evaluation — byte-identical, just without
 * building the rest. Anything else (positional predicates, functions/variables,
 * reverse axes, unions, prefixes, longer paths) returns 0 and the caller falls
 * back to the full evaluator.
 */
static int
mkr_first_recognise(const mkr_node_t *ast, const mkr_step_t **out_step)
{
  if (ast == NULL || ast->kind != MKR_NK_PATH) return 0;
  const mkr_step_t *steps = ast->u.path.steps;
  size_t nsteps = ast->u.path.nsteps;
  const mkr_step_t *nt;
  if (nsteps == 1 && steps[0].axis == MKR_AXIS_DESCENDANT) {
    nt = &steps[0];
  } else if (nsteps == 2
             && steps[0].axis == MKR_AXIS_DESCENDANT_OR_SELF
             && steps[0].test.kind == MKR_NT_NODE
             && steps[0].npredicates == 0
             && steps[1].axis == MKR_AXIS_CHILD) {
    nt = &steps[1];
  } else {
    return 0;
  }
  /* A prefixed name test needs the step driver's "unknown prefix -> RUNTIME
   * error" semantics, which this path does not reproduce; fall back. */
  if (nt->test.prefix.ptr != NULL) return 0;
  /* Every predicate must be a position-independent attribute predicate (no
   * position()/last()/numeric, no function/variable -> no handler involvement). */
  for (size_t p = 0; p < nt->npredicates; p++) {
    mkr_attr_pred_t ap;
    if (!mkr_match_attr_pred(nt->predicates[p], &ap)) return 0;
  }
  *out_step = nt;
  return 1;
}

/* Does +n+ satisfy every (already-recognised) attribute predicate of +step+?
 * Mirrors mkr_filter_attr_pred's per-node test so the result is identical. */
static int
mkr_first_node_ok(const mkr_step_t *step, MKR_DOM_NODE *n)
{
  for (size_t p = 0; p < step->npredicates; p++) {
    mkr_attr_pred_t ap;
    (void)mkr_match_attr_pred(step->predicates[p], &ap); /* recogniser ensured it matches */
    if (MKR_NODE_TYPE(n) != MKR_NTYPE_ELEMENT) return 0;
    MKR_DOM_ATTR *a =
        mkr_attr_by_qualified_name(MKR_NODE_AS_ELEMENT(n), ap.name, ap.name_len);
    if (a == NULL) return 0;
    if (ap.has_value) {
      size_t got_len = 0;
      const lxb_char_t *got = MKR_ATTR_VALUE(a, &got_len);
      if (got == NULL) got_len = 0;
      if (got_len != ap.value_len
          || (ap.value_len != 0 && memcmp(got, ap.value, ap.value_len) != 0)) {
        return 0;
      }
    }
  }
  return 1;
}

/* If +ast+ is a recognised at_xpath() first-match shape, walk <start>'s strict
 * descendants in document order and set *out_node to the first match (NULL if
 * none). Returns 1 when handled (caller skips the full evaluator), 0 when the
 * shape is not recognised. Never raises (the recognised shape cannot error). */
int
mkr_try_first_match(mkr_xpath_context_t *ctx, const mkr_node_t *ast,
                    MKR_DOM_NODE **out_node)
{
  const mkr_step_t *step;
  if (out_node == NULL || !mkr_first_recognise(ast, &step)) return 0;
  *out_node = NULL;

  MKR_DOM_NODE *start = ast->u.path.absolute
      ? (MKR_DOM_NODE *)mkr_ctx_document(ctx)
      : mkr_ctx_node(ctx);
  if (start == NULL) return 1; /* recognised; no context -> no match */

  /* Iterative pre-order (document order) over STRICT descendants of start. */
  for (MKR_DOM_NODE *n = MKR_NODE_FIRST_CHILD(start); n != NULL; ) {
    if (node_principal_match(&step->test, n, step->axis, ctx)
        && mkr_first_node_ok(step, n)) {
      *out_node = n;
      return 1;
    }
    if (MKR_NODE_FIRST_CHILD(n) != NULL) { n = MKR_NODE_FIRST_CHILD(n); continue; }
    while (n != start && MKR_NODE_NEXT(n) == NULL) n = MKR_NODE_PARENT(n);
    if (n == start) break;
    n = MKR_NODE_NEXT(n);
  }
  return 1; /* recognised; *out_node is the first match or NULL */
}

static int
eval_step(mkr_xpath_context_t *ctx, const mkr_step_t *step,
          mkr_nodeset_t *context_set, mkr_nodeset_t *out,
          mkr_xpath_error_t *err)
{
  if (!axis_is_implemented(step->axis)) {
    mkr_err_setf(err, MKR_XPATH_ERR_NOT_IMPLEMENTED,
                "native engine: axis '%s' not implemented yet",
                axis_name_str(step->axis));
    return -1;
  }
  /* Validate the namespace prefix once up front so we get a uniform
   * RUNTIME error instead of a silently empty match. Covers both `prefix:local`
   * (MKR_NT_NAME) and `prefix:*` (MKR_NT_WILDCARD). */
  if (step->test.prefix.ptr != NULL
      && mkr_ctx_lookup_ns(ctx, step->test.prefix.ptr, step->test.prefix.len, NULL) == NULL) {
    mkr_err_setf(err, MKR_XPATH_ERR_RUNTIME,
                "unknown namespace prefix '%s' in name test",
                step->test.prefix.ptr);
    return -1;
  }

  /* Post-pass = sort to doc order, then optional adjacent-dedup. We
   * need it when:
   *   1. The axis emits in reverse-doc per context (sort required to
   *      flip back to doc order; predicates were already evaluated
   *      against the axis-natural order).
   *   2. The axis aliases across contexts (sort then dedup).
   *   3. The axis does NOT aliase but multiple contexts can still
   *      produce results that interleave in doc order. child is the
   *      canonical case: html's children include head and body, and
   *      head's children include title — concatenated naively gives
   *      [head, body, title, ...] but doc order is [head, title, body,
   *      ...]. Only SELF and ATTRIBUTE are safe to concatenate without
   *      sorting (their outputs stay in doc order when inputs are). */
  const int need_post_pass = is_reverse_axis(step->axis)
                          || (axis_can_alias(step->axis) && context_set->count > 1)
                          || (context_set->count > 1
                              && step->axis != MKR_AXIS_SELF
                              && step->axis != MKR_AXIS_ATTRIBUTE);

  mkr_nodeset_t result;
  mkr_nodeset_init(&result);

  if (step->npredicates == 0) {
    /* `//tag` from the document: serve from the element index, skipping the
     * tree walk entirely. */
    int fp = try_descendant_tag_index(ctx, step, context_set, &result, err);
    if (fp < 0) {
      mkr_nodeset_clear(&result);
      return -1;
    }
    if (fp == 0) {
    /* No-predicate fast path: walk all contexts straight into the
     * result buffer, regardless of post_pass. Saves a per-context
     * fragment malloc/clear and the merge push that the predicate
     * path needs. Big win for //h3/parent::*, //h3/ancestor::div, and
     * other multi-context reverse-axis sweeps where every context
     * produces a single result. */
    step_visit_t st = { .out = &result, .step = step, .ctx = ctx, .err = err, .aborted = 0 };
    for (size_t ci = 0; ci < context_set->count; ++ci) {
      walk_axis(step->axis, context_set->items[ci], step_visit_cb, &st);
      if (st.aborted) {
        mkr_nodeset_clear(&result);
        return -1;
      }
    }
    }
  } else {
    /* Predicate path: position() / last() are per-context, so we have
     * to materialise each context's fragment before filtering. Reuse
     * a single fragment buffer across iterations — its items[] grows
     * to the largest single-context cardinality once instead of
     * malloc/freeing on every iteration. */
    mkr_nodeset_t fragment;
    mkr_nodeset_init(&fragment);
    for (size_t ci = 0; ci < context_set->count; ++ci) {
      fragment.count = 0;
      step_visit_t st = { .out = &fragment, .step = step, .ctx = ctx, .err = err, .aborted = 0 };
      walk_axis(step->axis, context_set->items[ci], step_visit_cb, &st);
      if (st.aborted) {
        mkr_nodeset_clear(&fragment);
        mkr_nodeset_clear(&result);
        return -1;
      }

      /* Predicates apply per-context with axis-natural position
       * numbering (XPath 1.0 §2.4). For reverse axes, fragment is in
       * reverse-doc order and [1] = closest to context, which is the
       * desired meaning. */
      if (apply_predicates(ctx, step->predicates, step->npredicates, &fragment, err) != 0) {
        mkr_nodeset_clear(&fragment);
        mkr_nodeset_clear(&result);
        return -1;
      }

      for (size_t i = 0; i < fragment.count; ++i) {
        if (mkr_nodeset_push(&result, fragment.items[i], mkr_ctx_limits(ctx), err) != 0) {
          mkr_nodeset_clear(&fragment);
          mkr_nodeset_clear(&result);
          return -1;
        }
      }
    }
    mkr_nodeset_clear(&fragment);
  }

  if (need_post_pass && result.count > 1) {
    /* Sort to document order, then adjacent-dedup. */
    mkr_nodeset_sort_doc_order(ctx, &result);
    size_t w = 1;
    for (size_t r = 1; r < result.count; ++r) {
      if (result.items[r] != result.items[r - 1]) {
        result.items[w++] = result.items[r];
      }
    }
    result.count = w;
  }
  *out = result;
  return 0;
}

static int
eval_steps(mkr_xpath_context_t *ctx, mkr_step_t *steps, size_t nsteps,
           mkr_nodeset_t *seed, mkr_val_t *out, mkr_xpath_error_t *err)
{
  mkr_nodeset_t current = *seed;  /* take ownership */
  memset(seed, 0, sizeof(*seed));
  for (size_t s = 0; s < nsteps; ++s) {
    mkr_nodeset_t next;
    mkr_nodeset_init(&next);
    if (eval_step(ctx, &steps[s], &current, &next, err) != 0) {
      mkr_nodeset_clear(&current);
      return -1;
    }
    mkr_nodeset_clear(&current);
    current = next;
  }
  out->type = MKR_XPATH_TYPE_NODESET;
  out->u.nodeset = current;
  return 0;
}

/* ---------- equality / relational ---------- */

/*
 * Both comparators return 0 on success and -1 on OOM/LIMIT (with err
 * populated). The boolean result is stored in *out_result. String
 * values for node-set comparisons go through mkr_get_cached_node_text so any
 * budget overrun is surfaced rather than silently truncating.
 */
static int
compare_eq(mkr_xpath_context_t *ctx, const mkr_val_t *l, const mkr_val_t *r,
           mkr_op_t op, int *out_result, mkr_xpath_error_t *err)
{
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  int want_eq = (op == MKR_OP_EQ);

  if (l->type == MKR_XPATH_TYPE_NODESET && r->type == MKR_XPATH_TYPE_NODESET) {
    /* All node string-values go through the per-eval cache. For an
     * M-by-N nodeset comparison this turns O(M*N) computations into
     * at most O(M+N). */
    (void)L;
    for (size_t i = 0; i < l->u.nodeset.count; ++i) {
      mkr_borrowed_text_t ls;
      if (mkr_get_cached_node_text(ctx, l->u.nodeset.items[i], &ls, err) != 0) {
        return -1;
      }
      for (size_t j = 0; j < r->u.nodeset.count; ++j) {
        mkr_borrowed_text_t rs;
        if (mkr_get_cached_node_text(ctx, r->u.nodeset.items[j], &rs, err) != 0) {
          return -1;
        }
        if (mkr_borrowed_text_eq(ls, rs) == want_eq) {
          *out_result = 1;
          return 0;
        }
      }
    }
    *out_result = 0;
    return 0;
  }
  if (l->type == MKR_XPATH_TYPE_NODESET || r->type == MKR_XPATH_TYPE_NODESET) {
    const mkr_val_t *ns = (l->type == MKR_XPATH_TYPE_NODESET) ? l : r;
    const mkr_val_t *sc = (l->type == MKR_XPATH_TYPE_NODESET) ? r : l;
    if (sc->type == MKR_XPATH_TYPE_NUMBER) {
      double target = sc->u.number;
      for (size_t i = 0; i < ns->u.nodeset.count; ++i) {
        mkr_borrowed_text_t s;
        if (mkr_get_cached_node_text(ctx, ns->u.nodeset.items[i], &s, err) != 0) {
          return -1;
        }
        double d = mkr_borrowed_text_to_number(s);
        if ((d == target) == want_eq) { *out_result = 1; return 0; }
      }
      *out_result = 0;
      return 0;
    }
    if (sc->type == MKR_XPATH_TYPE_BOOLEAN) {
      int b_ns = (ns->u.nodeset.count > 0);
      int b_sc = sc->u.boolean;
      int eq = (b_ns == b_sc);
      *out_result = want_eq ? eq : !eq;
      return 0;
    }
    mkr_owned_text_t target;
    if (mkr_val_to_owned_text_or_fail(sc, L, err, &target) != 0) return -1;
    for (size_t i = 0; i < ns->u.nodeset.count; ++i) {
      mkr_borrowed_text_t s;
      if (mkr_get_cached_node_text(ctx, ns->u.nodeset.items[i], &s, err) != 0) {
        mkr_owned_text_clear(&target); return -1;
      }
      if (mkr_borrowed_text_eq(s, mkr_borrowed_text_from_owned(target)) == want_eq) {
        mkr_owned_text_clear(&target); *out_result = 1; return 0;
      }
    }
    mkr_owned_text_clear(&target);
    *out_result = 0;
    return 0;
  }
  /* Scalar comparisons */
  if (l->type == MKR_XPATH_TYPE_BOOLEAN || r->type == MKR_XPATH_TYPE_BOOLEAN) {
    int eq = (mkr_val_to_boolean(l) == mkr_val_to_boolean(r));
    *out_result = want_eq ? eq : !eq;
    return 0;
  }
  if (l->type == MKR_XPATH_TYPE_NUMBER || r->type == MKR_XPATH_TYPE_NUMBER) {
    /* Both operands are non-nodeset (the nodeset case is handled
     * above); _unchecked is the right entry — no allocation possible. */
    double a = mkr_val_to_number_unchecked(l);
    double b = mkr_val_to_number_unchecked(r);
    int eq = (a == b);
    *out_result = want_eq ? eq : !eq;
    return 0;
  }
  mkr_owned_text_t ls, rs;
  if (mkr_val_to_owned_text_or_fail(l, L, err, &ls) != 0) return -1;
  if (mkr_val_to_owned_text_or_fail(r, L, err, &rs) != 0) { mkr_owned_text_clear(&ls); return -1; }
  int eq = mkr_borrowed_text_eq(mkr_borrowed_text_from_owned(ls), mkr_borrowed_text_from_owned(rs));
  mkr_owned_text_clear(&ls);
  mkr_owned_text_clear(&rs);
  *out_result = want_eq ? eq : !eq;
  return 0;
}

/* Apply a relational operator to two already-computed numbers. */
static int
rel_hit(mkr_op_t op, double a, double b)
{
  switch (op) {
  case MKR_OP_LT: return a < b;
  case MKR_OP_LE: return a <= b;
  case MKR_OP_GT: return a > b;
  case MKR_OP_GE: return a >= b;
  default:        return 0;
  }
}

static int
compare_rel(mkr_xpath_context_t *ctx, const mkr_val_t *l, const mkr_val_t *r,
            mkr_op_t op, int *out_result, mkr_xpath_error_t *err)
{
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);

  /* node-set vs node-set (XPath 1.0 §3.4): true iff SOME pair of nodes — one
   * from each set — satisfies the relation on their numeric string-values.
   * Must compare every pair, not just the first node of each side. */
  if (l->type == MKR_XPATH_TYPE_NODESET && r->type == MKR_XPATH_TYPE_NODESET) {
    (void)L;
    for (size_t i = 0; i < l->u.nodeset.count; ++i) {
      mkr_borrowed_text_t ls;
      if (mkr_get_cached_node_text(ctx, l->u.nodeset.items[i], &ls, err) != 0) {
        return -1;
      }
      double a = mkr_borrowed_text_to_number(ls);
      for (size_t j = 0; j < r->u.nodeset.count; ++j) {
        mkr_borrowed_text_t rs;
        if (mkr_get_cached_node_text(ctx, r->u.nodeset.items[j], &rs, err) != 0) {
          return -1;
        }
        if (rel_hit(op, a, mkr_borrowed_text_to_number(rs))) {
          *out_result = 1;
          return 0;
        }
      }
    }
    *out_result = 0;
    return 0;
  }

  if (l->type == MKR_XPATH_TYPE_NODESET || r->type == MKR_XPATH_TYPE_NODESET) {
    const mkr_val_t *ns = (l->type == MKR_XPATH_TYPE_NODESET) ? l : r;
    const mkr_val_t *sc = (l->type == MKR_XPATH_TYPE_NODESET) ? r : l;
    int swap = (l->type != MKR_XPATH_TYPE_NODESET);
    double scn;
    if (mkr_val_to_number_or_fail(sc, L, err, &scn) != 0) return -1;
    for (size_t i = 0; i < ns->u.nodeset.count; ++i) {
      mkr_borrowed_text_t s;
      if (mkr_get_cached_node_text(ctx, ns->u.nodeset.items[i], &s, err) != 0) {
        return -1;
      }
      double nv = mkr_borrowed_text_to_number(s);
      double a = swap ? scn : nv;
      double b = swap ? nv  : scn;
      if (rel_hit(op, a, b)) { *out_result = 1; return 0; }
    }
    *out_result = 0;
    return 0;
  }
  double a, b;
  if (mkr_val_to_number_or_fail(l, L, err, &a) != 0) return -1;
  if (mkr_val_to_number_or_fail(r, L, err, &b) != 0) return -1;
  switch (op) {
  case MKR_OP_LT: *out_result = (a < b);  break;
  case MKR_OP_LE: *out_result = (a <= b); break;
  case MKR_OP_GT: *out_result = (a > b);  break;
  case MKR_OP_GE: *out_result = (a >= b); break;
  default: *out_result = 0; break;
  }
  return 0;
}

/* ---------- union ---------- */

static int
union_nodeset(mkr_xpath_context_t *ctx, mkr_val_t *l, mkr_val_t *r, mkr_val_t *out,
              mkr_xpath_error_t *err)
{
  if (l->type != MKR_XPATH_TYPE_NODESET || r->type != MKR_XPATH_TYPE_NODESET) {
    mkr_err_set(err, MKR_XPATH_ERR_TYPE, "operands of '|' must be node-sets");
    return -1;
  }
  /* Push both sides without per-insert deduplication (the old contains()
   * loop was O(n^2)). Sort once at the end and collapse adjacent
   * duplicates — total cost is O(n log n) plus a single linear pass. */
  mkr_nodeset_t merged;
  mkr_nodeset_init(&merged);
  for (size_t i = 0; i < l->u.nodeset.count; ++i) {
    if (mkr_nodeset_push(&merged, l->u.nodeset.items[i], mkr_ctx_limits(ctx), err) != 0) {
      mkr_nodeset_clear(&merged);
      return -1;
    }
  }
  for (size_t i = 0; i < r->u.nodeset.count; ++i) {
    if (mkr_nodeset_push(&merged, r->u.nodeset.items[i], mkr_ctx_limits(ctx), err) != 0) {
      mkr_nodeset_clear(&merged);
      return -1;
    }
  }
  /* XPath 1.0 §3.3: the result of '|' is a node-set in document order,
   * which downstream string()/number()/positional predicates assume. */
  mkr_nodeset_unique_sorted(ctx, &merged);
  out->type = MKR_XPATH_TYPE_NODESET;
  out->u.nodeset = merged;
  return 0;
}

/* ---------- main eval ---------- */

static int
eval_path(mkr_xpath_context_t *ctx, const mkr_node_t *n,
          MKR_DOM_NODE *self_node,
          mkr_val_t *out, mkr_xpath_error_t *err)
{
  mkr_nodeset_t seed;
  mkr_nodeset_init(&seed);
  if (n->u.path.absolute) {
    MKR_DOM_NODE *root = (MKR_DOM_NODE *)mkr_ctx_document(ctx);
    if (root == NULL) {
      mkr_err_set(err, MKR_XPATH_ERR_RUNTIME, "absolute path with no document");
      return -1;
    }
    if (mkr_nodeset_push(&seed, root, mkr_ctx_limits(ctx), err) != 0) {
      mkr_nodeset_clear(&seed);
      return -1;
    }
  } else {
    if (mkr_nodeset_push(&seed, self_node, mkr_ctx_limits(ctx), err) != 0) {
      mkr_nodeset_clear(&seed);
      return -1;
    }
  }
  return eval_steps(ctx, n->u.path.steps, n->u.path.nsteps, &seed, out, err);
}

static int
eval_filter(mkr_xpath_context_t *ctx, const mkr_node_t *n,
            MKR_DOM_NODE *self_node, size_t self_pos, size_t self_size,
            mkr_val_t *out, mkr_xpath_error_t *err)
{
  mkr_val_t primary = {0};
  if (eval_node(ctx, n->u.filter.expr, self_node, self_pos, self_size, &primary, err) != 0) return -1;
  if (n->u.filter.npreds > 0) {
    if (primary.type != MKR_XPATH_TYPE_NODESET) {
      mkr_val_clear(&primary);
      mkr_err_set(err, MKR_XPATH_ERR_TYPE, "predicate applied to non-node-set");
      return -1;
    }
    if (apply_predicates(ctx, n->u.filter.preds, n->u.filter.npreds, &primary.u.nodeset, err) != 0) {
      mkr_val_clear(&primary);
      return -1;
    }
  }
  if (n->u.filter.npath > 0) {
    if (primary.type != MKR_XPATH_TYPE_NODESET) {
      mkr_val_clear(&primary);
      mkr_err_set(err, MKR_XPATH_ERR_TYPE, "path applied to non-node-set");
      return -1;
    }
    mkr_nodeset_t seed = primary.u.nodeset;
    memset(&primary.u.nodeset, 0, sizeof(primary.u.nodeset));
    int rc = eval_steps(ctx, n->u.filter.path_steps, n->u.filter.npath, &seed, out, err);
    return rc;
  }
  *out = primary;
  return 0;
}

/* ---------- function library bridge ---------- */

extern mkr_func_impl_t mkr_lookup_function(const char *ns_uri, const char *local_name);

static int
eval_fncall(mkr_xpath_context_t *ctx, const mkr_node_t *n,
            MKR_DOM_NODE *self_node, size_t self_pos, size_t self_size,
            mkr_val_t *out, mkr_xpath_error_t *err)
{
  const char *ns_uri = NULL;
  if (n->u.fncall.prefix.ptr) {
    ns_uri = mkr_ctx_lookup_ns(ctx, n->u.fncall.prefix.ptr, n->u.fncall.prefix.len, NULL);
    if (ns_uri == NULL) {
      mkr_err_setf(err, MKR_XPATH_ERR_RUNTIME, "unknown namespace prefix '%s'", n->u.fncall.prefix.ptr);
      return -1;
    }
  }
  mkr_func_impl_t fn = mkr_lookup_function(ns_uri, n->u.fncall.name.ptr);

  /* Evaluate arguments once and reuse for either path. */
  mkr_val_t *args = NULL;
  size_t    nargs = n->u.fncall.nargs;
  if (nargs > 0) {
    args = mkr_callocarray(nargs, sizeof(*args));
    if (args == NULL) {
      mkr_err_set(err, MKR_XPATH_ERR_OOM, "out of memory allocating function arguments");
      return -1;
    }
    for (size_t i = 0; i < nargs; ++i) {
      if (eval_node(ctx, n->u.fncall.args[i], self_node, self_pos, self_size, &args[i], err) != 0) {
        for (size_t j = 0; j <= i; ++j) mkr_val_clear(&args[j]);
        free(args);
        return -1;
      }
    }
  }

  int rc;
  if (fn != NULL) {
    rc = fn(ctx, self_node, self_pos, self_size, args, nargs, out, err);
  } else {
    /* No built-in match. Delegate to the per-call resolver (the Ruby
     * handler bridge installs one for the duration of evaluate()). */
    mkr_func_resolver_t resolver = mkr_ctx_func_resolver(ctx);
    int resolved = 1; /* not found by default */
    if (resolver != NULL) {
      resolved = resolver(mkr_xpath_get_user_data(ctx),
                          ctx, (void *)self_node, self_pos, self_size,
                          ns_uri, n->u.fncall.name.ptr,
                          (void *)args, nargs, (void *)out, err);
    }
    if (resolved > 0) {
      mkr_err_setf(err, MKR_XPATH_ERR_RUNTIME, "unknown function %s%s%s",
                  n->u.fncall.prefix.ptr ? n->u.fncall.prefix.ptr : "",
                  n->u.fncall.prefix.ptr ? ":" : "",
                  n->u.fncall.name.ptr);
      rc = -1;
    } else {
      rc = resolved; /* 0 = ok, -1 = function errored */
    }
  }

  for (size_t i = 0; i < nargs; ++i) mkr_val_clear(&args[i]);
  free(args);
  return rc;
}

static int
eval_binop(mkr_xpath_context_t *ctx, const mkr_node_t *n,
           MKR_DOM_NODE *self_node, size_t self_pos, size_t self_size,
           mkr_val_t *out, mkr_xpath_error_t *err)
{
  /* AND/OR short-circuit. */
  if (n->u.binop.op == MKR_OP_OR || n->u.binop.op == MKR_OP_AND) {
    mkr_val_t l = {0};
    if (eval_node(ctx, n->u.binop.lhs, self_node, self_pos, self_size, &l, err) != 0) return -1;
    int lb = mkr_val_to_boolean(&l);
    mkr_val_clear(&l);
    if (n->u.binop.op == MKR_OP_OR && lb) {
      out->type = MKR_XPATH_TYPE_BOOLEAN; out->u.boolean = 1; return 0;
    }
    if (n->u.binop.op == MKR_OP_AND && !lb) {
      out->type = MKR_XPATH_TYPE_BOOLEAN; out->u.boolean = 0; return 0;
    }
    mkr_val_t r = {0};
    if (eval_node(ctx, n->u.binop.rhs, self_node, self_pos, self_size, &r, err) != 0) return -1;
    int rb = mkr_val_to_boolean(&r);
    mkr_val_clear(&r);
    out->type = MKR_XPATH_TYPE_BOOLEAN; out->u.boolean = rb;
    return 0;
  }

  mkr_val_t l = {0}, r = {0};
  if (eval_node(ctx, n->u.binop.lhs, self_node, self_pos, self_size, &l, err) != 0) return -1;
  if (eval_node(ctx, n->u.binop.rhs, self_node, self_pos, self_size, &r, err) != 0) {
    mkr_val_clear(&l); return -1;
  }
  int rc = 0;
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  double a, b;
  switch (n->u.binop.op) {
  case MKR_OP_EQ: case MKR_OP_NE: {
    int br;
    rc = compare_eq(ctx, &l, &r, n->u.binop.op, &br, err);
    if (rc == 0) { out->type = MKR_XPATH_TYPE_BOOLEAN; out->u.boolean = br; }
    break;
  }
  case MKR_OP_LT: case MKR_OP_LE: case MKR_OP_GT: case MKR_OP_GE: {
    int br;
    rc = compare_rel(ctx, &l, &r, n->u.binop.op, &br, err);
    if (rc == 0) { out->type = MKR_XPATH_TYPE_BOOLEAN; out->u.boolean = br; }
    break;
  }
  case MKR_OP_ADD:
    if ((rc = mkr_val_to_number_or_fail(&l, L, err, &a)) != 0) break;
    if ((rc = mkr_val_to_number_or_fail(&r, L, err, &b)) != 0) break;
    out->type = MKR_XPATH_TYPE_NUMBER;
    out->u.number = a + b;
    break;
  case MKR_OP_SUB:
    if ((rc = mkr_val_to_number_or_fail(&l, L, err, &a)) != 0) break;
    if ((rc = mkr_val_to_number_or_fail(&r, L, err, &b)) != 0) break;
    out->type = MKR_XPATH_TYPE_NUMBER;
    out->u.number = a - b;
    break;
  case MKR_OP_MUL:
    if ((rc = mkr_val_to_number_or_fail(&l, L, err, &a)) != 0) break;
    if ((rc = mkr_val_to_number_or_fail(&r, L, err, &b)) != 0) break;
    out->type = MKR_XPATH_TYPE_NUMBER;
    out->u.number = a * b;
    break;
  case MKR_OP_DIV: {
    if ((rc = mkr_val_to_number_or_fail(&l, L, err, &a)) != 0) break;
    if ((rc = mkr_val_to_number_or_fail(&r, L, err, &b)) != 0) break;
    out->type = MKR_XPATH_TYPE_NUMBER;
    out->u.number = a / b;
    break;
  }
  case MKR_OP_MOD: {
    double am, bm;
    if ((rc = mkr_val_to_number_or_fail(&l, L, err, &am)) != 0) break;
    if ((rc = mkr_val_to_number_or_fail(&r, L, err, &bm)) != 0) break;
    out->type = MKR_XPATH_TYPE_NUMBER;
    out->u.number = fmod(am, bm);
    break;
  }
  case MKR_OP_UNION:
    /* union_nodeset reports its own typed error (TYPE on non-nodeset
     * operands, OOM/LIMIT from mkr_nodeset_push). Do not overwrite. */
    rc = union_nodeset(ctx, &l, &r, out, err);
    break;
  default:
    rc = -1;
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "unexpected binop");
    break;
  }
  mkr_val_clear(&l);
  mkr_val_clear(&r);
  return rc;
}

static int
eval_node(mkr_xpath_context_t *ctx, const mkr_node_t *n,
          MKR_DOM_NODE *self_node, size_t self_pos, size_t self_size,
          mkr_val_t *out, mkr_xpath_error_t *err)
{
  /* Budget + recursion bookkeeping. Every AST node visit counts as one
   * eval op; recursion depth tracks how deep we are in expression
   * nodes so we abort cleanly on pathological inputs. */
  mkr_xpath_limits_t *L = mkr_ctx_limits(ctx);
  if (mkr_limit_eval_op(L, err) != 0) return -1;
  if (mkr_limit_recurse_enter(L, err) != 0) return -1;

  /* Hoisting fast path: a CI subtree that's already been computed in
   * this evaluate is returned as a clone. The clone keeps ownership
   * semantics clean — mkr_val_clear on either copy is safe. */
  if (n->is_context_independent && n->memoized) {
    int rc = mkr_val_clone(&n->memo_value, out, err);
    mkr_limit_recurse_leave(L);
    return rc;
  }

  int rc;
  switch (n->kind) {
  case MKR_NK_LITERAL_STR: {
    mkr_owned_text_t text;
    if (mkr_owned_text_from_borrowed_copy(&text, mkr_borrowed_text_from_owned(n->u.literal),
                                          err, "out of memory copying literal") != 0) rc = -1;
    else {
      mkr_val_set_owned_text(out, text);
      rc = 0;
    }
    break;
  }
  case MKR_NK_LITERAL_NUM:
    out->type = MKR_XPATH_TYPE_NUMBER;
    out->u.number = n->u.literal_num;
    rc = 0;
    break;
  case MKR_NK_VARREF: {
    mkr_borrowed_text_t v;
    if (!mkr_ctx_lookup_variable_text(ctx, n->u.varref.prefix.ptr, n->u.varref.prefix.len,
                                      n->u.varref.name.ptr, n->u.varref.name.len, &v)) {
      mkr_err_setf(err, MKR_XPATH_ERR_RUNTIME, "undefined variable $%s%s%s",
                  n->u.varref.prefix.ptr ? n->u.varref.prefix.ptr : "",
                  n->u.varref.prefix.ptr ? ":" : "",
                  n->u.varref.name.ptr);
      rc = -1;
      break;
    }
    mkr_owned_text_t text;
    if (mkr_owned_text_from_borrowed_copy(&text, v, err,
                                          "out of memory copying variable value") != 0) rc = -1;
    else {
      mkr_val_set_owned_text(out, text);
      rc = 0;
    }
    break;
  }
  case MKR_NK_FNCALL:
    rc = eval_fncall(ctx, n, self_node, self_pos, self_size, out, err);
    break;
  case MKR_NK_UNARY: {
    mkr_val_t v = {0};
    if (eval_node(ctx, n->u.unary.expr, self_node, self_pos, self_size, &v, err) != 0) {
      rc = -1;
      break;
    }
    double d;
    if (mkr_val_to_number_or_fail(&v, L, err, &d) != 0) {
      mkr_val_clear(&v);
      rc = -1;
      break;
    }
    out->type = MKR_XPATH_TYPE_NUMBER;
    out->u.number = -d;
    mkr_val_clear(&v);
    rc = 0;
    break;
  }
  case MKR_NK_BINOP:
    rc = eval_binop(ctx, n, self_node, self_pos, self_size, out, err);
    break;
  case MKR_NK_PATH:
    rc = eval_path(ctx, n, self_node, out, err);
    break;
  case MKR_NK_FILTER:
    rc = eval_filter(ctx, n, self_node, self_pos, self_size, out, err);
    break;
  default:
    mkr_err_set(err, MKR_XPATH_ERR_INTERNAL, "unknown AST node");
    rc = -1;
    break;
  }

  /* Memoize on success when the subtree is CI. The clone keeps the
   * caller's out value independent of the cached one — important
   * because the caller is free to consume/modify their copy. */
  if (rc == 0 && n->is_context_independent && !n->memoized) {
    mkr_val_t memo = {0};
    if (mkr_val_clone(out, &memo, err) == 0) {
      /* Cast away const just for the memo slots; AST is otherwise
       * treated as read-only at eval. */
      mkr_node_t *mut = (mkr_node_t *)n;
      mut->memo_value = memo;
      mut->memoized   = 1;
    } else {
      /* OOM during clone — the caller's `out` is still valid. We
       * leave the node unmemoized and surface the error. */
      rc = -1;
    }
  }

  mkr_limit_recurse_leave(L);
  return rc;
}

int
mkr_eval_ast(mkr_xpath_context_t *ctx, const mkr_node_t *ast,
            mkr_val_t *out, mkr_xpath_error_t *err)
{
  MKR_DOM_NODE *self_node = mkr_ctx_node(ctx);
  return eval_node(ctx, ast, self_node, 1, 1, out, err);
}
