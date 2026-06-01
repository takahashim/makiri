#include "glue.h"

/* ------------------------------------------------------------------ */
/* Node wrapper type                                                  */
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
    /* The lxb_dom_node_t is owned by the document arena; never freed here. */
    xfree(ptr);
}

static size_t
mkr_node_memsize(const void *ptr)
{
    (void)ptr;
    return sizeof(mkr_node_data_t);
}

const rb_data_type_t mkr_node_type = {
    "Makiri::Node",
    { mkr_node_gc_mark, mkr_node_gc_free, mkr_node_memsize, },
    0, 0, RUBY_TYPED_FREE_IMMEDIATELY,
};

/* ------------------------------------------------------------------ */
/* wrap / unwrap                                                      */
/* ------------------------------------------------------------------ */

VALUE
mkr_wrap_node(lxb_dom_node_t *node, VALUE document)
{
    if (node == NULL) {
        return Qnil;
    }

    /* The document node maps back onto the Ruby Document object. */
    if (node->type == LXB_DOM_NODE_TYPE_DOCUMENT) {
        return document;
    }

    VALUE klass;
    switch (node->type) {
    case LXB_DOM_NODE_TYPE_ELEMENT:       klass = mkr_cElement;   break;
    case LXB_DOM_NODE_TYPE_ATTRIBUTE:     klass = mkr_cAttribute; break;
    case LXB_DOM_NODE_TYPE_TEXT:          klass = mkr_cText;      break;
    case LXB_DOM_NODE_TYPE_COMMENT:       klass = mkr_cComment;   break;
    case LXB_DOM_NODE_TYPE_CDATA_SECTION: klass = mkr_cCData;     break;
    case LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION:
                                          klass = mkr_cProcessingInstruction; break;
    case LXB_DOM_NODE_TYPE_DOCUMENT_TYPE: klass = mkr_cDocumentType; break;
    case LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT:
                                          klass = mkr_cDocumentFragment;      break;
    default:                              klass = mkr_cNode;      break;
    }

    mkr_node_data_t *nd;
    VALUE obj = TypedData_Make_Struct(klass, mkr_node_data_t, &mkr_node_type, nd);
    nd->node = node;
    nd->document = document;
    return obj;
}

lxb_dom_node_t *
mkr_node_unwrap(VALUE rb_node)
{
    if (rb_obj_is_kind_of(rb_node, mkr_cDocument)) {
        return (lxb_dom_node_t *)mkr_doc_unwrap(rb_node);
    }
    mkr_node_data_t *nd;
    TypedData_Get_Struct(rb_node, mkr_node_data_t, &mkr_node_type, nd);
    return nd->node;
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
/* name / type / content                                              */
/* ------------------------------------------------------------------ */

/*
 * Node name. Matches Nokogiri: lowercase tag name for HTML elements
 * (Lexbor lowercases during tokenization), and the un-prefixed DOM names
 * "text"/"comment"/"#cdata-section"/"document" for the other kinds.
 */
static VALUE
mkr_node_name(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    size_t len = 0;
    const lxb_char_t *name;

    switch (node->type) {
    case LXB_DOM_NODE_TYPE_ELEMENT:
        name = lxb_dom_element_qualified_name(lxb_dom_interface_element(node), &len);
        return name ? rb_utf8_str_new((const char *)name, len) : rb_str_new("", 0);
    case LXB_DOM_NODE_TYPE_ATTRIBUTE:
        name = lxb_dom_attr_qualified_name(lxb_dom_interface_attr(node), &len);
        return name ? rb_utf8_str_new((const char *)name, len) : rb_str_new("", 0);
    case LXB_DOM_NODE_TYPE_TEXT:
        return rb_utf8_str_new_cstr("text");
    case LXB_DOM_NODE_TYPE_COMMENT:
        return rb_utf8_str_new_cstr("comment");
    case LXB_DOM_NODE_TYPE_CDATA_SECTION:
        return rb_utf8_str_new_cstr("#cdata-section");
    case LXB_DOM_NODE_TYPE_DOCUMENT:
        return rb_utf8_str_new_cstr("document");
    default:
        name = lxb_dom_node_name(node, &len);
        return name ? rb_utf8_str_new((const char *)name, len) : rb_str_new("", 0);
    }
}

/* Numeric DOM node type (LXB_DOM_NODE_TYPE_*). */
static VALUE
mkr_node_get_type(VALUE self)
{
    return INT2NUM((int)mkr_node_unwrap(self)->type);
}

/*
 * DocumentType public / system identifiers (WHATWG DOM `publicId`/`systemId`).
 * Returns the String, or nil when the doctype carries no such identifier.
 * Lexbor represents a missing id inconsistently (NULL after `SYSTEM`, but an
 * empty string for a bare `<!DOCTYPE html>`), so we treat empty as absent and
 * return nil for both — matching Nokogiri (which also reports nil for an empty
 * or missing id). Defined only on Makiri::DocumentType, so the receiver is
 * always a doctype node; the guard is belt-and-suspenders.
 */
static VALUE
mkr_doctype_id(VALUE self, int system)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_DOCUMENT_TYPE) {
        return Qnil;
    }
    lxb_dom_document_type_t *dt = lxb_dom_interface_document_type(node);
    size_t len = 0;
    const lxb_char_t *id = system ? lxb_dom_document_type_system_id(dt, &len)
                                  : lxb_dom_document_type_public_id(dt, &len);
    return (id == NULL || len == 0) ? Qnil : rb_utf8_str_new((const char *)id, len);
}

