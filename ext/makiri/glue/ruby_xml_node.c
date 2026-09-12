/* ruby_xml_node.c - Ruby read + mutation API for custom XML nodes.
 *
 * The XML counterpart of ruby_node.c: it wraps a mkr_xml_node_t into the right
 * Makiri::XML::* leaf and defines the reader/query/mutation methods on the
 * Makiri::XML::NodeMethods behavior module (included into every XML leaf), each
 * reading the custom node's fields directly. XML nodes never inherit the lxb_dom
 * HTML readers (those live on Makiri::HTML::NodeMethods), so the surface is
 * structural; the in-place edits (remove/[]=/delete/content=/name=) route to the
 * Ruby-free primitives in xml/mkr_xml_mutate.c. The shared node TypedData (mkr_node_type) stores the
 * node pointer + a keepalive Document VALUE; for XML the pointer is a
 * mkr_xml_node_t* (the document arena outlives the wrapper via the Document).
 */
#include "glue.h"
#include "ruby_xml_node_internal.h"
#include "cross_import.h"          /* mkr_node_kind, mkr_cross_html_to_xml, mut_check decl */
#include "../xml/mkr_xml.h"        /* MKR_XML_MAX_DEPTH - the serializer honours the reader's nesting cap */
#include "../xml/mkr_xml_node.h"
#include "../xml/mkr_xml_mutate.h"
#include "../xml/mkr_xml_index.h"   /* element-name index invalidation on mutation */
#include "../core/mkr_core.h"   /* mkr_buf */

#include <ruby/encoding.h>     /* rb_to_encoding / rb_str_encode (output encoding) */
#include <stdlib.h>            /* qsort / free (C14N attribute sort) */
#include <string.h>

/* ---- node identity (== / eql? / hash / pointer_id) ----
 *
 * XML nodes share the mkr_node_data_t typed-data with HTML nodes, so the
 * underlying node pointer (mkr_node_id, representation-agnostic) IS the identity.
 * The implementations are therefore representation-neutral and live once in
 * ruby_node.c (mkr_node_equals / mkr_node_hash / mkr_node_pointer_id); the XML
 * NodeMethods module just binds to them below, exactly as the HTML module does. */

/* ---- mutation (Phase 1: in-place edits) ----------------------------------
 *
 * The write surface over the custom XML arena. The Ruby-free primitives live in
 * xml/mkr_xml_mutate.c (validation, namespace resolution, link/unlink); this
 * layer only coerces+verifies arguments through the bridge and maps the
 * mutation status to a Ruby exception. Detach-never-destroy: a removed node is
 * unlinked, never freed, so live wrappers stay valid (the same invariant the
 * read-only reader had). The XML reader keeps no attr/text index, so - unlike
 * the HTML side - there is nothing to invalidate after an edit. */

static mkr_xml_doc_t *
mkr_xml_node_xdoc(VALUE self)
{
    return mkr_parsed_xml_doc(mkr_doc_parsed(mkr_xml_node_document(self)));
}

/* A value/name byte length as a uint32 (the arena's per-slice cap), or raise. */
static uint32_t
mkr_xml_u32_len(size_t len)
{
    if (len > UINT32_MAX) {
        rb_raise(mkr_eError, "string too long for an XML node (max 4 GiB)");
    }
    return (uint32_t)len;
}

/* Map a mutation status to a Ruby exception (MKR_XML_MUT_OK returns). Shared with
 * ruby_doc.c / ruby_cross_import.c via cross_import.h (the cross-kind import entries
 * reuse it), so it is not static. */
void
mkr_xml_mut_check(mkr_xml_mut_status_t st)
{
    switch (st) {
    case MKR_XML_MUT_OK:        return;
    case MKR_XML_MUT_OOM:       rb_raise(mkr_eError, "out of memory mutating XML");
    case MKR_XML_MUT_BAD_NAME:  rb_raise(rb_eArgError, "not a well-formed XML name");
    case MKR_XML_MUT_BAD_CHARS: rb_raise(mkr_eError,
                                    "value contains a character or sequence not permitted in XML");
    case MKR_XML_MUT_UNBOUND_NS: rb_raise(mkr_eError,
                                    "namespace prefix is not bound in this scope");
    case MKR_XML_MUT_TYPE:      rb_raise(mkr_eError, "operation unsupported for this node type");
    case MKR_XML_MUT_CYCLE:     rb_raise(mkr_eError, "cannot insert a node into its own subtree");
    case MKR_XML_MUT_HIERARCHY: rb_raise(mkr_eError,
                                    "invalid placement (an attribute/document node cannot be a "
                                    "tree child, a document allows a single root element, and a "
                                    "sibling target must have a parent)");
    case MKR_XML_MUT_BAD_NS_DECL: rb_raise(mkr_eError,
                                    "cannot bind a namespace prefix to the empty namespace");
    }
    rb_raise(mkr_eError, "unknown XML mutation error");   /* unreachable; keeps the compiler happy */
}

