/* ruby_xml_node.c — Ruby read API for custom XML nodes (Phase 1, read-only).
 *
 * The XML counterpart of ruby_node.c: it wraps a mkr_xml_node_t into the right
 * Makiri::XML::* leaf and defines the reader/query methods on the
 * Makiri::XML::NodeMethods behavior module (included into every XML leaf), each
 * reading the custom node's fields directly. XML nodes never inherit the lxb_dom
 * HTML readers (those live on Makiri::HTML::NodeMethods), so the read-only
 * guarantee is structural. The shared node TypedData (mkr_node_type) stores the
 * node pointer + a keepalive Document VALUE; for XML the pointer is a
 * mkr_xml_node_t* (the document arena outlives the wrapper via the Document).
 */
#include "glue.h"
#include "../xml/mkr_xml_node.h"

#include <string.h>

/* ---- wrap / unwrap ---- */

VALUE
mkr_wrap_xml_node(mkr_xml_node_t *node, VALUE document)
{
    if (node == NULL) {
        return Qnil;
    }
    if (node->type == MKR_XML_NODE_TYPE_DOCUMENT) {
        return document;   /* the document node maps back onto the Ruby Document */
    }
    VALUE klass;
    switch (node->type) {
    case MKR_XML_NODE_TYPE_ELEMENT:   klass = mkr_cXmlElement;   break;
    case MKR_XML_NODE_TYPE_ATTRIBUTE: klass = mkr_cXmlAttribute; break;
    case MKR_XML_NODE_TYPE_TEXT:      klass = mkr_cXmlText;      break;
    case MKR_XML_NODE_TYPE_CDATA_SECTION:     klass = mkr_cXmlCData;     break;
    case MKR_XML_NODE_TYPE_COMMENT:   klass = mkr_cXmlComment;   break;
    case MKR_XML_NODE_TYPE_PI:        klass = mkr_cXmlProcessingInstruction; break;
    case MKR_XML_NODE_TYPE_DOCUMENT_TYPE: klass = mkr_cXmlDTD;       break;
    default:               klass = mkr_cXmlNode;      break;
    }
    mkr_node_data_t *nd;
    VALUE obj = TypedData_Make_Struct(klass, mkr_node_data_t, &mkr_node_type, nd);
    nd->node     = (lxb_dom_node_t *)node;   /* a mkr_xml_node_t*; XML readers cast back */
    nd->document = document;
    return obj;
}

static mkr_xml_node_t *
mkr_xml_node_unwrap(VALUE self)
{
    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        return mkr_parsed_xml_doc(mkr_doc_parsed(self))->doc_node;
    }
    mkr_node_data_t *nd;
    TypedData_Get_Struct(self, mkr_node_data_t, &mkr_node_type, nd);
    return (mkr_xml_node_t *)nd->node;
}

static VALUE
mkr_xml_node_document(VALUE self)
{
    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        return self;
    }
    mkr_node_data_t *nd;
    TypedData_Get_Struct(self, mkr_node_data_t, &mkr_node_type, nd);
    return nd->document;
}

/* ---- name / namespace ---- */

static VALUE
mkr_xml_node_name(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    switch (n->type) {
    case MKR_XML_NODE_TYPE_ELEMENT:
    case MKR_XML_NODE_TYPE_ATTRIBUTE: return rb_utf8_str_new(n->qname, (long)n->qname_len);
    case MKR_XML_NODE_TYPE_PI:        return rb_utf8_str_new(n->local, (long)n->local_len); /* target */
    case MKR_XML_NODE_TYPE_TEXT:      return rb_utf8_str_new_cstr("text");
    case MKR_XML_NODE_TYPE_CDATA_SECTION:     return rb_utf8_str_new_cstr("#cdata-section");
    case MKR_XML_NODE_TYPE_COMMENT:   return rb_utf8_str_new_cstr("comment");
    case MKR_XML_NODE_TYPE_DOCUMENT_TYPE: return rb_utf8_str_new(n->local, (long)n->local_len); /* the DOCTYPE name */
    default:               return rb_utf8_str_new_cstr("document");
    }
}

