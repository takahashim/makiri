/* ruby_xml_node_read.c - the XML node's readers.
 *
 * Split from ruby_xml_node.c (see ruby_xml_node_internal.h for why): everything
 * here reads the arena node and answers, without ever writing to the tree. The
 * wrap/unwrap bridge lives here too, since reading is what needs it first.
 *
 * XML nodes never inherit the lxb_dom HTML readers - those are on
 * Makiri::HTML::NodeMethods - so this surface is structural rather than shared.
 */
#include "glue.h"
#include "ruby_xml_node_internal.h"
#include "../xml/mkr_xml_node.h"
#include "../core/mkr_core.h"

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
    case MKR_XML_NODE_TYPE_ATTRIBUTE: klass = mkr_cXmlAttr; break;
    case MKR_XML_NODE_TYPE_TEXT:      klass = mkr_cXmlText;      break;
    case MKR_XML_NODE_TYPE_CDATA_SECTION:     klass = mkr_cXmlCDATASection;     break;
    case MKR_XML_NODE_TYPE_COMMENT:   klass = mkr_cXmlComment;   break;
    case MKR_XML_NODE_TYPE_PI:        klass = mkr_cXmlProcessingInstruction; break;
    case MKR_XML_NODE_TYPE_DOCUMENT_TYPE: klass = mkr_cXmlDocumentType;       break;
    case MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT: klass = mkr_cXmlDocumentFragment; break;
    default:               klass = mkr_cXmlNode;      break;
    }
    mkr_node_data_t *nd;
    VALUE obj = TypedData_Make_Struct(klass, mkr_node_data_t, &mkr_xml_node_type, nd);
    nd->node     = (mkr_raw_node_t *)node;   /* an mkr_xml_node_t*; XML readers cast back */
    nd->document = document;
    return obj;
}

/* The XML node-pointer accessor (the counterpart of mkr_html_node_unwrap): returns the
 * mkr_xml_node_t for an XML node or XML Document, and RAISES TypeError for an HTML
 * node/Document (TypedData_Get_Struct checks mkr_xml_node_type, which an HTML node
 * - wrapped under mkr_html_node_type - does not satisfy). Non-static so the shared
 * XPath glue can resolve an XML context/result node safely. */
mkr_xml_node_t *
mkr_xml_node_unwrap(VALUE self)
{
    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        return mkr_parsed_xml_doc(mkr_doc_parsed(self))->doc_node;
    }
    mkr_node_data_t *nd;
    TypedData_Get_Struct(self, mkr_node_data_t, &mkr_xml_node_type, nd);
    return (mkr_xml_node_t *)nd->node;
}

VALUE
mkr_xml_node_document(VALUE self)
{
    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        return self;
    }
    mkr_node_data_t *nd;   /* XML-strict: rejects a non-XML node at the type boundary */
    TypedData_Get_Struct(self, mkr_node_data_t, &mkr_xml_node_type, nd);
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
    case MKR_XML_NODE_TYPE_DOCUMENT_FRAGMENT: return rb_utf8_str_new_cstr("#document-fragment");
    default:               return rb_utf8_str_new_cstr("document");
    }
}

/* ---- DTD (DOCUMENT_TYPE) identifiers ----
 *
 * The doctype node repurposes fields: local/qname = the DOCTYPE name
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

/* ---- namespace introspection (Nokogiri-compatible) ----
 *
 * Makiri::XML::Namespace is a small (prefix, href) value object. xmlns
 * declarations are stored as ordinary attribute nodes (qname "xmlns" /
 * "xmlns:PREFIX"), so the four queries below just read the tree:
 *   #namespace               -> the node's own resolved namespace, or nil
 *   #namespace_definitions   -> the xmlns declarations ON this element
 *   #namespaces              -> all xmlns declarations IN SCOPE here (a Hash)
 *   #collect_namespaces      -> every xmlns declaration in the document (a Hash) */