/* Unwrap an XML node for mutation: a frozen node (Object#freeze) is immutable, so
 * raise FrozenError rather than edit it (the same contract HTML nodes have). */
static mkr_xml_node_t *
mkr_xml_node_unwrap_mutable(VALUE self)
{
    rb_check_frozen(self);
    /* Single mutation choke point (every mutator calls this): drop the cached
     * element-name index so the next query rebuilds it. Same discipline as the
     * HTML attr/text indices, in one place that cannot be forgotten. */
    mkr_xml_name_index_invalidate(mkr_xml_node_xdoc(self));
    return mkr_xml_node_unwrap(self);
}

/* node.remove / node.unlink -> node. Detach from the tree (or, for an attribute,
 * from its owner element); the node stays usable. */
static VALUE
mkr_xml_node_remove(VALUE self)
{
    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        rb_raise(mkr_eError, "cannot remove the document node");
    }
    mkr_xml_node_t *n = mkr_xml_node_unwrap_mutable(self);
    mkr_xml_remove(mkr_xml_node_xdoc(self), n);   /* detach + refresh the root/doctype cache */
    return self;
}

/* element[name] = value -> value. Adds or replaces the attribute. */
static VALUE
mkr_xml_node_aset(VALUE self, VALUE rb_name, VALUE rb_value)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap_mutable(self);
    if (n->type != MKR_XML_NODE_TYPE_ELEMENT) {
        rb_raise(mkr_eError, "cannot set an attribute on a non-element node");
    }
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "attribute name");
    mkr_ruby_borrowed_text_t vv = mkr_ruby_verified_text(rb_value, "attribute value");
    mkr_xml_mut_status_t st = mkr_xml_set_attribute(
        mkr_xml_node_xdoc(self), n,
        nv.ptr, mkr_xml_u32_len(nv.len), vv.ptr, mkr_xml_u32_len(vv.len), NULL);
    RB_GC_GUARD(nv.value);
    RB_GC_GUARD(vv.value);
    mkr_xml_mut_check(st);
    return rb_value;
}

/* element.set_attribute_ns(namespace_or_nil, qualified_name, value) -> value.
 *
 * Stores the attribute keyed on (explicit namespace, local name) - the DOM key -
 * with its qualified name case-preserved. A null/"" namespace is the null
 * namespace. xmlns declarations pass through as ordinary attributes in the xmlns
 * namespace. */
static VALUE
mkr_xml_node_set_attribute_ns(VALUE self, VALUE rb_ns, VALUE rb_qname, VALUE rb_value)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap_mutable(self);
    if (n->type != MKR_XML_NODE_TYPE_ELEMENT) {
        rb_raise(mkr_eError, "cannot set an attribute on a non-element node");
    }
    mkr_ruby_borrowed_text_t qv = mkr_ruby_verified_text(rb_qname, "attribute qualified name");
    mkr_ruby_borrowed_text_t vv = mkr_ruby_verified_text(rb_value, "attribute value");
    mkr_ruby_borrowed_text_t nv = {0};
    const char *ns_ptr = NULL; uint32_t ns_len = 0;
    if (!NIL_P(rb_ns)) {
        nv = mkr_ruby_verified_text(rb_ns, "namespace");
        ns_ptr = nv.ptr; ns_len = mkr_xml_u32_len(nv.len);
    }
    mkr_xml_mut_status_t st = mkr_xml_set_attribute_ns(
        mkr_xml_node_xdoc(self), n, ns_ptr, ns_len,
        qv.ptr, mkr_xml_u32_len(qv.len), vv.ptr, mkr_xml_u32_len(vv.len), NULL);
    RB_GC_GUARD(qv.value);
    RB_GC_GUARD(vv.value);
    RB_GC_GUARD(nv.value);
    mkr_xml_mut_check(st);
    return rb_value;
}