/* ---- DTD (DOCUMENT_TYPE) identifiers ----
 *
 * The off-tree doctype node repurposes fields: local/qname = the DOCTYPE name
 * (Node#name), prefix = the PUBLIC/external id, value = the SYSTEM id. A field
 * left NULL means that id was absent (-> nil); an empty literal (e.g. PUBLIC "")
 * is a non-NULL 0-length slice (-> ""). Mirrors Nokogiri::XML::DTD#external_id /
 * #system_id; #public_id is a WHATWG-DOM-style alias of #external_id. The DTD
 * itself is NOT parsed (no entities/elements), so there is nothing else to read. */
static VALUE
mkr_xml_dtd_external_id(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    return n->prefix == NULL ? Qnil : rb_utf8_str_new(n->prefix, (long)n->prefix_len);
}

static VALUE
mkr_xml_dtd_system_id(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    return n->value == NULL ? Qnil : rb_utf8_str_new(n->value, (long)n->value_len);
}

static VALUE
mkr_xml_node_local_name(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if (n->type == MKR_XML_NODE_TYPE_ELEMENT || n->type == MKR_XML_NODE_TYPE_ATTRIBUTE) {
        return rb_utf8_str_new(n->local, (long)n->local_len);
    }
    return Qnil;
}

static VALUE
mkr_xml_node_prefix(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    return n->prefix_len ? rb_utf8_str_new(n->prefix, (long)n->prefix_len) : Qnil;
}

static VALUE
mkr_xml_node_namespace_uri(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    return n->ns_uri_len ? rb_utf8_str_new(n->ns_uri, (long)n->ns_uri_len) : Qnil;
}

static VALUE
mkr_xml_node_node_type(VALUE self)
{
    return INT2NUM((int)mkr_xml_node_unwrap(self)->type);
}

/* ---- content / text ---- */

static VALUE
mkr_xml_node_content(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    /* leaf data nodes return their own value verbatim. */
    if (n->type == MKR_XML_NODE_TYPE_TEXT || n->type == MKR_XML_NODE_TYPE_CDATA_SECTION
        || n->type == MKR_XML_NODE_TYPE_COMMENT || n->type == MKR_XML_NODE_TYPE_ATTRIBUTE
        || n->type == MKR_XML_NODE_TYPE_PI) {
        return rb_utf8_str_new(n->value ? n->value : "", (long)n->value_len);
    }
    /* element / document: concatenate every TEXT/CDATA descendant in document
     * order. Iterative pre-order (parent-pointer) walk — no C recursion, so a
     * deep tree cannot overflow the stack. */
    VALUE str = rb_utf8_str_new("", 0);
    mkr_xml_node_t *cur = n->first_child;
    while (cur != NULL) {
        if ((cur->type == MKR_XML_NODE_TYPE_TEXT || cur->type == MKR_XML_NODE_TYPE_CDATA_SECTION) && cur->value_len) {
            rb_str_cat(str, cur->value, (long)cur->value_len);
        }
        if (cur->first_child != NULL) { cur = cur->first_child; continue; }
        while (cur != NULL && cur != n && cur->next == NULL) cur = cur->parent;
        if (cur == NULL || cur == n) break;
        cur = cur->next;
    }
    return str;
}

static VALUE
mkr_xml_node_value(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    return rb_utf8_str_new(n->value ? n->value : "", (long)n->value_len);
}

/* ---- navigation ---- */

static VALUE
mkr_xml_wrap_rel(VALUE self, mkr_xml_node_t *rel)
{
    return mkr_wrap_xml_node(rel, mkr_xml_node_document(self));
}

static VALUE mkr_xml_node_parent(VALUE self)   { return mkr_xml_wrap_rel(self, mkr_xml_node_unwrap(self)->parent); }
static VALUE mkr_xml_node_next(VALUE self)     { return mkr_xml_wrap_rel(self, mkr_xml_node_unwrap(self)->next); }
static VALUE mkr_xml_node_previous(VALUE self) { return mkr_xml_wrap_rel(self, mkr_xml_node_unwrap(self)->prev); }
static VALUE mkr_xml_node_first_child(VALUE self) { return mkr_xml_wrap_rel(self, mkr_xml_node_unwrap(self)->first_child); }
static VALUE mkr_xml_node_last_child(VALUE self)  { return mkr_xml_wrap_rel(self, mkr_xml_node_unwrap(self)->last_child); }

