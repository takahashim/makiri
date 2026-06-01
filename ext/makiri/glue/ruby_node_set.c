#include "glue.h"

/* MKR_NODE_SET_MAX (the per-set node cap, shared with the CSS/XPath glue) is
 * defined in glue.h. Every node-collecting path — tree walks
 * (children / element_children / attribute_nodes), XPath, and CSS — fails
 * closed at that bound instead of growing without limit. */

/* A NodeSet is a plain dynamic array of Lexbor node pointers plus a keepalive
 * reference to the owning Document. Nodes are owned by the document arena, so
 * marking the document keeps them all alive. */
typedef struct {
    lxb_dom_node_t **nodes;
    size_t           count;
    size_t           cap;
    VALUE            document;
} mkr_node_set_data_t;

static void
mkr_node_set_gc_mark(void *ptr)
{
    mkr_node_set_data_t *s = (mkr_node_set_data_t *)ptr;
    rb_gc_mark(s->document);
}

static void
mkr_node_set_gc_free(void *ptr)
{
    mkr_node_set_data_t *s = (mkr_node_set_data_t *)ptr;
    if (s->nodes != NULL) {
        xfree(s->nodes);
    }
    xfree(s);
}

static size_t
mkr_node_set_memsize(const void *ptr)
{
    const mkr_node_set_data_t *s = (const mkr_node_set_data_t *)ptr;
    return sizeof(*s) + s->cap * sizeof(lxb_dom_node_t *);
}

const rb_data_type_t mkr_node_set_type = {
    "Makiri::NodeSet",
    { mkr_node_set_gc_mark, mkr_node_set_gc_free, mkr_node_set_memsize, },
    0, 0, RUBY_TYPED_FREE_IMMEDIATELY,
};

VALUE
mkr_node_set_new(VALUE document)
{
    mkr_node_set_data_t *s;
    VALUE obj = TypedData_Make_Struct(mkr_cNodeSet, mkr_node_set_data_t,
                                      &mkr_node_set_type, s);
    s->nodes    = NULL;
    s->count    = 0;
    s->cap      = 0;
    s->document = document;
    return obj;
}

void
mkr_node_set_push(VALUE rb_set, lxb_dom_node_t *node)
{
    mkr_node_set_data_t *s;
    TypedData_Get_Struct(rb_set, mkr_node_set_data_t, &mkr_node_set_type, s);

    if (s->count >= MKR_NODE_SET_MAX) {
        rb_raise(mkr_eError, "node set size limit exceeded (%u nodes)",
                 MKR_NODE_SET_MAX);
    }

    if (s->count == s->cap) {
        size_t ncap = (s->cap == 0) ? 8 : s->cap * 2;
        if (ncap <= s->cap) { /* overflow */
            rb_raise(mkr_eError, "node set capacity overflow");
        }
        s->nodes = xrealloc(s->nodes, ncap * sizeof(lxb_dom_node_t *));
        s->cap = ncap;
    }
    s->nodes[s->count++] = node;
}

static VALUE
mkr_node_set_length(VALUE self)
{
    mkr_node_set_data_t *s;
    TypedData_Get_Struct(self, mkr_node_set_data_t, &mkr_node_set_type, s);
    return ULONG2NUM(s->count);
}

/* set[i] -> Node or nil. Negative indices count from the end. */
static VALUE
mkr_node_set_aref(VALUE self, VALUE rb_index)
{
    mkr_node_set_data_t *s;
    TypedData_Get_Struct(self, mkr_node_set_data_t, &mkr_node_set_type, s);

    long i = NUM2LONG(rb_index);
    if (i < 0) {
        i += (long)s->count;
    }
    if (i < 0 || (size_t)i >= s->count) {
        return Qnil;
    }
    return mkr_wrap_node(s->nodes[i], s->document);
}

static VALUE
mkr_node_set_each(VALUE self)
{
    mkr_node_set_data_t *s;
    TypedData_Get_Struct(self, mkr_node_set_data_t, &mkr_node_set_type, s);

    RETURN_ENUMERATOR(self, 0, 0);

    for (size_t i = 0; i < s->count; i++) {
        rb_yield(mkr_wrap_node(s->nodes[i], s->document));
    }
    return self;
}