/* element.remove_attribute_ns(namespace_or_nil, local_name) -> self. */
static VALUE
mkr_xml_node_remove_attribute_ns(VALUE self, VALUE rb_ns, VALUE rb_local)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap_mutable(self);
    if (n->type != MKR_XML_NODE_TYPE_ELEMENT) return self;
    mkr_ruby_borrowed_text_t lv = mkr_ruby_verified_text(rb_local, "attribute local name");
    mkr_ruby_borrowed_text_t nv = {0};
    const char *ns_ptr = NULL; uint32_t ns_len = 0;
    if (!NIL_P(rb_ns)) {
        nv = mkr_ruby_verified_text(rb_ns, "namespace");
        ns_ptr = nv.ptr; ns_len = mkr_xml_u32_len(nv.len);
    }
    mkr_xml_remove_attribute_ns(n, ns_ptr, ns_len, lv.ptr, mkr_xml_u32_len(lv.len));
    RB_GC_GUARD(lv.value);
    RB_GC_GUARD(nv.value);
    return self;
}

/* element.delete(name) -> self. Removes the attribute if present (no-op otherwise). */
static VALUE
mkr_xml_node_delete(VALUE self, VALUE rb_name)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap_mutable(self);
    if (n->type != MKR_XML_NODE_TYPE_ELEMENT) return self;
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "attribute name");
    mkr_xml_remove_attribute(n, nv.ptr, mkr_xml_u32_len(nv.len));
    RB_GC_GUARD(nv.value);
    return self;
}

/* node.content = text -> text. For an element: replace its children with one text
 * node (the string is stored verbatim and escaped on serialization). For a
 * text/cdata/comment/PI leaf: set its data. */
static VALUE
mkr_xml_node_set_content(VALUE self, VALUE rb_text)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap_mutable(self);
    mkr_ruby_borrowed_text_t tv = mkr_ruby_verified_text(rb_text, "node content");
    mkr_xml_mut_status_t st = mkr_xml_set_content(
        mkr_xml_node_xdoc(self), n, tv.ptr, mkr_xml_u32_len(tv.len));
    RB_GC_GUARD(tv.value);
    mkr_xml_mut_check(st);
    return rb_text;
}

/* node.name = new_name -> new_name. Renames an element or attribute in place
 * (identity + tree position preserved); the namespace is re-resolved against the
 * node's in-scope declarations. */
static VALUE
mkr_xml_node_set_name(VALUE self, VALUE rb_name)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap_mutable(self);
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "node name");
    mkr_xml_mut_status_t st = mkr_xml_rename(
        mkr_xml_node_xdoc(self), n, nv.ptr, mkr_xml_u32_len(nv.len));
    RB_GC_GUARD(nv.value);
    mkr_xml_mut_check(st);
    return rb_name;
}

/* ---- Phase 2: building new subtrees --------------------------------------
 *
 * Document factories create a detached node in the document's arena; insertion
 * (add_child / before / after / replace) links a node, resolving the inserted
 * subtree's namespaces against its new context, and deep-copies (imports) a node
 * that comes from another document. The Ruby-free primitives live in
 * xml/mkr_xml_mutate.c. NodeSet / String arguments are a later phase. */

/* Coerce +arg+ to an XML node that lives in (or is imported into) +xdoc+ (the
 * target document's arena, whose Ruby VALUE is +target_doc+). A node from another
 * document is deep-copied; a same-document node is returned as-is (move). */
static mkr_xml_node_t *
mkr_xml_incoming_node(mkr_xml_doc_t *xdoc, VALUE target_doc, VALUE arg, VALUE *adopt_from)
{
    *adopt_from = Qnil;
    if (!rb_obj_is_kind_of(arg, mkr_cNode)
        || !rb_obj_is_kind_of(mkr_xml_node_document(arg), mkr_cXmlDocument)) {
        rb_raise(rb_eTypeError,
                 "expected a Makiri::XML node (NodeSet / String arguments are a later phase)");
    }
    mkr_xml_node_t *src = mkr_xml_node_unwrap(arg);
    if (mkr_xml_node_document(arg) == target_doc) {
        return src;                                 /* same arena -> move */
    }
    /* Another document: the arenas own their own nodes, so the node cannot be
     * relinked across them. Copy it here and - once the insert has actually
     * succeeded - take it out of the document it came from, so the operation
     * reads as the move the DOM says it is (mkr_xml_adopt_finish). */
    mkr_xml_node_t *copy = NULL;
    mkr_xml_mut_check(mkr_xml_import_subtree(xdoc, src, &copy));
    *adopt_from = arg;
    return copy;
}

/* Finish the adoption: empty the node out of its old document. Called only after
 * the insert succeeded, so a rejected one leaves the source document alone.
 * A fragment is emptied rather than detached - it contributed its children, and
 * the DOM leaves a spliced fragment empty. */