static VALUE
mkr_doctype_public_id(VALUE self)
{
    return mkr_doctype_id(self, 0);
}

static VALUE
mkr_doctype_system_id(VALUE self)
{
    return mkr_doctype_id(self, 1);
}

/* Concatenated text content of this node and its descendants. The DOM spec
 * makes a Document's textContent null; we instead return the text of the root
 * element (matching the intuitive, Nokogiri-like Document#text). */
static VALUE
mkr_node_content(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type == LXB_DOM_NODE_TYPE_DOCUMENT) {
        node = lxb_dom_document_root((lxb_dom_document_t *)node);
        if (node == NULL) {
            return rb_utf8_str_new("", 0);
        }
    }

    /* Fast path for elements / fragments (the common case, incl. document text):
     * stream each descendant text/CDATA node's data straight into the Ruby
     * string, skipping lxb_dom_node_text_content's intermediate arena buffer and
     * the extra copy (and its destroy). Iterative pre-order so deep trees can't
     * blow the stack. Note: the dominant cost here is the descendant walk itself
     * — a single pass with rb_str_cat measured best (a two-pass sum+memcpy was
     * slower because it walked the tree twice). */
    if (node->type == LXB_DOM_NODE_TYPE_ELEMENT
        || node->type == LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT) {
        VALUE str = rb_utf8_str_new(NULL, 0);
        for (lxb_dom_node_t *n = node->first_child; n != NULL;) {
            if (n->type == LXB_DOM_NODE_TYPE_TEXT
                || n->type == LXB_DOM_NODE_TYPE_CDATA_SECTION) {
                const lexbor_str_t *d = &lxb_dom_interface_character_data(n)->data;
                if (d->data != NULL && d->length != 0) {
                    rb_str_cat(str, (const char *)d->data, (long)d->length);
                }
            }
            if (n->first_child != NULL) { n = n->first_child; continue; }
            while (n != node && n->next == NULL) { n = n->parent; }
            if (n == node) { break; }
            n = n->next;
        }
        return str;
    }

    /* Character-data and other node kinds keep the general (proven) path. */
    size_t len = 0;
    lxb_char_t *text = lxb_dom_node_text_content(node, &len);
    if (text == NULL) {
        return rb_utf8_str_new("", 0);
    }
    VALUE str = rb_utf8_str_new((const char *)text, len);
    lxb_dom_document_destroy_text(node->owner_document, text);
    return str;
}

