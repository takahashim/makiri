/* ruby_html_node.c - the HTML (Lexbor) node representation: wrapping an
 * lxb_dom_node_t into a Makiri::HTML::* leaf, the HTML node-pointer accessor, and
 * all the HTML node reader methods. The XML counterpart is ruby_xml_node.c; the
 * shared, representation-neutral node type system (the TypedData types plus the
 * kind-agnostic mkr_node_raw / mkr_node_id / mkr_node_document accessors) lives in
 * ruby_node.c. */
#include "glue.h"

#include <lexbor/ns/ns.h>   /* lxb_ns_by_id, LXB_NS__UNDEF (namespaceURI) */

/* ------------------------------------------------------------------ */
/* wrap / unwrap                                                      */
/* ------------------------------------------------------------------ */

VALUE
mkr_wrap_html_node(lxb_dom_node_t *node, VALUE document)
{
    if (node == NULL) {
        return Qnil;
    }

    /* The document node maps back onto the Ruby Document object. */
    if (node->type == LXB_DOM_NODE_TYPE_DOCUMENT) {
        return document;
    }

    /* An HTML (lxb_dom) node wraps to a Makiri::HTML::* leaf; the leaf carries the
     * lxb_dom reader methods via the included mkr_mHtmlNodeMethods module. XML
     * nodes get their own wrap path (Makiri::XML::* leaves) in step 2. An uncommon
     * DOM node type with no specific leaf (entity/notation - Lexbor's HTML parser
     * does not produce these) falls back to the generic Makiri::HTML::Node rather
     * than being misclassified as an Element. */
    VALUE klass;
    switch (node->type) {
    case LXB_DOM_NODE_TYPE_ELEMENT:       klass = mkr_cHtmlElement;   break;
    case LXB_DOM_NODE_TYPE_ATTRIBUTE:     klass = mkr_cHtmlAttr; break;
    case LXB_DOM_NODE_TYPE_TEXT:          klass = mkr_cHtmlText;      break;
    case LXB_DOM_NODE_TYPE_COMMENT:       klass = mkr_cHtmlComment;   break;
    case LXB_DOM_NODE_TYPE_CDATA_SECTION: klass = mkr_cHtmlCDATASection;     break;
    case LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION:
                                          klass = mkr_cHtmlProcessingInstruction; break;
    case LXB_DOM_NODE_TYPE_DOCUMENT_TYPE: klass = mkr_cHtmlDocumentType; break;
    case LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT:
                                          klass = mkr_cHtmlDocumentFragment;      break;
    default:                              klass = mkr_cHtmlNode;      break;
    }

    mkr_node_data_t *nd;
    VALUE obj = TypedData_Make_Struct(klass, mkr_node_data_t, &mkr_html_node_type, nd);
    nd->node = (mkr_raw_node_t *)node;
    nd->document = document;
    return obj;
}

/* The HTML node-pointer accessor: returns the lxb_dom_node_t for an HTML node or
 * HTML Document, and RAISES TypeError for an XML node/Document (TypedData_Get_Struct
 * checks mkr_html_node_type, which an XML node - wrapped under mkr_xml_node_type -
 * does not satisfy). Every HTML-glue site that dereferences a node or hands its
 * pointer to Lexbor MUST use this, for `self` and for arguments alike. */
lxb_dom_node_t *
mkr_html_node_unwrap(VALUE rb_node)
{
    if (rb_obj_is_kind_of(rb_node, mkr_cDocument)) {
        if (rb_obj_is_kind_of(rb_node, mkr_cXmlDocument)) {
            rb_raise(rb_eTypeError, "expected an HTML node, got a Makiri::XML::Document");
        }
        return (lxb_dom_node_t *)mkr_html_doc_unwrap(rb_node);
    }
    mkr_node_data_t *nd;
    TypedData_Get_Struct(rb_node, mkr_node_data_t, &mkr_html_node_type, nd);
    return (lxb_dom_node_t *)nd->node;
}