static void
mkr_xml_adopt_finish(VALUE arg)
{
    if (NIL_P(arg)) return;

    mkr_xml_node_t *src = mkr_xml_node_unwrap(arg);
    mkr_xml_doc_t *sdoc = mkr_xml_node_xdoc(arg);
    if (src->type == MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT) {
        mkr_xml_node_t *c;
        while ((c = src->first_child) != NULL) mkr_xml_remove(sdoc, c);
    } else {
        mkr_xml_remove(sdoc, src);
    }
    mkr_xml_name_index_invalidate(sdoc);
}

/* The four insertion verbs share this shape: frozen-check self, coerce/import the
 * argument, run the primitive, and return the inserted node (wrapped from the
 * target document). +op+ selects the primitive. */
typedef enum { MKR_INS_CHILD, MKR_INS_BEFORE, MKR_INS_AFTER, MKR_INS_REPLACE } mkr_ins_op_t;

/* A DOCUMENT_FRAGMENT contributes its CHILDREN, not itself (like Nokogiri / the
 * DOM): splice them in place of the fragment, in order, leaving it empty. Each
 * child is inserted relative to +target+ per +op+ (resolving its namespaces
 * against the new context, as a single node would); for AFTER the insertion point
 * advances so the children keep their order. Returns the (now empty) fragment, as
 * Nokogiri's add_child/before/after/replace return the fragment. */
static VALUE
mkr_xml_splice_fragment(mkr_xml_doc_t *xdoc, mkr_xml_node_t *target,
                        mkr_xml_node_t *frag, VALUE doc_v, mkr_ins_op_t op)
{
    if (op == MKR_INS_REPLACE) {
        /* Whole-fragment replace is an engine primitive: it validates the fragment
         * before touching a link and keeps +target+ until every child is spliced
         * in, so a rejected replace never destroys the replaced subtree. */
        mkr_xml_mut_check(mkr_xml_replace_with_fragment(xdoc, target, frag));
        return mkr_wrap_xml_node(frag, doc_v);
    }

    mkr_xml_node_t *ref = target;   /* moving insertion point for AFTER */
    mkr_xml_node_t *c;
    while ((c = frag->first_child) != NULL) {   /* each insert detaches c from frag */
        mkr_xml_mut_status_t st;
        switch (op) {
        case MKR_INS_CHILD:   st = mkr_xml_insert_child(xdoc, target, c);  break;
        case MKR_INS_AFTER:   st = mkr_xml_insert_after(xdoc, ref, c); ref = c; break;
        default:              st = mkr_xml_insert_before(xdoc, target, c); break; /* BEFORE */
        }
        mkr_xml_mut_check(st);
    }
    return mkr_wrap_xml_node(frag, doc_v);
}

static VALUE
mkr_xml_node_insert(VALUE self, VALUE arg, mkr_ins_op_t op)
{
    mkr_xml_node_t *target = mkr_xml_node_unwrap_mutable(self);
    VALUE doc_v = mkr_xml_node_document(self);
    mkr_xml_doc_t *xdoc = mkr_xml_node_xdoc(self);
    VALUE adopt_from;
    mkr_xml_node_t *node = mkr_xml_incoming_node(xdoc, doc_v, arg, &adopt_from);

    if (node->type == MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT) {
        VALUE out = mkr_xml_splice_fragment(xdoc, target, node, doc_v, op);
        mkr_xml_adopt_finish(adopt_from);
        return out;
    }

    mkr_xml_mut_status_t st;
    switch (op) {
    case MKR_INS_CHILD:   st = mkr_xml_insert_child(xdoc, target, node);  break;
    case MKR_INS_BEFORE:  st = mkr_xml_insert_before(xdoc, target, node); break;
    case MKR_INS_AFTER:   st = mkr_xml_insert_after(xdoc, target, node);  break;
    default:              st = mkr_xml_replace_node(xdoc, target, node);  break;
    }
    mkr_xml_mut_check(st);
    mkr_xml_adopt_finish(adopt_from);
    return mkr_wrap_xml_node(node, doc_v);
}

/* element.add_child(node) -> the inserted node. */
static VALUE mkr_xml_node_add_child(VALUE self, VALUE arg) { return mkr_xml_node_insert(self, arg, MKR_INS_CHILD); }
/* node.add_previous_sibling(other) / node.before(other) -> the inserted node. */
static VALUE mkr_xml_node_before(VALUE self, VALUE arg)    { return mkr_xml_node_insert(self, arg, MKR_INS_BEFORE); }
/* node.add_next_sibling(other) / node.after(other) -> the inserted node. */
static VALUE mkr_xml_node_after(VALUE self, VALUE arg)     { return mkr_xml_node_insert(self, arg, MKR_INS_AFTER); }
/* node.replace(other) -> the inserted node (the replaced node is detached). */
static VALUE mkr_xml_node_replace(VALUE self, VALUE arg)   { return mkr_xml_node_insert(self, arg, MKR_INS_REPLACE); }