/* ------------------------------------------------------------------ */
/* tree navigation                                                    */
/* ------------------------------------------------------------------ */

static VALUE
mkr_node_get_document(VALUE self)
{
    return mkr_node_document(self);
}

static VALUE
mkr_node_parent(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    VALUE document = mkr_node_document(self);

    /* Lexbor never links an attribute back to its element, so node->parent is
     * NULL for attributes. Resolve via the compat attr->owner index. */
    if (node->type == LXB_DOM_NODE_TYPE_ATTRIBUTE) {
        lxb_dom_node_t *owner =
            mkr_parsed_attr_owner(mkr_doc_parsed(document),
                                  lxb_dom_interface_attr(node));
        return mkr_wrap_node(owner, document);
    }

    return mkr_wrap_node(node->parent, document);
}

static VALUE
mkr_node_next(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    return mkr_wrap_node(node->next, mkr_node_document(self));
}

static VALUE
mkr_node_previous(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    return mkr_wrap_node(node->prev, mkr_node_document(self));
}

static VALUE
mkr_node_next_element(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self)->next;
    while (node != NULL && node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        node = node->next;
    }
    return mkr_wrap_node(node, mkr_node_document(self));
}

static VALUE
mkr_node_previous_element(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self)->prev;
    while (node != NULL && node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        node = node->prev;
    }
    return mkr_wrap_node(node, mkr_node_document(self));
}

/* First child node (any type), or nil. */
static VALUE
mkr_node_child(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    return mkr_wrap_node(node->first_child, mkr_node_document(self));
}

/* All child nodes as a NodeSet. */
static VALUE
mkr_node_children(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    VALUE document = mkr_node_document(self);
    VALUE set = mkr_node_set_new(document);
    for (lxb_dom_node_t *c = node->first_child; c != NULL; c = c->next) {
        mkr_node_set_push(set, c);
    }
    return set;
}

/* Child elements only, as a NodeSet. */
static VALUE
mkr_node_element_children(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    VALUE document = mkr_node_document(self);
    VALUE set = mkr_node_set_new(document);
    for (lxb_dom_node_t *c = node->first_child; c != NULL; c = c->next) {
        if (c->type == LXB_DOM_NODE_TYPE_ELEMENT) {
            mkr_node_set_push(set, c);
        }
    }
    return set;
}

/* Ancestor elements, nearest first (parent, grandparent, ... root). */
static VALUE
mkr_node_ancestors(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    VALUE document = mkr_node_document(self);
    VALUE set = mkr_node_set_new(document);
    for (lxb_dom_node_t *p = node->parent; p != NULL; p = p->parent) {
        if (p->type == LXB_DOM_NODE_TYPE_ELEMENT) {
            mkr_node_set_push(set, p);
        }
    }
    return set;
}

static VALUE
mkr_node_first_element_child(VALUE self)
{
    lxb_dom_node_t *c = mkr_node_unwrap(self)->first_child;
    while (c != NULL && c->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        c = c->next;
    }
    return mkr_wrap_node(c, mkr_node_document(self));
}

static VALUE
mkr_node_last_element_child(VALUE self)
{
    lxb_dom_node_t *c = mkr_node_unwrap(self)->last_child;
    while (c != NULL && c->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        c = c->prev;
    }
    return mkr_wrap_node(c, mkr_node_document(self));
}

/* ------------------------------------------------------------------ */
/* attributes (read-only)                                             */
/* ------------------------------------------------------------------ */