/* mkr_node_raw / mkr_node_id / mkr_node_document (the kind-agnostic accessors) and
 * the TypedData types live in ruby_node.c (the shared node core). */

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
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    size_t len = 0;
    const lxb_char_t *name;

    switch (node->type) {
    case LXB_DOM_NODE_TYPE_ELEMENT:
        name = lxb_dom_element_qualified_name(lxb_dom_interface_element(node), &len);
        return mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)name, len));
    case LXB_DOM_NODE_TYPE_ATTRIBUTE:
        name = lxb_dom_attr_qualified_name(lxb_dom_interface_attr(node), &len);
        return mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)name, len));
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
        return mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)name, len));
    }
}

/* ------------------------------------------------------------------ */
/* namespace (WHATWG DOM Element/Attr: namespaceURI/prefix/localName)  */
/* ------------------------------------------------------------------ */

/*
 * Local name (DOM `localName`): the name without any prefix - "div" for
 * <div>, "path" for an SVG <path>, "href" for an xlink:href attribute.
 * Defined on Element and Attribute only; nil for the other node kinds (the DOM
 * gives a Text/Comment/Document no localName).
 */
static VALUE
mkr_node_local_name(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    size_t len = 0;
    const lxb_char_t *name;

    switch (node->type) {
    case LXB_DOM_NODE_TYPE_ELEMENT:
        name = lxb_dom_element_local_name(lxb_dom_interface_element(node), &len);
        break;
    case LXB_DOM_NODE_TYPE_ATTRIBUTE: {
        /* The case-preserved local name is the suffix of the qualified name;
         * Lexbor's stored local_name is lower-cased even when the qualified name
         * keeps its case (set_attribute_ns is case-sensitive). */
        lxb_dom_attr_t *at = lxb_dom_interface_attr(node);
        size_t qlen = 0, llen = 0;
        const lxb_char_t *q = lxb_dom_attr_qualified_name(at, &qlen);
        (void) lxb_dom_attr_local_name(at, &llen);
        if (q != NULL && qlen >= llen) {
            name = q + (qlen - llen);
            len = llen;
        }
        else {
            name = lxb_dom_attr_local_name(at, &len);
        }
        break;
    }
    default:
        return Qnil;
    }
    return mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)name, len));
}

/*
 * Namespace prefix (DOM `prefix`): nil unless the qualified name is
 * `prefix:local` - typically nil for HTML5-parsed content. Derived from the
 * qualified-vs-local length (qualified == prefix ":" local), so a colon inside
 * a local name can't be mistaken for a separator. Element/Attribute only.
 */
static VALUE
mkr_node_prefix(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    const lxb_char_t *q = NULL;
    size_t qlen = 0, llen = 0;

    switch (node->type) {
    case LXB_DOM_NODE_TYPE_ELEMENT: {
        lxb_dom_element_t *el = lxb_dom_interface_element(node);
        q = lxb_dom_element_qualified_name(el, &qlen);
        (void) lxb_dom_element_local_name(el, &llen);
        break;
    }
    case LXB_DOM_NODE_TYPE_ATTRIBUTE: {
        lxb_dom_attr_t *at = lxb_dom_interface_attr(node);
        q = lxb_dom_attr_qualified_name(at, &qlen);
        (void) lxb_dom_attr_local_name(at, &llen);
        break;
    }
    default:
        return Qnil;
    }
    if (q == NULL || qlen <= llen + 1) {   /* no "prefix:" segment */
        return Qnil;
    }
    return mkr_ruby_str_from_borrowed(
        mkr_borrowed_text((const char *)q, qlen - llen - 1));
}

/*
 * The fixed namespaces the HTML parser assigns to foreign-content attributes by
 * prefix (the "adjust foreign attributes" step). Lexbor tags an attribute node
 * with its *element's* ns rather than the attribute's own, so an attribute's
 * namespaceURI is resolved from its prefix here, not from node->ns. Returns
 * NULL (=> DOM null) for any other prefix.
 */