static mkr_node_set_data_t *
mkr_node_set_get(VALUE v)
{
    if (!rb_obj_is_kind_of(v, mkr_cNodeSet)) {
        rb_raise(rb_eTypeError, "expected a Makiri::NodeSet");
    }
    mkr_node_set_data_t *s;
    TypedData_Get_Struct(v, mkr_node_set_data_t, &mkr_node_set_type, s);
    return s;
}

static int
mkr_node_set_member(const mkr_node_set_data_t *s, const lxb_dom_node_t *n)
{
    for (size_t i = 0; i < s->count; i++) {
        if (s->nodes[i] == n) {
            return 1;
        }
    }
    return 0;
}

/* Append node to result unless it is already present (dedup by identity). r is
 * the result's data struct (stable across pushes; only its array reallocs). */
static void
mkr_push_unique(VALUE result, mkr_node_set_data_t *r, lxb_dom_node_t *n)
{
    if (!mkr_node_set_member(r, n)) {
        mkr_node_set_push(result, n);
    }
}

/*
 * Set operations. Results preserve encounter order (self first) and dedupe by
 * node identity. Document-order is NOT imposed (that is the XPath engine's job);
 * these mirror Nokogiri's operators on the common same-query operands.
 */

/* self | other -> union (deduped). */
static VALUE
mkr_node_set_op_or(VALUE self, VALUE other)
{
    mkr_node_set_data_t *s = mkr_node_set_get(self);
    mkr_node_set_data_t *o = mkr_node_set_get(other);
    VALUE result = mkr_node_set_new(s->document);
    mkr_node_set_data_t *r = mkr_node_set_get(result);
    for (size_t i = 0; i < s->count; i++) mkr_push_unique(result, r, s->nodes[i]);
    for (size_t i = 0; i < o->count; i++) mkr_push_unique(result, r, o->nodes[i]);
    return result;
}

/* self + other -> concatenation (duplicates kept). */
static VALUE
mkr_node_set_op_plus(VALUE self, VALUE other)
{
    mkr_node_set_data_t *s = mkr_node_set_get(self);
    mkr_node_set_data_t *o = mkr_node_set_get(other);
    VALUE result = mkr_node_set_new(s->document);
    for (size_t i = 0; i < s->count; i++) mkr_node_set_push(result, s->nodes[i]);
    for (size_t i = 0; i < o->count; i++) mkr_node_set_push(result, o->nodes[i]);
    return result;
}

/* self & other -> intersection (self order, deduped). */
static VALUE
mkr_node_set_op_and(VALUE self, VALUE other)
{
    mkr_node_set_data_t *s = mkr_node_set_get(self);
    mkr_node_set_data_t *o = mkr_node_set_get(other);
    VALUE result = mkr_node_set_new(s->document);
    mkr_node_set_data_t *r = mkr_node_set_get(result);
    for (size_t i = 0; i < s->count; i++) {
        if (mkr_node_set_member(o, s->nodes[i])) {
            mkr_push_unique(result, r, s->nodes[i]);
        }
    }
    return result;
}

/* self - other -> difference (self order, deduped). */
static VALUE
mkr_node_set_op_minus(VALUE self, VALUE other)
{
    mkr_node_set_data_t *s = mkr_node_set_get(self);
    mkr_node_set_data_t *o = mkr_node_set_get(other);
    VALUE result = mkr_node_set_new(s->document);
    mkr_node_set_data_t *r = mkr_node_set_get(result);
    for (size_t i = 0; i < s->count; i++) {
        if (!mkr_node_set_member(o, s->nodes[i])) {
            mkr_push_unique(result, r, s->nodes[i]);
        }
    }
    return result;
}

void
mkr_init_node_set(void)
{
    rb_define_method(mkr_cNodeSet, "|", mkr_node_set_op_or,    1);
    rb_define_method(mkr_cNodeSet, "+", mkr_node_set_op_plus,  1);
    rb_define_method(mkr_cNodeSet, "&", mkr_node_set_op_and,   1);
    rb_define_method(mkr_cNodeSet, "-", mkr_node_set_op_minus, 1);

    rb_define_method(mkr_cNodeSet, "length", mkr_node_set_length, 0);
    rb_define_method(mkr_cNodeSet, "[]",     mkr_node_set_aref,   1);
    rb_define_method(mkr_cNodeSet, "each",   mkr_node_set_each,   0);
}