static VALUE mkr_cXmlNamespace;

static VALUE
mkr_ns_new(VALUE prefix, VALUE href)
{
    VALUE ns = rb_obj_alloc(mkr_cXmlNamespace);
    rb_ivar_set(ns, rb_intern("@prefix"), prefix);
    rb_ivar_set(ns, rb_intern("@href"), href);
    return ns;
}

static VALUE mkr_ns_prefix(VALUE self) { return rb_ivar_get(self, rb_intern("@prefix")); }
static VALUE mkr_ns_href(VALUE self)   { return rb_ivar_get(self, rb_intern("@href")); }

static VALUE
mkr_ns_equal(VALUE self, VALUE other)
{
    if (!rb_obj_is_kind_of(other, mkr_cXmlNamespace)) return Qfalse;
    return (rb_equal(mkr_ns_prefix(self), mkr_ns_prefix(other)) &&
            rb_equal(mkr_ns_href(self), mkr_ns_href(other))) ? Qtrue : Qfalse;
}

static VALUE
mkr_ns_hash(VALUE self)
{
    return rb_funcall(rb_ary_new3(2, mkr_ns_prefix(self), mkr_ns_href(self)), rb_intern("hash"), 0);
}

static VALUE
mkr_ns_inspect(VALUE self)
{
    return rb_sprintf("#<Makiri::XML::Namespace prefix=%" PRIsVALUE " href=%" PRIsVALUE ">",
                      rb_inspect(mkr_ns_prefix(self)), rb_inspect(mkr_ns_href(self)));
}

/* The xmlns-declaration detector lives in the node layer (mkr_xml_node_xmlns_decl)
 * so the namespace introspection, the C14N walk, and the mutation namespace
 * resolver all share one definition. */

static VALUE
mkr_xml_node_namespace(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if ((n->type == MKR_XML_NODE_TYPE_ELEMENT || n->type == MKR_XML_NODE_TYPE_ATTRIBUTE)
        && n->ns_uri_len > 0) {
        VALUE prefix = n->prefix_len ? rb_utf8_str_new(n->prefix, (long)n->prefix_len) : Qnil;
        return mkr_ns_new(prefix, rb_utf8_str_new(n->ns_uri, (long)n->ns_uri_len));
    }
    return Qnil;
}

static VALUE
mkr_xml_node_namespace_definitions(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    VALUE arr = rb_ary_new();
    if (n->type == MKR_XML_NODE_TYPE_ELEMENT) {
        for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
            const char *p, *u; uint32_t pl, ul;
            if (mkr_xml_node_xmlns_decl(a, &p, &pl, &u, &ul)) {
                VALUE prefix = pl ? rb_utf8_str_new(p, (long)pl) : Qnil;
                rb_ary_push(arr, mkr_ns_new(prefix, rb_utf8_str_new(u, (long)ul)));
            }
        }
    }
    return arr;
}

static VALUE
mkr_xml_node_namespaces(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    VALUE h = rb_hash_new();
    for (mkr_xml_node_t *e = n; e != NULL; e = e->parent) {   /* inner scope wins */
        if (e->type != MKR_XML_NODE_TYPE_ELEMENT) continue;
        for (mkr_xml_node_t *a = e->attrs; a != NULL; a = a->next) {
            const char *p, *u; uint32_t pl, ul;
            if (!mkr_xml_node_xmlns_decl(a, &p, &pl, &u, &ul)) continue;
            VALUE key = rb_utf8_str_new(a->qname, (long)a->qname_len); /* "xmlns" / "xmlns:p" */
            if (rb_hash_lookup2(h, key, Qundef) == Qundef) {
                rb_hash_aset(h, key, rb_utf8_str_new(u, (long)ul));
            }
        }
    }
    return h;
}