static VALUE
mkr_xml_node_children(VALUE self)
{
    VALUE doc = mkr_xml_node_document(self);
    VALUE set = mkr_node_set_new(doc);
    for (mkr_xml_node_t *c = mkr_xml_node_unwrap(self)->first_child; c != NULL; c = c->next) {
        mkr_node_set_push(set, (lxb_dom_node_t *)c);
    }
    return set;
}

/* ---- attributes ---- */

static VALUE
mkr_xml_node_aref(VALUE self, VALUE rb_name)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if (n->type != MKR_XML_NODE_TYPE_ELEMENT) return Qnil;
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "attribute name");
    VALUE out = Qnil;
    for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
        if (a->qname_len == nv.len && memcmp(a->qname, nv.ptr, nv.len) == 0) {
            out = rb_utf8_str_new(a->value ? a->value : "", (long)a->value_len);
            break;
        }
    }
    RB_GC_GUARD(nv.value);
    return out;
}

static VALUE
mkr_xml_node_attribute_nodes(VALUE self)
{
    VALUE doc = mkr_xml_node_document(self);
    VALUE set = mkr_node_set_new(doc);
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if (n->type == MKR_XML_NODE_TYPE_ELEMENT) {
        for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
            mkr_node_set_push(set, (lxb_dom_node_t *)a);
        }
    }
    return set;
}

static VALUE
mkr_xml_node_get_document(VALUE self)
{
    return mkr_xml_node_document(self);
}

/* ---- fail-closed guards for the unsupported query / serialization surface ----
 *
 * Makiri::XML is a strict, read-only reader (§7.5 / §12). Two HTML-node
 * conveniences would silently misbehave on XML and are turned into explicit
 * NotImplementedError instead of a wrong result:
 *   - CSS selectors: Lexbor's lxb_selectors lower-cases names, which destroys
 *     XML's case- and namespace-sensitive matching — use #xpath.
 *   - serialization (to_xml/to_html/to_s/inner_html/outer_html): writing the
 *     read-only tree back out is a later phase; emitting HTML-serialized markup
 *     for an XML document would be wrong (escaping / CDATA / xhtml differ). */
static VALUE
mkr_xml_node_no_css(int argc, VALUE *argv, VALUE self)
{
    (void)argc; (void)argv; (void)self;
    rb_raise(rb_eNotImpError,
             "Makiri::XML does not support CSS selectors; use #xpath. "
             "(CSS cannot preserve XML element-name case or namespaces.) "
             "Default-namespace documents (RSS/Atom) need a registered prefix, "
             "e.g. doc.xpath(\"//a:entry\", \"a\" => \"http://www.w3.org/2005/Atom\").");
}

static VALUE
mkr_xml_node_no_serialize(int argc, VALUE *argv, VALUE self)
{
    (void)argc; (void)argv; (void)self;
    rb_raise(rb_eNotImpError,
             "Makiri::XML is read-only in this version; node serialization "
             "(to_xml / to_html / to_s / inner_html / outer_html) is not supported.");
}

/* ---- node identity (== / eql? / hash / pointer_id) ----
 *
 * XML nodes share the mkr_node_data_t typed-data with HTML nodes, so the
 * underlying node pointer (mkr_node_unwrap, representation-agnostic) IS the
 * identity. Without these, two wrappers for the same XML node compared unequal
 * (Object identity), which broke #path, NodeSet/Set dedup, and Hash keys.
 * Same contract as the HTML nodes (ruby_node.c): a.pointer_id == b.pointer_id
 * iff a.eql?(b). */
static VALUE
mkr_xml_node_pointer_id(VALUE self)
{
    return ULL2NUM((unsigned long long)(uintptr_t)mkr_node_unwrap(self));
}

static VALUE
mkr_xml_node_equals(VALUE self, VALUE other)
{
    if (!rb_obj_is_kind_of(other, mkr_cNode)) return Qfalse;
    return mkr_node_unwrap(self) == mkr_node_unwrap(other) ? Qtrue : Qfalse;
}