static const char *
mkr_attr_ns_for_prefix(const char *p, size_t n)
{
    if (n == 5 && memcmp(p, "xlink", 5) == 0) return "http://www.w3.org/1999/xlink";
    if (n == 3 && memcmp(p, "xml",   3) == 0) return "http://www.w3.org/XML/1998/namespace";
    if (n == 5 && memcmp(p, "xmlns", 5) == 0) return "http://www.w3.org/2000/xmlns/";
    return NULL;
}

/*
 * Namespace URI (DOM `namespaceURI`).
 *
 * Element: resolved from node->ns, so - DOM-faithfully - an HTML element is in
 * the XHTML namespace ("http://www.w3.org/1999/xhtml"), not nil (an HTML
 * element is never namespaceless; this is what browsers' DOM and `namespace-uri()`
 * return). SVG/MathML elements get their own URI; nil only when truly
 * unnamespaced (LXB_NS__UNDEF).
 *
 * Attribute: nil for an unprefixed attribute (class, id, ...); for a prefixed
 * one, the parser-assigned foreign-content namespace keyed on the prefix
 * (xlink/xml/xmlns), else nil.
 *
 * Other node kinds: nil.
 */
static VALUE
mkr_node_namespace_uri(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);

    if (node->type == LXB_DOM_NODE_TYPE_ELEMENT) {
        if (node->ns == LXB_NS__UNDEF) {
            return Qnil;
        }
        lxb_dom_document_t *doc = node->owner_document;
        if (doc == NULL || doc->ns == NULL) {
            return Qnil;
        }
        size_t len = 0;
        const lxb_char_t *uri = lxb_ns_by_id(doc->ns, node->ns, &len);
        if (uri == NULL || len == 0) {
            return Qnil;
        }
        return mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)uri, len));
    }

    if (node->type == LXB_DOM_NODE_TYPE_ATTRIBUTE) {
        lxb_dom_attr_t *at = lxb_dom_interface_attr(node);

        /* An attribute set via set_attribute_ns records its OWN namespace on the
         * attr node - distinguishable because it differs from the owner element's
         * ns (a normally-set/parsed attr inherits the element's). Resolve it from
         * the interned id; LXB_NS__UNDEF (set by set_attribute_ns(nil, ...)) is
         * the null namespace. */
        if (at->owner != NULL && node->ns != at->owner->node.ns) {
            if (node->ns == LXB_NS__UNDEF) {
                return Qnil;
            }
            lxb_dom_document_t *doc = node->owner_document;
            if (doc != NULL && doc->ns != NULL) {
                size_t len = 0;
                const lxb_char_t *uri = lxb_ns_by_id(doc->ns, node->ns, &len);
                if (uri != NULL && len != 0) {
                    return mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)uri, len));
                }
            }
            return Qnil;
        }

        size_t qlen = 0, llen = 0;
        const lxb_char_t *q = lxb_dom_attr_qualified_name(at, &qlen);
        (void) lxb_dom_attr_local_name(at, &llen);
        if (q == NULL || qlen <= llen + 1) {
            return Qnil;   /* unprefixed attribute => no namespace */
        }
        const char *uri = mkr_attr_ns_for_prefix((const char *)q, qlen - llen - 1);
        return uri ? rb_utf8_str_new_cstr(uri) : Qnil;
    }

    return Qnil;
}

/*
 * Element#tag_name (DOM `tagName`): the qualified name, uppercased for an HTML
 * element in an HTML document ("DIV"), as the DOM specifies - unlike #name,
 * which is the lowercase qualified name. SVG/MathML elements keep their case.
 * nil for non-element nodes.
 */
static VALUE
mkr_node_tag_name(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return Qnil;
    }
    size_t len = 0;
    const lxb_char_t *name =
        lxb_dom_element_tag_name(lxb_dom_interface_element(node), &len);
    if (name == NULL) {
        return Qnil;
    }
    return mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)name, len));
}