/* node[name] -> String or nil (nil when not an element or absent). */
static VALUE
mkr_node_aref(VALUE self, VALUE rb_name)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return Qnil;
    }

    VALUE name = rb_String(rb_name);
    const lxb_char_t *nm = (const lxb_char_t *)RSTRING_PTR(name);
    size_t nlen = (size_t)RSTRING_LEN(name);

    lxb_dom_element_t *el = lxb_dom_interface_element(node);
    if (!lxb_dom_element_has_attribute(el, nm, nlen)) {
        return Qnil;
    }

    size_t vlen = 0;
    const lxb_char_t *val = lxb_dom_element_get_attribute(el, nm, nlen, &vlen);
    return val ? rb_utf8_str_new((const char *)val, vlen) : rb_utf8_str_new("", 0);
}

/* node.key?(name) -> true/false */
static VALUE
mkr_node_has_key(VALUE self, VALUE rb_name)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return Qfalse;
    }
    VALUE name = rb_String(rb_name);
    lxb_dom_element_t *el = lxb_dom_interface_element(node);
    return lxb_dom_element_has_attribute(el,
                                         (const lxb_char_t *)RSTRING_PTR(name),
                                         (size_t)RSTRING_LEN(name))
               ? Qtrue : Qfalse;
}

/* node.keys -> [String, ...] of attribute names (document order). */
static VALUE
mkr_node_keys(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    VALUE ary = rb_ary_new();
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return ary;
    }
    lxb_dom_attr_t *attr =
        lxb_dom_element_first_attribute(lxb_dom_interface_element(node));
    while (attr != NULL) {
        size_t len = 0;
        const lxb_char_t *name = lxb_dom_attr_qualified_name(attr, &len);
        rb_ary_push(ary, name ? rb_utf8_str_new((const char *)name, len)
                              : rb_str_new("", 0));
        attr = lxb_dom_element_next_attribute(attr);
    }
    return ary;
}

/* node.values -> [String, ...] of attribute values (document order). */
static VALUE
mkr_node_values(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    VALUE ary = rb_ary_new();
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return ary;
    }
    lxb_dom_attr_t *attr =
        lxb_dom_element_first_attribute(lxb_dom_interface_element(node));
    while (attr != NULL) {
        size_t len = 0;
        const lxb_char_t *val = lxb_dom_attr_value(attr, &len);
        rb_ary_push(ary, val ? rb_utf8_str_new((const char *)val, len)
                             : rb_str_new("", 0));
        attr = lxb_dom_element_next_attribute(attr);
    }
    return ary;
}

/* element.attribute_nodes -> NodeSet of Attribute nodes (document order).
 * Empty for non-elements. These wrap the bare lxb_dom_attr_t; navigating back
 * with Attribute#parent goes through the compat attr->owner index. */
static VALUE
mkr_node_attribute_nodes(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    VALUE document = mkr_node_document(self);
    VALUE set = mkr_node_set_new(document);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return set;
    }
    lxb_dom_attr_t *attr =
        lxb_dom_element_first_attribute(lxb_dom_interface_element(node));
    while (attr != NULL) {
        mkr_node_set_push(set, lxb_dom_interface_node(attr));
        attr = lxb_dom_element_next_attribute(attr);
    }
    return set;
}

/* attr.value -> the attribute's value String. For non-attribute nodes, falls
 * back to text content (matching the loose Nokogiri-ish meaning of #value). */
static VALUE
mkr_node_value(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type == LXB_DOM_NODE_TYPE_ATTRIBUTE) {
        size_t len = 0;
        const lxb_char_t *val =
            lxb_dom_attr_value(lxb_dom_interface_attr(node), &len);
        return val ? rb_utf8_str_new((const char *)val, len)
                   : rb_utf8_str_new("", 0);
    }
    return mkr_node_content(self);
}

/* node.line -> 1-based source line, or nil when unknown.
 *
 * The line comes from the byte offset stamped onto the node at parse time
 * (source-location tracking) resolved against the document's line table.
 * Returns nil for nodes the tracker could not place (e.g. parser-inserted
 * implicit <html>/<head>/<body>, or any node when tracking was disabled). */
static VALUE
mkr_node_line(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    mkr_parsed_t   *p    = mkr_doc_parsed(mkr_node_document(self));
    size_t line = mkr_parsed_node_line(p, node);
    return line == 0 ? Qnil : ULONG2NUM(line);
}