static VALUE
mkr_xml_node_collect_namespaces(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    mkr_xml_node_t *root = n;
    while (root->parent != NULL) root = root->parent;   /* the DOCUMENT node */
    VALUE h = rb_hash_new();
    /* iterative pre-order over the whole tree (parent-pointer walk, no recursion) */
    for (mkr_xml_node_t *cur = root; cur != NULL;) {
        if (cur->type == MKR_XML_NODE_TYPE_ELEMENT) {
            for (mkr_xml_node_t *a = cur->attrs; a != NULL; a = a->next) {
                const char *p, *u; uint32_t pl, ul;
                if (!mkr_xml_node_xmlns_decl(a, &p, &pl, &u, &ul)) continue;
                rb_hash_aset(h, rb_utf8_str_new(a->qname, (long)a->qname_len),
                             rb_utf8_str_new(u, (long)ul));
            }
        }
        if (cur->first_child != NULL) { cur = cur->first_child; continue; }
        while (cur != root && cur->next == NULL) cur = cur->parent;
        if (cur == root) break;
        cur = cur->next;
    }
    return h;
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
     * order. Iterative pre-order (parent-pointer) walk - no C recursion, so a
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

VALUE
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
        mkr_node_set_push(set, (mkr_raw_node_t *)c);
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
        if (mkr_bytes_eq(a->qname, a->qname_len, nv.ptr, nv.len)) {
            out = rb_utf8_str_new(a->value ? a->value : "", (long)a->value_len);
            break;
        }
    }
    RB_GC_GUARD(nv.value);
    return out;
}

/* The Attr node whose qualified name is exactly `name`, or nil. XML attributes
 * are stored under their qualified name, so this is the same match `#[]` makes
 * - but it hands back the node, which the DOM's by-name family needs in order
 * to read the attribute's namespace and prefix. (The HTML side has to look
 * harder, and carries the note on case: see
 * Makiri::HTML::NodeMethods#attribute_by_qualified_name.)
 *
 * set_attribute_ns can leave two attributes sharing a qualified name in
 * different namespaces; the first wins, as getAttribute does. */
static VALUE
mkr_xml_node_attribute_by_qualified_name(VALUE self, VALUE rb_name)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if (n->type != MKR_XML_NODE_TYPE_ELEMENT) return Qnil;
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "attribute name");
    VALUE out = Qnil;
    for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
        if (mkr_bytes_eq(a->qname, a->qname_len, nv.ptr, nv.len)) {
            out = mkr_wrap_xml_node(a, mkr_xml_node_document(self));
            break;
        }
    }
    RB_GC_GUARD(nv.value);
    return out;
}

/* The value of the attribute with that qualified name, or nil. Same match as
 * #[], which for XML is already the qualified-name one; it exists so the DOM
 * layer can ask both representations the same question. */
static VALUE
mkr_xml_node_attribute_value_by_qualified_name(VALUE self, VALUE rb_name)
{
    return mkr_xml_node_aref(self, rb_name);
}

static VALUE
mkr_xml_node_attribute_nodes(VALUE self)
{
    VALUE doc = mkr_xml_node_document(self);
    VALUE set = mkr_node_set_new(doc);
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if (n->type == MKR_XML_NODE_TYPE_ELEMENT) {
        for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
            mkr_node_set_push(set, (mkr_raw_node_t *)a);
        }
    }
    return set;
}

static VALUE
mkr_xml_node_get_document(VALUE self)
{
    return mkr_xml_node_document(self);
}


/* element_children -> NodeSet of the child element nodes (nodeType 1) only, in
 * document order (the counterpart of HTML's #element_children). */
static VALUE
mkr_xml_node_element_children(VALUE self)
{
    VALUE doc = mkr_xml_node_document(self);
    VALUE set = mkr_node_set_new(doc);
    for (mkr_xml_node_t *c = mkr_xml_node_unwrap(self)->first_child; c != NULL; c = c->next) {
        if (c->type == MKR_XML_NODE_TYPE_ELEMENT) {
            mkr_node_set_push(set, (mkr_raw_node_t *)c);
        }
    }
    return set;
}