/*
 * ProcessingInstruction#target (DOM `target`): the PI's target name
 * (the "xml" in <?xml ...?>). nil for non-PI nodes. The PI's data is read via
 * #content / #text like any character-data node.
 */
static VALUE
mkr_node_pi_target(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION) {
        return Qnil;
    }
    size_t len = 0;
    const lxb_char_t *t = lxb_dom_processing_instruction_target(
        lxb_dom_interface_processing_instruction(node), &len);
    return mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)t, len));
}

/* Numeric DOM node type (LXB_DOM_NODE_TYPE_*). */
static VALUE
mkr_node_get_type(VALUE self)
{
    return INT2NUM((int)mkr_html_node_unwrap(self)->type);
}

/*
 * DocumentType public / system identifiers (WHATWG DOM `publicId`/`systemId`).
 * Returns the String, or nil when the doctype carries no such identifier.
 * Lexbor represents a missing id inconsistently (NULL after `SYSTEM`, but an
 * empty string for a bare `<!DOCTYPE html>`), so we treat empty as absent and
 * return nil for both - matching Nokogiri (which also reports nil for an empty
 * or missing id). Defined only on Makiri::DocumentType, so the receiver is
 * always a doctype node; the guard is belt-and-suspenders.
 */
static VALUE
mkr_doctype_id(VALUE self, int system)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_DOCUMENT_TYPE) {
        return Qnil;
    }
    lxb_dom_document_type_t *dt = lxb_dom_interface_document_type(node);
    size_t len = 0;
    const lxb_char_t *id = system ? lxb_dom_document_type_system_id(dt, &len)
                                  : lxb_dom_document_type_public_id(dt, &len);
    return (id == NULL || len == 0)
             ? Qnil
             : mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)id, len));
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

/*
 * A <template> element's "template contents" - the separate DocumentFragment
 * the HTML parser fills instead of making the parsed nodes children of the
 * <template> (WHATWG DOM `HTMLTemplateElement.content`; browsers behave the
 * same: template.children is empty, template.content holds the nodes). Lexbor
 * stores it on the template interface; we surface it as a Makiri::DocumentFragment
 * so it can be traversed/queried (`tpl.content_fragment.css("p")`).
 *
 * Returns nil for any node that is not an HTML <template>. Note: CSS/XPath over
 * the *template element itself* deliberately do NOT descend into the content
 * (matching the DOM, and unavoidable for CSS since it runs Lexbor's selector
 * engine over the real tree) - query the fragment instead.
 */
static VALUE
mkr_node_content_fragment(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT
        || node->local_name != LXB_TAG_TEMPLATE
        || node->ns != LXB_NS_HTML) {
        return Qnil;
    }
    lxb_dom_document_fragment_t *content = lxb_html_interface_template(node)->content;
    if (content == NULL) {
        return Qnil;
    }
    return mkr_wrap_html_node((lxb_dom_node_t *)content, mkr_node_document(self));
}

/* Concatenated text content of this node and its descendants. The DOM spec
 * makes a Document's textContent null; we instead return the text of the root
 * element (matching the intuitive, Nokogiri-like Document#text). */