/* ------------------------------------------------------------------ */
/* identity                                                           */
/* ------------------------------------------------------------------ */

/* Pointer identity: equal iff both wrap the same lxb_dom_node_t. */
static VALUE
mkr_node_equals(VALUE self, VALUE other)
{
    if (!rb_obj_is_kind_of(other, mkr_cNode)) {
        return Qfalse;
    }
    return mkr_node_unwrap(self) == mkr_node_unwrap(other) ? Qtrue : Qfalse;
}

/* Stable hash derived from the node pointer, so a == b implies a.hash ==
 * b.hash even across separately-created wrappers. */
static VALUE
mkr_node_hash(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    return ULL2NUM((unsigned long long)(uintptr_t)node);
}

void
mkr_init_node(void)
{
    rb_define_method(mkr_cNode, "name",       mkr_node_name,       0);
    rb_define_method(mkr_cNode, "node_type",  mkr_node_get_type,   0);
    rb_define_method(mkr_cNode, "content",    mkr_node_content,    0);
    rb_define_method(mkr_cNode, "text",       mkr_node_content,    0);
    rb_define_method(mkr_cNode, "inner_text", mkr_node_content,    0);

    rb_define_method(mkr_cNode, "document",   mkr_node_get_document, 0);
    rb_define_method(mkr_cNode, "parent",     mkr_node_parent,       0);
    rb_define_method(mkr_cNode, "next",             mkr_node_next,     0);
    rb_define_method(mkr_cNode, "next_sibling",     mkr_node_next,     0);
    rb_define_method(mkr_cNode, "previous",         mkr_node_previous, 0);
    rb_define_method(mkr_cNode, "previous_sibling", mkr_node_previous, 0);
    rb_define_method(mkr_cNode, "next_element",     mkr_node_next_element,     0);
    rb_define_method(mkr_cNode, "previous_element", mkr_node_previous_element, 0);

    rb_define_method(mkr_cNode, "child",                mkr_node_child,                0);
    rb_define_method(mkr_cNode, "children",             mkr_node_children,             0);
    rb_define_method(mkr_cNode, "element_children",     mkr_node_element_children,     0);
    rb_define_method(mkr_cNode, "elements",             mkr_node_element_children,     0);
    rb_define_method(mkr_cNode, "first_element_child",  mkr_node_first_element_child,  0);
    rb_define_method(mkr_cNode, "last_element_child",   mkr_node_last_element_child,   0);
    rb_define_method(mkr_cNode, "ancestors",            mkr_node_ancestors,            0);

    rb_define_method(mkr_cNode, "[]",     mkr_node_aref,    1);
    rb_define_method(mkr_cNode, "key?",   mkr_node_has_key, 1);
    rb_define_method(mkr_cNode, "keys",   mkr_node_keys,    0);
    rb_define_method(mkr_cNode, "values", mkr_node_values,  0);
    rb_define_method(mkr_cNode, "attribute_nodes", mkr_node_attribute_nodes, 0);
    rb_define_method(mkr_cNode, "value",  mkr_node_value,   0);
    rb_define_method(mkr_cNode, "line",   mkr_node_line,    0);

    rb_define_method(mkr_cNode, "==",   mkr_node_equals, 1);
    rb_define_method(mkr_cNode, "eql?", mkr_node_equals, 1);
    rb_define_method(mkr_cNode, "hash", mkr_node_hash,   0);

    /* DocumentType identifiers (WHATWG DOM names; external_id is the
     * Nokogiri-compatible alias for public_id). */
    rb_define_method(mkr_cDocumentType, "public_id",   mkr_doctype_public_id, 0);
    rb_define_method(mkr_cDocumentType, "external_id", mkr_doctype_public_id, 0);
    rb_define_method(mkr_cDocumentType, "system_id",   mkr_doctype_system_id, 0);
}
