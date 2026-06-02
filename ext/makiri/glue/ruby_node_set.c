#include "glue.h"
#include "../core/mkr_safe.h"

#include <stdint.h>

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
        /* Geometric growth via mkr_grow_capacity; xrealloc2 does the
         * overflow-checked (count * size) allocation while keeping the buffer
         * GC-accounted and paired with xfree in gc_free. */
        size_t new_cap;
        if (!mkr_grow_capacity(s->cap, s->count + 1, sizeof(lxb_dom_node_t *), &new_cap)) {
            rb_raise(mkr_eError, "node set capacity overflow");
        }
        s->nodes = xrealloc2(s->nodes, new_cap, sizeof(lxb_dom_node_t *));
        s->cap = new_cap;
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

/* Open-addressing pointer-hash set, used to keep the set operators below O(n²)
 * (a CPU-DoS vector at large operand sizes). NULL is the empty sentinel; DOM
 * node pointers are never NULL. Sized once for the expected element count (load
 * factor < 0.5), so it never rehashes. cap == 0 means "not built" — the caller
 * then falls back to a linear scan (small operands, or allocation failure). */
typedef struct {
    const lxb_dom_node_t **slots;
    size_t                 cap;
} mkr_ptrset_t;

/* Below this operand size a linear scan is cheaper than building the hash. */
#define MKR_NODE_SET_HASH_MIN 64

static size_t
mkr_ptr_hash(const lxb_dom_node_t *p)
{
    uintptr_t x = (uintptr_t)p;
    x ^= x >> 33;
    x *= (uintptr_t)0xff51afd7ed558ccdULL;
    x ^= x >> 33;
    return (size_t)x;
}

/* Build (cap 0 on overflow / allocation failure → linear fallback). Sized for
 * load factor < 0.5 (>= n * 2 slots, min 16), all through the overflow-checked
 * safe-core helpers. */
static void
mkr_ptrset_init(mkr_ptrset_t *set, size_t n)
{
    set->slots = NULL;
    set->cap   = 0;

    size_t need, cap;
    if (!mkr_size_mul(n, 2, &need)) return;                       /* n * 2 overflow */
    if (!mkr_grow_capacity(16, need, sizeof(*set->slots), &cap)) return;

    set->slots = mkr_callocarray(cap, sizeof(*set->slots));
    set->cap   = (set->slots != NULL) ? cap : 0;
}

static void
mkr_ptrset_free(mkr_ptrset_t *set)
{
    free(set->slots);
}

/* Add p; returns 1 if newly added, 0 if already present. */
static int
mkr_ptrset_add(mkr_ptrset_t *set, const lxb_dom_node_t *p)
{
    size_t mask = set->cap - 1;
    size_t j = mkr_ptr_hash(p) & mask;
    while (set->slots[j] != NULL) {
        if (set->slots[j] == p) return 0;
        j = (j + 1) & mask;
    }
    set->slots[j] = p;
    return 1;
}

static int
mkr_ptrset_has(const mkr_ptrset_t *set, const lxb_dom_node_t *p)
{
    size_t mask = set->cap - 1;
    size_t j = mkr_ptr_hash(p) & mask;
    while (set->slots[j] != NULL) {
        if (set->slots[j] == p) return 1;
        j = (j + 1) & mask;
    }
    return 0;
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

    mkr_ptrset_t seen = { NULL, 0 };
    if (s->count + o->count > MKR_NODE_SET_HASH_MIN) {
        mkr_ptrset_init(&seen, s->count + o->count);
    }
    mkr_node_set_data_t *srcs[2] = { s, o };
    for (int k = 0; k < 2; k++) {
        for (size_t i = 0; i < srcs[k]->count; i++) {
            lxb_dom_node_t *n = srcs[k]->nodes[i];
            int fresh = seen.cap ? mkr_ptrset_add(&seen, n)
                                 : !mkr_node_set_member(r, n);
            if (fresh) mkr_node_set_push(result, n);
        }
    }
    mkr_ptrset_free(&seen);
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

/* Shared core of & and -: keep each (deduped) node of self whose membership in
 * other equals +keep_if_in_other+ (1 for intersection, 0 for difference). */
static VALUE
mkr_node_set_op_filter(VALUE self, VALUE other, int keep_if_in_other)
{
    mkr_node_set_data_t *s = mkr_node_set_get(self);
    mkr_node_set_data_t *o = mkr_node_set_get(other);
    VALUE result = mkr_node_set_new(s->document);
    mkr_node_set_data_t *r = mkr_node_set_get(result);

    mkr_ptrset_t oset = { NULL, 0 }; /* membership of `other` */
    if (o->count > MKR_NODE_SET_HASH_MIN) {
        mkr_ptrset_init(&oset, o->count);
        if (oset.cap) {
            for (size_t i = 0; i < o->count; i++) mkr_ptrset_add(&oset, o->nodes[i]);
        }
    }
    mkr_ptrset_t seen = { NULL, 0 }; /* result dedup */
    if (s->count > MKR_NODE_SET_HASH_MIN) {
        mkr_ptrset_init(&seen, s->count);
    }

    for (size_t i = 0; i < s->count; i++) {
        lxb_dom_node_t *n = s->nodes[i];
        int in_o = oset.cap ? mkr_ptrset_has(&oset, n) : mkr_node_set_member(o, n);
        if (in_o != keep_if_in_other) continue;
        int fresh = seen.cap ? mkr_ptrset_add(&seen, n) : !mkr_node_set_member(r, n);
        if (fresh) mkr_node_set_push(result, n);
    }
    mkr_ptrset_free(&oset);
    mkr_ptrset_free(&seen);
    return result;
}

/* self & other -> intersection (self order, deduped). */
static VALUE
mkr_node_set_op_and(VALUE self, VALUE other)
{
    return mkr_node_set_op_filter(self, other, 1);
}

/* self - other -> difference (self order, deduped). */
static VALUE
mkr_node_set_op_minus(VALUE self, VALUE other)
{
    return mkr_node_set_op_filter(self, other, 0);
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