static VALUE
mkr_node_content(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    if (node->type == LXB_DOM_NODE_TYPE_DOCUMENT) {
        node = lxb_dom_document_root((lxb_dom_document_t *)node);
        if (node == NULL) {
            return rb_utf8_str_new("", 0);
        }
    }

    /* Fast path for elements / fragments (the common case, incl. document text).
     *
     * Preferred: the per-document text index (lexbor_compat/text_index.c) maps
     * this node to the contiguous, document-order run of its descendants' text
     * slices, so we serve a single pre-sized memcpy run with no per-extraction
     * tree walk - the walk is otherwise the dominant, cache-bound cost. Built
     * lazily on first use and dropped on any mutation, so a slice can never
     * point at reallocated/detached storage.
     *
     * Fallback (index unavailable - node outside the indexed tree, e.g. a
     * fragment, or a build OOM): stream each descendant text/CDATA node's data
     * straight into the Ruby string via an iterative pre-order walk (stack-safe;
     * skips Lexbor's intermediate arena buffer + copy). */
    if (node->type == LXB_DOM_NODE_TYPE_ELEMENT
        || node->type == LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT) {
        mkr_parsed_t *parsed = mkr_doc_parsed(mkr_node_document(self));
        const mkr_borrowed_text_t *slices;
        size_t nslices, total;
        if (parsed != NULL
            && mkr_parsed_text_slices(parsed, node, &slices, &nslices, &total)) {
            return mkr_ruby_str_from_slices(slices, nslices, total);
        }

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
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    VALUE document = mkr_node_document(self);

    /* Lexbor never links an attribute back to its element, so node->parent is
     * NULL for attributes. Resolve via the compat attr->owner index. */
    if (node->type == LXB_DOM_NODE_TYPE_ATTRIBUTE) {
        lxb_dom_node_t *owner =
            mkr_parsed_attr_owner(mkr_doc_parsed(document),
                                  lxb_dom_interface_attr(node));
        return mkr_wrap_html_node(owner, document);
    }

    return mkr_wrap_html_node(node->parent, document);
}

static VALUE
mkr_node_next(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    return mkr_wrap_html_node(node->next, mkr_node_document(self));
}

static VALUE
mkr_node_previous(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    return mkr_wrap_html_node(node->prev, mkr_node_document(self));
}

static VALUE
mkr_node_next_element(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self)->next;
    while (node != NULL && node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        node = node->next;
    }
    return mkr_wrap_html_node(node, mkr_node_document(self));
}

static VALUE
mkr_node_previous_element(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self)->prev;
    while (node != NULL && node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        node = node->prev;
    }
    return mkr_wrap_html_node(node, mkr_node_document(self));
}

/* First child node (any type), or nil. */
static VALUE
mkr_node_child(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    return mkr_wrap_html_node(node->first_child, mkr_node_document(self));
}

/* All child nodes as a NodeSet. */
static VALUE
mkr_node_children(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    VALUE document = mkr_node_document(self);
    VALUE set = mkr_node_set_new(document);
    for (lxb_dom_node_t *c = node->first_child; c != NULL; c = c->next) {
        mkr_node_set_push(set, (mkr_raw_node_t *)c);
    }
    return set;
}

/* Child elements only, as a NodeSet. */
static VALUE
mkr_node_element_children(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    VALUE document = mkr_node_document(self);
    VALUE set = mkr_node_set_new(document);
    for (lxb_dom_node_t *c = node->first_child; c != NULL; c = c->next) {
        if (c->type == LXB_DOM_NODE_TYPE_ELEMENT) {
            mkr_node_set_push(set, (mkr_raw_node_t *)c);
        }
    }
    return set;
}

/* Ancestor elements, nearest first (parent, grandparent, ... root). */
static VALUE
mkr_node_ancestors(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    VALUE document = mkr_node_document(self);
    VALUE set = mkr_node_set_new(document);
    for (lxb_dom_node_t *p = node->parent; p != NULL; p = p->parent) {
        if (p->type == LXB_DOM_NODE_TYPE_ELEMENT) {
            mkr_node_set_push(set, (mkr_raw_node_t *)p);
        }
    }
    return set;
}

static VALUE
mkr_node_first_element_child(VALUE self)
{
    lxb_dom_node_t *c = mkr_html_node_unwrap(self)->first_child;
    while (c != NULL && c->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        c = c->next;
    }
    return mkr_wrap_html_node(c, mkr_node_document(self));
}

static VALUE
mkr_node_last_element_child(VALUE self)
{
    lxb_dom_node_t *c = mkr_html_node_unwrap(self)->last_child;
    while (c != NULL && c->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        c = c->prev;
    }
    return mkr_wrap_html_node(c, mkr_node_document(self));
}

/* ------------------------------------------------------------------ */
/* attributes (read-only)                                             */
/* ------------------------------------------------------------------ */