void
mkr_init_xml_node(void)
{
    rb_define_method(mkr_mXmlNodeMethods, "name",          mkr_xml_node_name, 0);
    rb_define_method(mkr_mXmlNodeMethods, "local_name",    mkr_xml_node_local_name, 0);
    rb_define_method(mkr_mXmlNodeMethods, "prefix",        mkr_xml_node_prefix, 0);
    rb_define_method(mkr_mXmlNodeMethods, "namespace_uri", mkr_xml_node_namespace_uri, 0);
    rb_define_method(mkr_mXmlNodeMethods, "node_type",     mkr_xml_node_node_type, 0);
    rb_define_method(mkr_mXmlNodeMethods, "content",       mkr_xml_node_content, 0);
    rb_define_method(mkr_mXmlNodeMethods, "text",          mkr_xml_node_content, 0);
    rb_define_method(mkr_mXmlNodeMethods, "inner_text",    mkr_xml_node_content, 0);
    rb_define_method(mkr_mXmlNodeMethods, "value",         mkr_xml_node_value, 0);
    rb_define_method(mkr_mXmlNodeMethods, "document",      mkr_xml_node_get_document, 0);
    rb_define_method(mkr_mXmlNodeMethods, "parent",        mkr_xml_node_parent, 0);
    rb_define_method(mkr_mXmlNodeMethods, "next",          mkr_xml_node_next, 0);
    rb_define_method(mkr_mXmlNodeMethods, "next_sibling",  mkr_xml_node_next, 0);
    rb_define_method(mkr_mXmlNodeMethods, "previous",      mkr_xml_node_previous, 0);
    rb_define_method(mkr_mXmlNodeMethods, "previous_sibling", mkr_xml_node_previous, 0);
    rb_define_method(mkr_mXmlNodeMethods, "child",         mkr_xml_node_first_child, 0);
    rb_define_method(mkr_mXmlNodeMethods, "last_element_child", mkr_xml_node_last_child, 0);
    rb_define_method(mkr_mXmlNodeMethods, "children",      mkr_xml_node_children, 0);
    rb_define_method(mkr_mXmlNodeMethods, "[]",            mkr_xml_node_aref, 1);
    rb_define_method(mkr_mXmlNodeMethods, "attribute_nodes", mkr_xml_node_attribute_nodes, 0);

    /* Node identity by underlying pointer, so #path / NodeSet dedup / Set / Hash
     * work (the same contract HTML nodes have). */
    rb_define_method(mkr_mXmlNodeMethods, "==",         mkr_xml_node_equals, 1);
    rb_define_method(mkr_mXmlNodeMethods, "eql?",       mkr_xml_node_equals, 1);
    rb_define_method(mkr_mXmlNodeMethods, "hash",       mkr_xml_node_pointer_id, 0);
    rb_define_method(mkr_mXmlNodeMethods, "pointer_id", mkr_xml_node_pointer_id, 0);

    /* Fail-closed: CSS + serialization are unsupported on XML; raise rather than
     * return a wrong result (these shadow nothing — XML nodes have no css/to_s
     * otherwise). Also defined on XML::Document, which includes this module. */
    rb_define_method(mkr_mXmlNodeMethods, "css",        mkr_xml_node_no_css, -1);
    rb_define_method(mkr_mXmlNodeMethods, "at_css",     mkr_xml_node_no_css, -1);
    rb_define_method(mkr_mXmlNodeMethods, "to_xml",     mkr_xml_node_no_serialize, -1);
    rb_define_method(mkr_mXmlNodeMethods, "to_html",    mkr_xml_node_no_serialize, -1);
    rb_define_method(mkr_mXmlNodeMethods, "to_s",       mkr_xml_node_no_serialize, -1);
    rb_define_method(mkr_mXmlNodeMethods, "inner_html", mkr_xml_node_no_serialize, -1);
    rb_define_method(mkr_mXmlNodeMethods, "outer_html", mkr_xml_node_no_serialize, -1);

    /* Makiri::XML::DTD identifiers (Nokogiri-compatible; #public_id is the
     * WHATWG-DOM alias of #external_id). #name comes from the shared reader. */
    rb_define_method(mkr_cXmlDTD, "external_id", mkr_xml_dtd_external_id, 0);
    rb_define_method(mkr_cXmlDTD, "public_id",   mkr_xml_dtd_external_id, 0);
    rb_define_method(mkr_cXmlDTD, "system_id",   mkr_xml_dtd_system_id, 0);
}