/* element << node -> self (Nokogiri's <<: append and return the receiver). */
static VALUE
mkr_xml_node_lshift(VALUE self, VALUE arg)
{
    mkr_xml_node_insert(self, arg, MKR_INS_CHILD);
    return self;
}

/* ---- Document factories ---- */

/* rb_hash_foreach body: set one attribute on the element (arg). Keys/values are
 * stringified (Nokogiri accepts symbol keys / non-string values), then run
 * through the normal validated attribute setter. */
static int
mkr_xml_create_attr_i(VALUE key, VALUE val, VALUE rb_el)
{
    mkr_xml_node_aset(rb_el, rb_obj_as_string(key), rb_obj_as_string(val));
    return ST_CONTINUE;
}

static int
mkr_xml_dom_name_forbidden_byte(unsigned char c)
{
    return c == 0 || c == '\t' || c == '\n' || c == '\f' || c == '\r'
        || c == ' ' || c == '/' || c == '>';
}

static int
mkr_xml_dom_prefix_ok(const char *p, size_t len)
{
    if (len == 0) return 0;
    for (size_t i = 0; i < len; i++) {
        if (mkr_xml_dom_name_forbidden_byte((unsigned char)p[i])) return 0;
    }
    return 1;
}

static int
mkr_xml_dom_local_ok(const char *p, size_t len)
{
    if (len == 0) return 0;
    unsigned char first = (unsigned char)p[0];
    if (first < 0x80
        && !((first >= 'A' && first <= 'Z') || (first >= 'a' && first <= 'z')
             || first == ':' || first == '_')) {
        return 0;
    }
    for (size_t i = 0; i < len; i++) {
        if (mkr_xml_dom_name_forbidden_byte((unsigned char)p[i])) return 0;
    }
    return 1;
}

static void
mkr_xml_dom_name_consistency(mkr_ruby_borrowed_text_t qv,
                             mkr_ruby_borrowed_text_t pv,
                             int has_prefix,
                             mkr_ruby_borrowed_text_t lv,
                             mkr_xml_qname_t *qn)
{
    if (!mkr_xml_dom_local_ok(lv.ptr, lv.len)) {
        rb_raise(rb_eArgError, "invalid DOM element local name");
    }
    if (!has_prefix) {
        if (qv.len != lv.len || memcmp(qv.ptr, lv.ptr, qv.len) != 0) {
            rb_raise(rb_eArgError,
                     "qualified name must equal local name when prefix is nil");
        }
        qn->qname = qv.ptr; qn->qname_len = mkr_xml_u32_len(qv.len);
        qn->prefix = qv.ptr; qn->prefix_len = 0;
        qn->local = qv.ptr; qn->local_len = mkr_xml_u32_len(qv.len);
        return;
    }

    if (!mkr_xml_dom_prefix_ok(pv.ptr, pv.len)) {
        rb_raise(rb_eArgError, "invalid DOM element prefix");
    }
    if (qv.len != pv.len + 1 + lv.len
        || memcmp(qv.ptr, pv.ptr, pv.len) != 0
        || qv.ptr[pv.len] != ':'
        || memcmp(qv.ptr + pv.len + 1, lv.ptr, lv.len) != 0) {
        rb_raise(rb_eArgError,
                 "qualified name must be prefix + ':' + local name");
    }
    qn->qname = qv.ptr; qn->qname_len = mkr_xml_u32_len(qv.len);
    qn->prefix = qv.ptr; qn->prefix_len = mkr_xml_u32_len(pv.len);
    qn->local = qv.ptr + pv.len + 1; qn->local_len = mkr_xml_u32_len(lv.len);
}


/* clone_node(deep = false) -> a detached copy of this node in the same document
 * (element/attribute name case, namespaces, and the CDATA node type preserved);
 * deep copies the whole subtree. Backs Node#dup / #clone and DOM cloneNode. */
static VALUE
mkr_xml_node_clone_node(int argc, VALUE *argv, VALUE self)
{
    VALUE rb_deep;
    rb_scan_args(argc, argv, "01", &rb_deep);
    mkr_xml_node_t *out = NULL;
    mkr_xml_mut_check(mkr_xml_clone_node(mkr_xml_node_xdoc(self),
                                         mkr_xml_node_unwrap(self),
                                         RTEST(rb_deep), &out));
    return mkr_xml_wrap_rel(self, out);
}