/* node[name] -> String or nil (nil when not an element or absent). */
static VALUE
mkr_node_aref(VALUE self, VALUE rb_name)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return Qnil;
    }

    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "attribute name");
    const lxb_char_t *nm = (const lxb_char_t *)nv.ptr;
    size_t nlen = nv.len;

    lxb_dom_element_t *el = lxb_dom_interface_element(node);
    if (!lxb_dom_element_has_attribute(el, nm, nlen)) {
        return Qnil;
    }

    size_t vlen = 0;
    const lxb_char_t *val = lxb_dom_element_get_attribute(el, nm, nlen, &vlen);
    RB_GC_GUARD(nv.value);
    return mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)val, vlen));
}

/* node.key?(name) -> true/false */
static VALUE
mkr_node_has_key(VALUE self, VALUE rb_name)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return Qfalse;
    }
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "attribute name");
    lxb_dom_element_t *el = lxb_dom_interface_element(node);
    bool has = lxb_dom_element_has_attribute(el, (const lxb_char_t *)nv.ptr, nv.len);
    RB_GC_GUARD(nv.value);
    return has ? Qtrue : Qfalse;
}

/* node.keys -> [String, ...] of attribute names (document order). */
static VALUE
mkr_node_keys(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    VALUE ary = rb_ary_new();
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return ary;
    }
    lxb_dom_attr_t *attr =
        lxb_dom_element_first_attribute(lxb_dom_interface_element(node));
    while (attr != NULL) {
        size_t len = 0;
        const lxb_char_t *name = lxb_dom_attr_qualified_name(attr, &len);
        rb_ary_push(ary, mkr_ruby_str_from_borrowed(
                             mkr_borrowed_text((const char *)name, len)));
        attr = lxb_dom_element_next_attribute(attr);
    }
    return ary;
}

/* node.values -> [String, ...] of attribute values (document order). */
static VALUE
mkr_node_values(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    VALUE ary = rb_ary_new();
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return ary;
    }
    lxb_dom_attr_t *attr =
        lxb_dom_element_first_attribute(lxb_dom_interface_element(node));
    while (attr != NULL) {
        size_t len = 0;
        const lxb_char_t *val = lxb_dom_attr_value(attr, &len);
        rb_ary_push(ary, mkr_ruby_str_from_borrowed(
                             mkr_borrowed_text((const char *)val, len)));
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
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    VALUE document = mkr_node_document(self);
    VALUE set = mkr_node_set_new(document);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return set;
    }
    lxb_dom_attr_t *attr =
        lxb_dom_element_first_attribute(lxb_dom_interface_element(node));
    while (attr != NULL) {
        mkr_node_set_push(set, (mkr_raw_node_t *)lxb_dom_interface_node(attr));
        attr = lxb_dom_element_next_attribute(attr);
    }
    return set;
}

/* attr.value -> the attribute's value String. For non-attribute nodes, falls
 * back to text content (matching the loose Nokogiri-ish meaning of #value). */
static VALUE
mkr_node_value(VALUE self)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    if (node->type == LXB_DOM_NODE_TYPE_ATTRIBUTE) {
        size_t len = 0;
        const lxb_char_t *val =
            lxb_dom_attr_value(lxb_dom_interface_attr(node), &len);
        return mkr_ruby_str_from_borrowed(mkr_borrowed_text((const char *)val, len));
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
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
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
    /* Identity by pointer, kind-agnostic (an HTML node is simply never equal to an
     * XML node) - mkr_node_id never dereferences, so comparing across
     * representations is safe. */
    return mkr_node_id(self) == mkr_node_id(other) ? Qtrue : Qfalse;
}

/* Distance from `n` to the root (a node with no parent). */
static size_t
mkr_node_depth(const lxb_dom_node_t *n)
{
    size_t d = 0;
    for (const lxb_dom_node_t *p = n->parent; p != NULL; p = p->parent) {
        d++;
    }
    return d;
}