void
mkr_init_xml_node_read(void)
{
    rb_define_method(mkr_mXmlNodeMethods, "name",          mkr_xml_node_name, 0);
    rb_define_method(mkr_mXmlNodeMethods, "local_name",    mkr_xml_node_local_name, 0);
    rb_define_method(mkr_mXmlNodeMethods, "prefix",        mkr_xml_node_prefix, 0);
    rb_define_method(mkr_mXmlNodeMethods, "namespace_uri", mkr_xml_node_namespace_uri, 0);
    rb_define_method(mkr_mXmlNodeMethods, "node_type",     mkr_xml_node_node_type, 0);

    /* namespace introspection (Nokogiri-compatible) + the Namespace value object */
    mkr_cXmlNamespace = rb_define_class_under(mkr_mXML, "Namespace", rb_cObject);
    rb_define_method(mkr_cXmlNamespace, "prefix",  mkr_ns_prefix, 0);
    rb_define_method(mkr_cXmlNamespace, "href",    mkr_ns_href, 0);
    rb_define_method(mkr_cXmlNamespace, "to_s",    mkr_ns_href, 0);
    rb_define_method(mkr_cXmlNamespace, "==",      mkr_ns_equal, 1);
    rb_define_method(mkr_cXmlNamespace, "eql?",    mkr_ns_equal, 1);
    rb_define_method(mkr_cXmlNamespace, "hash",    mkr_ns_hash, 0);
    rb_define_method(mkr_cXmlNamespace, "inspect", mkr_ns_inspect, 0);
    rb_define_method(mkr_mXmlNodeMethods, "namespace",     mkr_xml_node_namespace, 0);
    rb_define_method(mkr_mXmlNodeMethods, "namespace_definitions", mkr_xml_node_namespace_definitions, 0);
    rb_define_method(mkr_mXmlNodeMethods, "namespaces",    mkr_xml_node_namespaces, 0);
    rb_define_method(mkr_mXmlNodeMethods, "collect_namespaces", mkr_xml_node_collect_namespaces, 0);
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
    rb_define_method(mkr_mXmlNodeMethods, "element_children", mkr_xml_node_element_children, 0);
    rb_define_method(mkr_mXmlNodeMethods, "[]",            mkr_xml_node_aref, 1);
    rb_define_method(mkr_mXmlNodeMethods, "attribute_nodes", mkr_xml_node_attribute_nodes, 0);
    rb_define_method(mkr_mXmlNodeMethods, "attribute_by_qualified_name",
                     mkr_xml_node_attribute_by_qualified_name, 1);
    rb_define_method(mkr_mXmlNodeMethods, "attribute_value_by_qualified_name",
                     mkr_xml_node_attribute_value_by_qualified_name, 1);

    /* Node identity by underlying pointer, so #path / NodeSet dedup / Set / Hash
     * work (the same contract HTML nodes have). */
    rb_define_method(mkr_mXmlNodeMethods, "==",         mkr_node_equals, 1);
    rb_define_method(mkr_mXmlNodeMethods, "eql?",       mkr_node_equals, 1);
    rb_define_method(mkr_mXmlNodeMethods, "hash",       mkr_node_hash, 0);
    rb_define_method(mkr_mXmlNodeMethods, "pointer_id", mkr_node_pointer_id, 0);

    /* Makiri::XML::DocumentType identifiers (#public_id is the Nokogiri-style
     * alias of #external_id). #name comes from the shared reader. */
    rb_define_method(mkr_cXmlDocumentType, "external_id", mkr_xml_dtd_external_id, 0);
    rb_define_method(mkr_cXmlDocumentType, "public_id",   mkr_xml_dtd_external_id, 0);
    rb_define_method(mkr_cXmlDocumentType, "system_id",   mkr_xml_dtd_system_id, 0);
}