/* create_element(name, content = nil, attributes = {}) -> Element.
 * Nokogiri-style trailing arguments: a Hash sets attributes, any other (non-nil)
 * argument is the element's text content. */
static VALUE
mkr_xml_doc_create_element(int argc, VALUE *argv, VALUE self)
{
    VALUE rb_name, rb_rest;
    rb_scan_args(argc, argv, "1*", &rb_name, &rb_rest);
    VALUE rb_content = Qnil, rb_attrs = Qnil;
    for (long i = 0; i < RARRAY_LEN(rb_rest); i++) {
        VALUE a = RARRAY_AREF(rb_rest, i);
        if (RB_TYPE_P(a, T_HASH)) {
            rb_attrs = a;
        } else if (!NIL_P(a)) {
            rb_content = a;
        }
    }

    mkr_xml_doc_t *xdoc = mkr_xml_node_xdoc(self);
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "element name");
    mkr_xml_node_t *el = NULL;
    mkr_xml_mut_status_t st = mkr_xml_new_element(xdoc, nv.ptr, mkr_xml_u32_len(nv.len), &el);
    RB_GC_GUARD(nv.value);
    mkr_xml_mut_check(st);
    if (!NIL_P(rb_content)) {
        mkr_ruby_borrowed_text_t tv = mkr_ruby_verified_text(rb_content, "element content");
        st = mkr_xml_set_content(xdoc, el, tv.ptr, mkr_xml_u32_len(tv.len));
        RB_GC_GUARD(tv.value);
        mkr_xml_mut_check(st);
    }
    VALUE rb_el = mkr_wrap_xml_node(el, self);
    if (!NIL_P(rb_attrs)) {
        rb_hash_foreach(rb_attrs, mkr_xml_create_attr_i, rb_el);
    }
    return rb_el;
}

/* create_loose_dom_element(qualified_name, prefix, local_name, namespace_uri)
 * -> Element.
 *
 * Internal browser-DOM interop escape hatch: create an XML-backed element whose
 * element name follows WHATWG DOM element-name rules rather than XML QName
 * rules. The resulting node is deliberately not XML-serializable. */
static VALUE
mkr_xml_doc_create_loose_dom_element(VALUE self, VALUE rb_qname, VALUE rb_prefix,
                                     VALUE rb_local, VALUE rb_ns)
{
    mkr_xml_doc_t *xdoc = mkr_xml_node_xdoc(self);
    mkr_ruby_borrowed_text_t qv = mkr_ruby_verified_text(rb_qname, "qualified name");
    mkr_ruby_borrowed_text_t lv = mkr_ruby_verified_text(rb_local, "local name");
    mkr_ruby_borrowed_text_t pv = { Qnil, NULL, 0 };
    int has_prefix = !NIL_P(rb_prefix);
    if (has_prefix) {
        pv = mkr_ruby_verified_text(rb_prefix, "prefix");
    }
    mkr_ruby_borrowed_text_t nv = { Qnil, NULL, 0 };
    if (!NIL_P(rb_ns)) {
        nv = mkr_ruby_verified_text(rb_ns, "namespace URI");
    }

    mkr_xml_qname_t qn;
    mkr_xml_dom_name_consistency(qv, pv, has_prefix, lv, &qn);
    mkr_xml_node_t *el = NULL;
    mkr_xml_mut_status_t st = mkr_xml_new_loose_dom_element(
        xdoc, &qn, nv.ptr, mkr_xml_u32_len(nv.len), &el);
    RB_GC_GUARD(qv.value);
    RB_GC_GUARD(lv.value);
    RB_GC_GUARD(pv.value);
    RB_GC_GUARD(nv.value);
    mkr_xml_mut_check(st);
    return mkr_wrap_xml_node(el, self);
}

/* create_document_type(name, public_id = "", system_id = "") -> DocumentType.
 *
 * DOM DOMImplementation.createDocumentType: a detached DocumentType owned by this
 * document, to be placed before the root with add_child / add_previous_sibling
 * (the placement guards keep it document-level, pre-root, and single). An
 * omitted or empty public/system id is absent; an invalid name fails closed. */