/*
 * Node#<=> : document (pre-order) position, so an array of nodes can be sorted.
 * Returns -1 / 0 / 1, or nil when the nodes are not comparable: a non-node,
 * different documents or detached subtrees (no common root), or an attribute
 * node (attributes are not in the first_child/next chain, so their order is not
 * defined here). Included via Comparable, which gives <, >, between?, etc.
 */
static VALUE
mkr_node_spaceship(VALUE self, VALUE other)
{
    if (!rb_obj_is_kind_of(other, mkr_cNode)
        || rb_obj_is_kind_of(mkr_node_document(other), mkr_cXmlDocument)) {
        return Qnil;   /* not a node, or an XML node - never order-comparable to HTML */
    }
    lxb_dom_node_t *a = mkr_html_node_unwrap(self);
    lxb_dom_node_t *b = mkr_html_node_unwrap(other);
    if (a == b) {
        return INT2FIX(0);
    }
    if (a->type == LXB_DOM_NODE_TYPE_ATTRIBUTE
        || b->type == LXB_DOM_NODE_TYPE_ATTRIBUTE
        || a->owner_document != b->owner_document) {
        return Qnil;
    }

    size_t da = mkr_node_depth(a), db = mkr_node_depth(b);
    lxb_dom_node_t *pa = a, *pb = b;

    /* Raise the deeper node to the other's depth; if it lands on the other,
     * that other is an ancestor and so comes first in pre-order. */
    if (da > db) {
        for (size_t k = 0; k < da - db; k++) pa = pa->parent;
        if (pa == b) return INT2FIX(1);   /* b is an ancestor of a */
    } else if (db > da) {
        for (size_t k = 0; k < db - da; k++) pb = pb->parent;
        if (pb == a) return INT2FIX(-1);  /* a is an ancestor of b */
    }

    /* Climb both until they share a parent (the lowest common ancestor). */
    while (pa->parent != pb->parent) {
        if (pa->parent == NULL || pb->parent == NULL) {
            return Qnil;                  /* different trees */
        }
        pa = pa->parent;
        pb = pb->parent;
    }
    if (pa->parent == NULL) {
        return Qnil;                      /* two distinct roots */
    }

    /* pa and pb are distinct siblings: earlier in the child list comes first. */
    for (lxb_dom_node_t *c = pa->parent->first_child; c != NULL; c = c->next) {
        if (c == pa) return INT2FIX(-1);
        if (c == pb) return INT2FIX(1);
    }
    return Qnil; /* unreachable for a well-formed tree */
}

/* Nokogiri-compatible identity: the underlying lxb_dom_node_t pointer as an
 * Integer. Stable for the node's lifetime and unique among currently-live
 * nodes; a freed-then-reallocated node may reuse an address (same caveat as
 * Nokogiri::XML::Node#pointer_id). a.pointer_id == b.pointer_id iff a.eql?(b). */
static VALUE
mkr_node_pointer_id(VALUE self)
{
    return ULL2NUM((unsigned long long)mkr_node_id(self));
}

/* Stable hash derived from the node pointer, so a == b implies a.hash ==
 * b.hash even across separately-created wrappers. Shares the pointer value
 * with #pointer_id. */
static VALUE
mkr_node_hash(VALUE self)
{
    return mkr_node_pointer_id(self);
}

