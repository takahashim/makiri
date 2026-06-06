#include "glue.h"
#include "../core/mkr_core.h"

#include <stdint.h>

/* MKR_NODE_SET_MAX (the per-set node cap, shared with the CSS/XPath glue) is
 * defined in glue.h. Every node-collecting path - tree walks
 * (children / element_children / attribute_nodes), XPath, and CSS - fails
 * closed at that bound instead of growing without limit. */

/* A NodeSet is a plain dynamic array of Lexbor node pointers plus a keepalive
 * reference to the owning Document. Nodes are owned by the document arena, so
 * marking the document keeps them all alive. */
/* The stored nodes are mkr_raw_node_t* (representation-opaque; see glue.h): the
 * set never dereferences them, it only compares them for identity and, when
 * vending a node, casts to the representation named by doc_is_xml. This keeps an
 * XML set from ever reading its mkr_xml_node_t* pointers as lxb_dom_node_t. */
typedef struct {
    mkr_raw_node_t **nodes;
    size_t           count;
    size_t           cap;
    VALUE            document;
    int              doc_is_xml;  /* cached once: the stored pointers are
                                   * mkr_xml_node_t* (wrap as Makiri::XML::*) */
} mkr_node_set_data_t;

/* Wrap a stored node into a Ruby Node, choosing the representation by the set's
 * (fixed) document kind - an XML document's nodes are custom mkr_xml_node_t. This
 * is the ONLY place a stored raw node is cast back to a typed pointer, and the
 * cast is justified by doc_is_xml. The kind is cached at construction so this
 * stays a single branch per node (a per-node is_kind_of/parsed-kind probe would
 * regress the hot traversal path). */
static VALUE
mkr_node_set_wrap(const mkr_node_set_data_t *s, mkr_raw_node_t *node)
{
    if (s->doc_is_xml) {
        return mkr_wrap_xml_node((struct mkr_xml_node *)node, s->document);
    }
    return mkr_wrap_html_node((lxb_dom_node_t *)node, s->document);
}

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
    size_t nodes_bytes;
    size_t total;
    if (!mkr_size_mul(s->cap, sizeof(mkr_raw_node_t *), &nodes_bytes)) {
        return sizeof(*s);
    }
    if (!mkr_size_add(sizeof(*s), nodes_bytes, &total)) {
        return sizeof(*s);
    }
    return total;
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
    s->nodes      = NULL;
    s->count      = 0;
    s->cap        = 0;
    s->document   = document;
    s->doc_is_xml = rb_obj_is_kind_of(document, mkr_cXmlDocument) ? 1 : 0;
    return obj;
}

void
mkr_node_set_push(VALUE rb_set, mkr_raw_node_t *node)
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
        if (!mkr_grow_capacity(s->cap, s->count + 1, sizeof(mkr_raw_node_t *), &new_cap)) {
            rb_raise(mkr_eError, "node set capacity overflow");
        }
        s->nodes = xrealloc2(s->nodes, new_cap, sizeof(mkr_raw_node_t *));
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

/* A new NodeSet from the [beg, beg+len) run of `s` (caller has clamped them to
 * [0, count]). */
static VALUE
mkr_node_set_slice(mkr_node_set_data_t *s, long beg, long len)
{
    VALUE result = mkr_node_set_new(s->document);
    for (long i = 0; i < len; i++) {
        mkr_node_set_push(result, s->nodes[beg + i]);
    }
    return result;
}

/* set[i]               -> Node or nil (negative indices count from the end).
 * set[start, length]   -> a new NodeSet (nil if start is out of range).
 * set[range]           -> a new NodeSet (nil if the range start is out of range).
 * Mirrors Array#[]. */
static VALUE
mkr_node_set_aref(int argc, VALUE *argv, VALUE self)
{
    mkr_node_set_data_t *s;
    TypedData_Get_Struct(self, mkr_node_set_data_t, &mkr_node_set_type, s);
    long count = (long)s->count;

    if (argc == 2) {                       /* set[start, length] */
        long beg = NUM2LONG(argv[0]);
        long len = NUM2LONG(argv[1]);
        if (beg < 0) beg += count;
        if (beg < 0 || beg > count || len < 0) return Qnil;
        if (len > count - beg) len = count - beg;
        return mkr_node_set_slice(s, beg, len);
    }

    rb_check_arity(argc, 1, 2);

    if (rb_obj_is_kind_of(argv[0], rb_cRange)) {
        long beg, len;
        if (rb_range_beg_len(argv[0], &beg, &len, count, 0) != Qtrue) {
            return Qnil;                   /* start out of range */
        }
        return mkr_node_set_slice(s, beg, len);
    }

    long i = NUM2LONG(argv[0]);
    if (i < 0) i += count;
    if (i < 0 || i >= count) return Qnil;
    return mkr_node_set_wrap(s, s->nodes[i]);
}

