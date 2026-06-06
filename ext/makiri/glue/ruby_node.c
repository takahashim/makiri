/* ruby_node.c — the shared, representation-neutral node core.
 *
 * HTML (Lexbor) nodes and XML (custom-arena) nodes are two representations of the
 * same Ruby-facing Node abstraction. This file owns what is common to BOTH: the
 * TypedData types that distinguish the two wrappers (so a representation-specific
 * accessor rejects the wrong kind via Ruby's own type machinery), the shared GC
 * functions, and the kind-agnostic accessors used for identity and document
 * lookup. The HTML node implementation (wrap/unwrap + reader methods) lives in
 * ruby_html_node.c, the XML one in ruby_xml_node.c. */
#include "glue.h"
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
 * handed an XML node and vice versa — it is structurally impossible to read one
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
 * as an opaque void* — dereferencing it requires an explicit cast, so it cannot be
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

/* Node identity as an integer, for #==/#eql?/#hash/#pointer_id — kind-agnostic and
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