void
mkr_init_node(void)
{
    rb_define_method(mkr_mHtmlNodeMethods, "name",          mkr_node_name,          0);
    rb_define_method(mkr_mHtmlNodeMethods, "namespace_uri", mkr_node_namespace_uri, 0);
    rb_define_method(mkr_mHtmlNodeMethods, "prefix",        mkr_node_prefix,        0);
    rb_define_method(mkr_mHtmlNodeMethods, "local_name",    mkr_node_local_name,    0);
    rb_define_method(mkr_mHtmlNodeMethods, "tag_name",      mkr_node_tag_name,      0);
    rb_define_method(mkr_mHtmlNodeMethods, "target",        mkr_node_pi_target,     0);
    rb_define_method(mkr_mHtmlNodeMethods, "node_type",  mkr_node_get_type,   0);
    rb_define_method(mkr_mHtmlNodeMethods, "content",    mkr_node_content,    0);
    rb_define_method(mkr_mHtmlNodeMethods, "text",       mkr_node_content,    0);
    rb_define_method(mkr_mHtmlNodeMethods, "inner_text", mkr_node_content,    0);

    rb_define_method(mkr_mHtmlNodeMethods, "document",   mkr_node_get_document, 0);
    rb_define_method(mkr_mHtmlNodeMethods, "parent",     mkr_node_parent,       0);
    rb_define_method(mkr_mHtmlNodeMethods, "next",             mkr_node_next,     0);
    rb_define_method(mkr_mHtmlNodeMethods, "next_sibling",     mkr_node_next,     0);
    rb_define_method(mkr_mHtmlNodeMethods, "previous",         mkr_node_previous, 0);
    rb_define_method(mkr_mHtmlNodeMethods, "previous_sibling", mkr_node_previous, 0);
    rb_define_method(mkr_mHtmlNodeMethods, "next_element",     mkr_node_next_element,     0);
    rb_define_method(mkr_mHtmlNodeMethods, "previous_element", mkr_node_previous_element, 0);

    rb_define_method(mkr_mHtmlNodeMethods, "child",                mkr_node_child,                0);
    rb_define_method(mkr_mHtmlNodeMethods, "children",             mkr_node_children,             0);
    rb_define_method(mkr_mHtmlNodeMethods, "element_children",     mkr_node_element_children,     0);
    rb_define_method(mkr_mHtmlNodeMethods, "elements",             mkr_node_element_children,     0);
    rb_define_method(mkr_mHtmlNodeMethods, "first_element_child",  mkr_node_first_element_child,  0);
    rb_define_method(mkr_mHtmlNodeMethods, "last_element_child",   mkr_node_last_element_child,   0);
    rb_define_method(mkr_mHtmlNodeMethods, "ancestors",            mkr_node_ancestors,            0);

    rb_define_method(mkr_mHtmlNodeMethods, "[]",     mkr_node_aref,    1);
    rb_define_method(mkr_mHtmlNodeMethods, "key?",   mkr_node_has_key, 1);
    rb_define_method(mkr_mHtmlNodeMethods, "keys",   mkr_node_keys,    0);
    rb_define_method(mkr_mHtmlNodeMethods, "values", mkr_node_values,  0);
    rb_define_method(mkr_mHtmlNodeMethods, "attribute_nodes", mkr_node_attribute_nodes, 0);
    rb_define_method(mkr_mHtmlNodeMethods, "value",  mkr_node_value,   0);
    rb_define_method(mkr_mHtmlNodeMethods, "line",   mkr_node_line,    0);

    rb_define_method(mkr_mHtmlNodeMethods, "==",   mkr_node_equals, 1);
    rb_define_method(mkr_mHtmlNodeMethods, "eql?", mkr_node_equals, 1);
    rb_define_method(mkr_mHtmlNodeMethods, "<=>",  mkr_node_spaceship, 1);
    rb_define_method(mkr_mHtmlNodeMethods, "hash", mkr_node_hash,   0);
    rb_define_method(mkr_mHtmlNodeMethods, "pointer_id", mkr_node_pointer_id, 0);
    rb_define_method(mkr_mHtmlNodeMethods, "clone_node", mkr_node_clone_node, -1);

    /* DocumentType identifiers (WHATWG DOM names; external_id is the
     * Nokogiri-compatible alias for public_id). */
    rb_define_method(mkr_cHtmlDocumentType, "public_id",   mkr_doctype_public_id, 0);
    rb_define_method(mkr_cHtmlDocumentType, "external_id", mkr_doctype_public_id, 0);
    rb_define_method(mkr_cHtmlDocumentType, "system_id",   mkr_doctype_system_id, 0);

    /* <template> contents (WHATWG DOM HTMLTemplateElement.content). */
    rb_define_method(mkr_cHtmlElement, "content_fragment", mkr_node_content_fragment, 0);
}