static VALUE
mkr_xml_doc_create_document_type(int argc, VALUE *argv, VALUE self)
{
    VALUE rb_name, rb_pub, rb_sys;
    rb_scan_args(argc, argv, "12", &rb_name, &rb_pub, &rb_sys);
    mkr_xml_doc_t *xdoc = mkr_xml_node_xdoc(self);

    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "doctype name");
    mkr_ruby_borrowed_text_t pv = { Qnil, NULL, 0 }, sv = { Qnil, NULL, 0 };
    if (!NIL_P(rb_pub)) pv = mkr_ruby_verified_text(rb_pub, "doctype public id");
    if (!NIL_P(rb_sys)) sv = mkr_ruby_verified_text(rb_sys, "doctype system id");
    /* empty id == absent (NULL), matching the HTML factory and Nokogiri */
    const char *pub = pv.len ? pv.ptr : NULL;
    const char *sys = sv.len ? sv.ptr : NULL;

    mkr_xml_node_t *dt = NULL;
    mkr_xml_mut_status_t st = mkr_xml_new_document_type(
        xdoc, nv.ptr, mkr_xml_u32_len(nv.len),
        pub, mkr_xml_u32_len(pv.len), sys, mkr_xml_u32_len(sv.len), &dt);
    RB_GC_GUARD(nv.value);
    RB_GC_GUARD(pv.value);
    RB_GC_GUARD(sv.value);
    mkr_xml_mut_check(st);
    return mkr_wrap_xml_node(dt, self);
}

/* Shared body for the leaf-data factories (text / comment / cdata). */
static VALUE
mkr_xml_doc_create_chardata(VALUE self, VALUE rb_text, uint8_t type, const char *what)
{
    mkr_xml_doc_t *xdoc = mkr_xml_node_xdoc(self);
    mkr_ruby_borrowed_text_t tv = mkr_ruby_verified_text(rb_text, what);
    mkr_xml_node_t *n = NULL;
    mkr_xml_mut_status_t st = mkr_xml_new_chardata(xdoc, type, tv.ptr, mkr_xml_u32_len(tv.len), &n);
    RB_GC_GUARD(tv.value);
    mkr_xml_mut_check(st);
    return mkr_wrap_xml_node(n, self);
}

static VALUE mkr_xml_doc_create_text_node(VALUE self, VALUE t) { return mkr_xml_doc_create_chardata(self, t, MKR_XML_NODE_TYPE_TEXT, "text content"); }
static VALUE mkr_xml_doc_create_comment(VALUE self, VALUE t)   { return mkr_xml_doc_create_chardata(self, t, MKR_XML_NODE_TYPE_COMMENT, "comment content"); }
static VALUE mkr_xml_doc_create_cdata(VALUE self, VALUE t)     { return mkr_xml_doc_create_chardata(self, t, MKR_XML_NODE_TYPE_CDATA_SECTION, "CDATA content"); }

static VALUE
mkr_xml_doc_create_pi(VALUE self, VALUE rb_target, VALUE rb_data)
{
    mkr_xml_doc_t *xdoc = mkr_xml_node_xdoc(self);
    mkr_ruby_borrowed_text_t tg = mkr_ruby_verified_text(rb_target, "PI target");
    mkr_ruby_borrowed_text_t dt = mkr_ruby_verified_text(rb_data, "PI data");
    mkr_xml_node_t *pi = NULL;
    mkr_xml_mut_status_t st = mkr_xml_new_pi(
        xdoc, tg.ptr, mkr_xml_u32_len(tg.len), dt.ptr, mkr_xml_u32_len(dt.len), &pi);
    RB_GC_GUARD(tg.value);
    RB_GC_GUARD(dt.value);
    mkr_xml_mut_check(st);
    return mkr_wrap_xml_node(pi, self);
}

/* Makiri::XML::Document#import_node(node, deep = false) - the DOM importNode for
 * an XML document. A same-representation (XML) node is deep/shallow-copied into
 * this document's arena (namespaces re-resolved when it is later linked); an HTML
 * node is TRANSLATED across representations (lxb -> mkr) by ruby_cross_import.c.
 * The result is detached and owned by this document; the source is untouched.
 * Fails closed (no partial node returned). */
static VALUE
mkr_xml_doc_import_node(int argc, VALUE *argv, VALUE self)
{
    VALUE node_v, deep_v;
    rb_scan_args(argc, argv, "11", &node_v, &deep_v);
    int deep = RTEST(deep_v);

    mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(self));
    mkr_xml_node_t *copy = NULL;

    switch (mkr_node_kind(node_v)) {
    case MKR_NODE_KIND_XML:
        mkr_xml_mut_check(mkr_xml_copy_node(xdoc, mkr_xml_node_unwrap(node_v), deep, &copy));
        break;
    case MKR_NODE_KIND_HTML:
        mkr_xml_mut_check(mkr_cross_html_to_xml(xdoc, mkr_html_node_unwrap(node_v), deep, &copy));
        break;
    default:
        rb_raise(rb_eTypeError, "import_node expects a Makiri node");
    }
    return mkr_wrap_xml_node(copy, self);
}