static VALUE
mkr_node_set_each(VALUE self)
{
    mkr_node_set_data_t *s;
    TypedData_Get_Struct(self, mkr_node_set_data_t, &mkr_node_set_type, s);

    RETURN_ENUMERATOR(self, 0, 0);

    for (size_t i = 0; i < s->count; i++) {
        rb_yield(mkr_node_set_wrap(s, s->nodes[i]));
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

/* The "other" operand of a set operation, requiring it to share +s+'s document.
 * A result NodeSet borrows exactly one document VALUE (GC keepalive) and one
 * representation flag (HTML vs XML) to wrap its nodes; mixing two documents -
 * which also means possibly mixing HTML and XML - would wrap a node under the
 * wrong representation and fail to keep its document alive. Fail closed instead
 * of silently producing a corrupt set. */
static mkr_node_set_data_t *
mkr_node_set_other(const mkr_node_set_data_t *s, VALUE other)
{
    mkr_node_set_data_t *o = mkr_node_set_get(other);
    if (o->document != s->document) {
        rb_raise(mkr_eError, "cannot combine node sets from different documents");
    }
    return o;
}

static int
mkr_node_set_member(const mkr_node_set_data_t *s, const mkr_raw_node_t *n)
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
 * factor < 0.5), so it never rehashes. cap == 0 means "not built" - the caller
 * then falls back to a linear scan (small operands, or allocation failure). */
typedef struct {
    const mkr_raw_node_t **slots;
    size_t                 cap;
} mkr_ptrset_t;

/* Below this operand size a linear scan is cheaper than building the hash. */
#define MKR_NODE_SET_HASH_MIN 64

/* Pointer hashing is shared: mkr_ptr_hash (core/mkr_core.h). */

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
mkr_ptrset_add(mkr_ptrset_t *set, const mkr_raw_node_t *p)
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
mkr_ptrset_has(const mkr_ptrset_t *set, const mkr_raw_node_t *p)
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
    mkr_node_set_data_t *o = mkr_node_set_other(s, other);
    VALUE result = mkr_node_set_new(s->document);
    mkr_node_set_data_t *r = mkr_node_set_get(result);

    mkr_ptrset_t seen = { NULL, 0 };
    if (s->count + o->count > MKR_NODE_SET_HASH_MIN) {
        mkr_ptrset_init(&seen, s->count + o->count);
    }
    mkr_node_set_data_t *srcs[2] = { s, o };
    for (int k = 0; k < 2; k++) {
        for (size_t i = 0; i < srcs[k]->count; i++) {
            mkr_raw_node_t *n = srcs[k]->nodes[i];
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
    mkr_node_set_data_t *o = mkr_node_set_other(s, other);
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
    mkr_node_set_data_t *o = mkr_node_set_other(s, other);
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
        mkr_raw_node_t *n = s->nodes[i];
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

/* #dup / #clone: a new NodeSet over the same nodes (the nodes are shared - they
 * are owned by the document arena - but the set itself is independent), like
 * Nokogiri. Defined here because the allocator is undef'd, so Ruby's default
 * allocate-then-copy raises; any level/freeze argument is ignored. */
static VALUE
mkr_node_set_dup(int argc, VALUE *argv, VALUE self)
{
    (void)argc;
    (void)argv;
    mkr_node_set_data_t *s = mkr_node_set_get(self);
    VALUE copy = mkr_node_set_new(s->document);
    /* Reuse the overflow-checked growth + cap enforcement of mkr_node_set_push;
     * the source already has no duplicates, so this is a faithful copy. */
    for (size_t i = 0; i < s->count; i++) {
        mkr_node_set_push(copy, s->nodes[i]);
    }
    return copy;
}

void
mkr_init_node_set(void)
{
    rb_define_method(mkr_cNodeSet, "|", mkr_node_set_op_or,    1);
    rb_define_method(mkr_cNodeSet, "+", mkr_node_set_op_plus,  1);
    rb_define_method(mkr_cNodeSet, "&", mkr_node_set_op_and,   1);
    rb_define_method(mkr_cNodeSet, "-", mkr_node_set_op_minus, 1);

    rb_define_method(mkr_cNodeSet, "length", mkr_node_set_length, 0);
    rb_define_method(mkr_cNodeSet, "[]",     mkr_node_set_aref,  -1);
    rb_define_method(mkr_cNodeSet, "each",   mkr_node_set_each,   0);
    rb_define_method(mkr_cNodeSet, "dup",    mkr_node_set_dup,   -1);
    /* #clone is defined in Ruby (node_set.rb) so it can honour `freeze:`. */
}
