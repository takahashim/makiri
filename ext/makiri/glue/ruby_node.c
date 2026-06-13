/* ruby_node.c - the shared, representation-neutral node core.
 *
 * HTML (Lexbor) nodes and XML (custom-arena) nodes are two representations of the
 * same Ruby-facing Node abstraction. This file owns what is common to BOTH: the
 * TypedData types that distinguish the two wrappers (so a representation-specific
 * accessor rejects the wrong kind via Ruby's own type machinery), the shared GC
 * functions, and the kind-agnostic accessors used for identity and document
 * lookup. The HTML node implementation (wrap/unwrap + reader methods) lives in
 * ruby_html_node.c, the XML one in ruby_xml_node.c. */
#include "glue.h"
#include "cross_import.h"          /* mkr_node_kind_t, for the import_node kind dispatch */
#include "../xml/mkr_xml_node.h"   /* mkr_xml_doc_t::doc_node, for the kind-aware mkr_node_raw */

/* ------------------------------------------------------------------ */
/* GC + TypedData types                                               */
/* ------------------------------------------------------------------ */

static void
mkr_node_gc_mark(void *ptr)
{
    mkr_node_data_t *nd = (mkr_node_data_t *)ptr;
    rb_gc_mark(nd->document);
}

static void
mkr_node_gc_free(void *ptr)
{
    /* The node is owned by the document arena (HTML or XML); never freed here. */
    xfree(ptr);
}

static size_t
mkr_node_memsize(const void *ptr)
{
    (void)ptr;
    return sizeof(mkr_node_data_t);
}

/* HTML and XML nodes share the mkr_node_data_t layout (node pointer + keepalive
 * Document) and the same GC functions, but are wrapped under DISTINCT TypedData
 * types so the representation is checked by Ruby's own type machinery: an HTML
 * accessor (mkr_html_node_unwrap, via mkr_html_node_type) raises TypeError when
 * handed an XML node and vice versa - it is structurally impossible to read one
 * representation's pointer as the other's. mkr_node_type is the shared base (both
 * derive from it via .parent), so the kind-agnostic identity accessors accept
 * either. This is the single source of HTML/XML node-pointer safety; there is no
 * ambiguous "return an lxb_dom_node_t for any node" unwrap. */
const rb_data_type_t mkr_node_type = {
    "Makiri::Node",
    { mkr_node_gc_mark, mkr_node_gc_free, mkr_node_memsize, },
    0, 0, RUBY_TYPED_FREE_IMMEDIATELY,
};
const rb_data_type_t mkr_html_node_type = {
    "Makiri::HTML::Node",
    { mkr_node_gc_mark, mkr_node_gc_free, mkr_node_memsize, },
    &mkr_node_type, 0, RUBY_TYPED_FREE_IMMEDIATELY,
};
const rb_data_type_t mkr_xml_node_type = {
    "Makiri::XML::Node",
    { mkr_node_gc_mark, mkr_node_gc_free, mkr_node_memsize, },
    &mkr_node_type, 0, RUBY_TYPED_FREE_IMMEDIATELY,
};

/* ------------------------------------------------------------------ */
/* kind-agnostic accessors (identity / document)                      */
/* ------------------------------------------------------------------ */

/* The kind-AGNOSTIC raw node pointer (base mkr_node_type, accepts HTML or XML),
 * as an opaque void* - dereferencing it requires an explicit cast, so it cannot be
 * mistaken for a typed pointer. Only for the few sites where the representation is
 * either irrelevant (identity comparison) or already guaranteed by an external
 * same-document/kind check (the XPath context node). The Document branch is
 * kind-aware (XML Document -> its arena document node, HTML -> the Lexbor one). */
void *
mkr_node_raw(VALUE rb_node)
{
    if (rb_obj_is_kind_of(rb_node, mkr_cDocument)) {
        mkr_parsed_t *parsed = mkr_doc_parsed(rb_node);
        if (mkr_parsed_kind(parsed) == MKR_DOC_XML) {
            mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(parsed);
            return xdoc ? (void *)xdoc->doc_node : NULL;
        }
        return (void *)mkr_html_doc_unwrap(rb_node);
    }
    mkr_node_data_t *nd;
    TypedData_Get_Struct(rb_node, mkr_node_data_t, &mkr_node_type, nd);
    return nd->node;
}

/* Which representation a wrapped node is, by its TypedData type (the robust
 * discriminator - not the Ruby class). A Document, a NodeSet, or any non-node is
 * MKR_NODE_KIND_OTHER. The cross-kind Document#import_node entries use this to
 * route a node to the same-representation copy or the cross-representation
 * translator (lexbor_compat/cross_import.c). */
mkr_node_kind_t
mkr_node_kind(VALUE v)
{
    if (rb_typeddata_is_kind_of(v, &mkr_html_node_type)) return MKR_NODE_KIND_HTML;
    if (rb_typeddata_is_kind_of(v, &mkr_xml_node_type))  return MKR_NODE_KIND_XML;
    return MKR_NODE_KIND_OTHER;
}

/* Node identity as an integer, for #==/#eql?/#hash/#pointer_id - kind-agnostic and
 * never dereferenced. */
uintptr_t
mkr_node_id(VALUE rb_node)
{
    return (uintptr_t)mkr_node_raw(rb_node);
}

VALUE
mkr_node_document(VALUE rb_node)
{
    if (rb_obj_is_kind_of(rb_node, mkr_cDocument)) {
        return rb_node;
    }
    mkr_node_data_t *nd;
    TypedData_Get_Struct(rb_node, mkr_node_data_t, &mkr_node_type, nd);
    return nd->document;
}

/* ------------------------------------------------------------------ */
/* identity (representation-neutral)                                  */
/* ------------------------------------------------------------------ */
/* These depend only on the kind-agnostic mkr_node_id (which never dereferences a
 * node), so they are identical for HTML and XML and live here, in the shared
 * node core, rather than duplicated in each representation's reader file. The
 * HTML and XML NodeMethods modules both bind their ==/eql?/hash/pointer_id to
 * these. */

/* Pointer identity: equal iff both wrappers resolve to the same node pointer (an
 * HTML node is thus never equal to an XML node). */
VALUE
mkr_node_equals(VALUE self, VALUE other)
{
    if (!rb_obj_is_kind_of(other, mkr_cNode)) {
        return Qfalse;
    }
    return mkr_node_id(self) == mkr_node_id(other) ? Qtrue : Qfalse;
}

/* Nokogiri-compatible identity: the underlying node pointer as an Integer. Stable
 * for the node's lifetime and unique among currently-live nodes; a
 * freed-then-reallocated node may reuse an address (same caveat as
 * Nokogiri::XML::Node#pointer_id). a.pointer_id == b.pointer_id iff a.eql?(b). */
VALUE
mkr_node_pointer_id(VALUE self)
{
    return ULL2NUM((unsigned long long)mkr_node_id(self));
}

/* Stable hash derived from the node pointer, so a == b implies a.hash == b.hash
 * even across separately-created wrappers. Shares the pointer value with
 * #pointer_id. */
VALUE
mkr_node_hash(VALUE self)
{
    return mkr_node_pointer_id(self);
}