/* The node-class .new constructors (Element/Text/Comment/CDATASection/ProcessingInstruction.new)
 * and Document#root= are pure delegations to the document factories / insertion
 * verbs and live in the Ruby convenience layer (lib/makiri/), so a single
 * definition serves both HTML and XML and the document-type check is consistent. */

void
mkr_init_xml_node(void)
{
    /* Serialization (#to_xml / #canonicalize, and the refused HTML ones)
     * lives in ruby_xml_node_serialize.c. */
    mkr_init_xml_node_serialize();
    /* The readers (name/namespace/content/navigation/attributes) and the
     * Namespace value object live in ruby_xml_node_read.c. */
    mkr_init_xml_node_read();


    /* Mutation (Phase 1: in-place edits). Detach-never-destroy; the primitives
     * live in xml/mkr_xml_mutate.c. */
    rb_define_method(mkr_mXmlNodeMethods, "remove",   mkr_xml_node_remove,      0);
    rb_define_method(mkr_mXmlNodeMethods, "unlink",   mkr_xml_node_remove,      0);
    rb_define_method(mkr_mXmlNodeMethods, "[]=",      mkr_xml_node_aset,        2);
    rb_define_method(mkr_mXmlNodeMethods, "delete",   mkr_xml_node_delete,      1);
    rb_define_method(mkr_mXmlNodeMethods, "remove_attribute", mkr_xml_node_delete, 1);
    rb_define_method(mkr_mXmlNodeMethods, "set_attribute_ns",    mkr_xml_node_set_attribute_ns,    3);
    rb_define_method(mkr_mXmlNodeMethods, "remove_attribute_ns", mkr_xml_node_remove_attribute_ns, 2);
    rb_define_method(mkr_mXmlNodeMethods, "content=", mkr_xml_node_set_content, 1);
    rb_define_method(mkr_mXmlNodeMethods, "name=",    mkr_xml_node_set_name,    1);

    /* Mutation (Phase 2: building). Insertion accepts a single Makiri::XML node;
     * a node from another document is deep-copied (imported) into this one. */
    rb_define_method(mkr_mXmlNodeMethods, "add_child",             mkr_xml_node_add_child, 1);
    rb_define_method(mkr_mXmlNodeMethods, "<<",                    mkr_xml_node_lshift,    1);
    rb_define_method(mkr_mXmlNodeMethods, "add_previous_sibling",  mkr_xml_node_before,    1);
    rb_define_method(mkr_mXmlNodeMethods, "before",               mkr_xml_node_before,    1);
    rb_define_method(mkr_mXmlNodeMethods, "add_next_sibling",      mkr_xml_node_after,     1);
    rb_define_method(mkr_mXmlNodeMethods, "after",                mkr_xml_node_after,     1);
    rb_define_method(mkr_mXmlNodeMethods, "replace",              mkr_xml_node_replace,   1);

    /* Document factories. The node-class .new constructors and Document#root= are
     * pure delegations to these, defined once in the Ruby layer (lib/makiri/)
     * so HTML and XML share them. */
    rb_define_method(mkr_cXmlDocument, "create_element",                mkr_xml_doc_create_element, -1);
    rb_define_method(mkr_cXmlDocument, "create_loose_dom_element",      mkr_xml_doc_create_loose_dom_element, 4);
    rb_define_method(mkr_cXmlDocument, "create_document_type",          mkr_xml_doc_create_document_type, -1);
    rb_define_method(mkr_cXmlDocument, "create_text_node",             mkr_xml_doc_create_text_node, 1);
    rb_define_method(mkr_cXmlDocument, "create_comment",               mkr_xml_doc_create_comment, 1);
    rb_define_method(mkr_cXmlDocument, "create_cdata",                 mkr_xml_doc_create_cdata, 1);
    rb_define_method(mkr_cXmlDocument, "create_cdata_node",            mkr_xml_doc_create_cdata, 1);
    rb_define_method(mkr_cXmlDocument, "create_processing_instruction", mkr_xml_doc_create_pi, 2);
    rb_define_method(mkr_cXmlDocument, "import_node",                   mkr_xml_doc_import_node, -1);



    rb_define_method(mkr_mXmlNodeMethods, "clone_node",    mkr_xml_node_clone_node, -1);
}
