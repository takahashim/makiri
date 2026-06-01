#include "glue.h"

#include <lexbor/html/parser.h>

/*
 * DOM mutation (v0.2). Thin wrappers over Lexbor's insert/remove/create
 * functions, with the safety checks Lexbor itself omits:
 *
 *   - same-document: a node can only be inserted into its own document
 *     (cross-document moves would splice foreign-arena pointers);
 *   - no cycles: a node cannot become a descendant of itself;
 *   - attribute nodes are not tree children.
 *
 * We never destroy detached nodes: the document arena owns all node memory and
 * frees it wholesale, and live Ruby wrappers may still point at a removed node.
 * `remove`/`unlink` therefore only detach.
 *
 * Any structural change drops the persistent attr->owner index so the next
 * XPath rebuilds it (and re-backfills attribute parents).
 */

static void
mkr_invalidate_index(VALUE node)
{
    mkr_parsed_attr_index_invalidate(mkr_doc_parsed(mkr_node_document(node)));
}

static lxb_dom_node_t *
mkr_arg_node(VALUE v)
{
    if (!rb_obj_is_kind_of(v, mkr_cNode)) {
        rb_raise(rb_eTypeError, "expected a Makiri::Node");
    }
    return mkr_node_unwrap(v);
}

/* Validate that `incoming` may be placed relative to `ref` and detach it from
 * any current parent (move semantics). Raises on the unsafe cases. */
static void
mkr_prepare_insert(lxb_dom_node_t *ref, lxb_dom_node_t *incoming)
{
    if (incoming->type == LXB_DOM_NODE_TYPE_ATTRIBUTE) {
        rb_raise(mkr_eError, "an attribute node cannot be inserted into the tree");
    }
    if (ref->owner_document != incoming->owner_document) {
        rb_raise(mkr_eError, "cannot move a node between documents");
    }
    /* incoming must not be an inclusive ancestor of ref. */
    for (lxb_dom_node_t *p = ref; p != NULL; p = p->parent) {
        if (p == incoming) {
            rb_raise(mkr_eError, "cannot insert a node into its own subtree");
        }
    }
    if (incoming->parent != NULL) {
        lxb_dom_node_remove(incoming);
    }
}

/* A document fragment is spliced in place of itself: its children are inserted
 * and the fragment node is left empty. */
static int
mkr_is_fragment(const lxb_dom_node_t *n)
{
    return n->type == LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT;
}

/* ------------------------------------------------------------------ */
/* tree mutation                                                      */
/* ------------------------------------------------------------------ */

/* node.add_child(child) -> child. Appends child as the last child. A document
 * fragment contributes its children rather than itself. */
static VALUE
mkr_node_add_child(VALUE self, VALUE rb_child)
{
    lxb_dom_node_t *parent = mkr_node_unwrap(self);
    lxb_dom_node_t *child  = mkr_arg_node(rb_child);
    mkr_prepare_insert(parent, child);
    if (mkr_is_fragment(child)) {
        lxb_dom_node_t *c;
        while ((c = child->first_child) != NULL) {
            lxb_dom_node_remove(c);
            lxb_dom_node_insert_child(parent, c);
        }
    } else {
        lxb_dom_node_insert_child(parent, child);
    }
    mkr_invalidate_index(self);
    return rb_child;
}

/* node << child -> node (chainable). */
static VALUE
mkr_node_append(VALUE self, VALUE rb_child)
{
    mkr_node_add_child(self, rb_child);
    return self;
}

static VALUE
mkr_node_add_previous_sibling(VALUE self, VALUE rb_node)
{
    lxb_dom_node_t *ref  = mkr_node_unwrap(self);
    lxb_dom_node_t *node = mkr_arg_node(rb_node);
    if (ref->parent == NULL) {
        rb_raise(mkr_eError, "cannot add a sibling to a node with no parent");
    }
    mkr_prepare_insert(ref, node);
    if (mkr_is_fragment(node)) {
        lxb_dom_node_t *c;
        while ((c = node->first_child) != NULL) {
            lxb_dom_node_remove(c);
            lxb_dom_node_insert_before(ref, c);
        }
    } else {
        lxb_dom_node_insert_before(ref, node);
    }
    mkr_invalidate_index(self);
    return rb_node;
}

static VALUE
mkr_node_add_next_sibling(VALUE self, VALUE rb_node)
{
    lxb_dom_node_t *ref  = mkr_node_unwrap(self);
    lxb_dom_node_t *node = mkr_arg_node(rb_node);
    if (ref->parent == NULL) {
        rb_raise(mkr_eError, "cannot add a sibling to a node with no parent");
    }
    mkr_prepare_insert(ref, node);
    if (mkr_is_fragment(node)) {
        lxb_dom_node_t *anchor = ref, *c;
        while ((c = node->first_child) != NULL) {
            lxb_dom_node_remove(c);
            lxb_dom_node_insert_after(anchor, c);
            anchor = c; /* keep document order after ref */
        }
    } else {
        lxb_dom_node_insert_after(ref, node);
    }
    mkr_invalidate_index(self);
    return rb_node;
}

/* node.remove / node.unlink -> node. Detaches from the tree (still usable). */
static VALUE
mkr_node_remove(VALUE self)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type == LXB_DOM_NODE_TYPE_ATTRIBUTE) {
        rb_raise(mkr_eError, "use delete(name) to remove an attribute");
    }
    if (node->parent != NULL) {
        lxb_dom_node_remove(node);
        mkr_invalidate_index(self);
    }
    return self;
}

/* node.replace(other) -> other. Puts other where node is, detaches node. */
static VALUE
mkr_node_replace(VALUE self, VALUE rb_other)
{
    lxb_dom_node_t *ref   = mkr_node_unwrap(self);
    lxb_dom_node_t *other = mkr_arg_node(rb_other);
    if (ref->parent == NULL) {
        rb_raise(mkr_eError, "cannot replace a node with no parent");
    }
    mkr_prepare_insert(ref, other);
    if (mkr_is_fragment(other)) {
        lxb_dom_node_t *c;
        while ((c = other->first_child) != NULL) {
            lxb_dom_node_remove(c);
            lxb_dom_node_insert_before(ref, c);
        }
    } else {
        lxb_dom_node_insert_before(ref, other);
    }
    lxb_dom_node_remove(ref);
    mkr_invalidate_index(self);
    return rb_other;
}

/* ------------------------------------------------------------------ */
/* attribute mutation                                                 */
/* ------------------------------------------------------------------ */

/* element[name] = value -> value */
static VALUE
mkr_node_aset(VALUE self, VALUE rb_name, VALUE rb_value)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        rb_raise(mkr_eError, "cannot set an attribute on a non-element node");
    }
    VALUE name  = rb_String(rb_name);
    VALUE value = rb_String(rb_value);
    mkr_check_text(name, "attribute name");
    mkr_check_text(value, "attribute value");
    lxb_dom_attr_t *attr = lxb_dom_element_set_attribute(
        lxb_dom_interface_element(node),
        (const lxb_char_t *)RSTRING_PTR(name), (size_t)RSTRING_LEN(name),
        (const lxb_char_t *)RSTRING_PTR(value), (size_t)RSTRING_LEN(value));
    if (attr == NULL) {
        rb_raise(mkr_eError, "failed to set attribute");
    }
    mkr_invalidate_index(self);
    return rb_value;
}

/* element.name = new_name -> new_name. Renames the element in place (identity
 * preserved): create a throwaway element with the new name so the document
 * interns it, copy its name fields onto this node, then discard it. */
static VALUE
mkr_node_set_name(VALUE self, VALUE rb_name)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        rb_raise(mkr_eError, "name= is only supported on elements");
    }
    VALUE name = rb_String(rb_name);
    mkr_check_text(name, "element name");
    lxb_dom_element_t *fresh = lxb_dom_document_create_element(
        node->owner_document,
        (const lxb_char_t *)RSTRING_PTR(name), (size_t)RSTRING_LEN(name), NULL);
    if (fresh == NULL) {
        rb_raise(mkr_eError, "failed to rename element");
    }

    lxb_dom_element_t *el = lxb_dom_interface_element(node);
    el->node.local_name = fresh->node.local_name;
    el->node.prefix     = fresh->node.prefix;
    el->node.ns         = fresh->node.ns;
    el->upper_name      = fresh->upper_name;
    el->qualified_name  = fresh->qualified_name;

    lxb_dom_node_destroy(lxb_dom_interface_node(fresh));
    return rb_name;
}

/* node.content = text -> text. DOM textContent setter: for an element this
 * replaces all children with a single text node; for a text/comment/cdata node
 * it sets the data. */
static VALUE
mkr_node_set_content(VALUE self, VALUE rb_text)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    VALUE text = rb_String(rb_text);
    mkr_check_text(text, "node content");
    if (lxb_dom_node_text_content_set(
            node, (const lxb_char_t *)RSTRING_PTR(text),
            (size_t)RSTRING_LEN(text)) != LXB_STATUS_OK) {
        rb_raise(mkr_eError, "failed to set node content");
    }
    mkr_invalidate_index(self);
    return rb_text;
}

/* element.delete(name) -> self. Removes the attribute if present. */
static VALUE
mkr_node_delete(VALUE self, VALUE rb_name)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return self;
    }
    VALUE name = rb_String(rb_name);
    mkr_check_text(name, "attribute name");
    lxb_dom_element_remove_attribute(
        lxb_dom_interface_element(node),
        (const lxb_char_t *)RSTRING_PTR(name), (size_t)RSTRING_LEN(name));
    mkr_invalidate_index(self);
    return self;
}

/* ------------------------------------------------------------------ */
/* inner_html= / outer_html= (fragment parse + import)                */
/* ------------------------------------------------------------------ */

/*
 * Parse `html` as a fragment in the context of `context_el` and splice the
 * imported nodes via `emit`. UTF-8 decoding (browser-compatible: invalid bytes
 * -> U+FFFD) and the import + <template>-content fixup are shared with the
 * DocumentFragment paths (mkr_sanitize_html_input / mkr_import_fragment_children
 * / mkr_emit_* in glue/ruby_doc.c). The transient parser is destroyed before
 * returning. Raises on parser or parse failure.
 */
static void
mkr_parse_fragment_into(lxb_dom_node_t *context_el, VALUE rb_html,
                        lxb_dom_document_t *doc,
                        void (*emit)(lxb_dom_node_t *imported, void *u), void *u)
{
    VALUE html = rb_String(rb_html);

    lxb_html_parser_t *parser = lxb_html_parser_create();
    if (parser == NULL || lxb_html_parser_init(parser) != LXB_STATUS_OK) {
        if (parser != NULL) {
            lxb_html_parser_destroy(parser);
        }
        rb_raise(mkr_eError, "failed to create HTML parser");
    }

    const lxb_char_t *hsrc;
    size_t            hlen;
    lxb_char_t       *owned;
    if (mkr_sanitize_html_input(html, &hsrc, &hlen, &owned) != 0) {
        lxb_html_parser_destroy(parser);
        rb_raise(mkr_eError, "out of memory decoding fragment HTML");
    }

    lxb_dom_node_t *frag = lxb_html_parse_fragment(
        parser, (lxb_html_element_t *)context_el, hsrc, hlen);
    free(owned); /* consumed by the parse; free on every subsequent path */
    if (frag == NULL) {
        lxb_html_parser_destroy(parser);
        rb_raise(mkr_eError, "failed to parse HTML fragment");
    }

    mkr_import_fragment_children(doc, frag, emit, u);

    /* Frees the transient fragment document; our imported copies live on. */
    lxb_html_parser_destroy(parser);
    RB_GC_GUARD(html);
}

/* element.inner_html = html -> html. Replaces the element's children. */
static VALUE
mkr_node_set_inner_html(VALUE self, VALUE rb_html)
{
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        rb_raise(mkr_eError, "inner_html= requires an element");
    }

    /* Detach existing children (arena reclaims them at document destroy). */
    lxb_dom_node_t *c;
    while ((c = node->first_child) != NULL) {
        lxb_dom_node_remove(c);
    }

    mkr_parse_fragment_into(node, rb_html, node->owner_document,
                            mkr_emit_append, node);
    mkr_invalidate_index(self);
    return rb_html;
}

/* node.outer_html = html -> html. Replaces the node itself with the parse. */
static VALUE
mkr_node_set_outer_html(VALUE self, VALUE rb_html)
{
    lxb_dom_node_t *node   = mkr_node_unwrap(self);
    lxb_dom_node_t *parent = node->parent;
    if (parent == NULL || parent->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        rb_raise(mkr_eError, "outer_html= requires a node with a parent element");
    }

    /* Parse in the parent's context, splice imported nodes before self. */
    mkr_parse_fragment_into(parent, rb_html, node->owner_document,
                            mkr_emit_before, node);
    lxb_dom_node_remove(node);
    mkr_invalidate_index(self);
    return rb_html;
}

/* ------------------------------------------------------------------ */
/* node creation (Document)                                           */
/* ------------------------------------------------------------------ */

static VALUE
mkr_doc_create_element(VALUE self, VALUE rb_name)
{
    lxb_dom_document_t *doc = mkr_doc_unwrap(self);
    VALUE name = rb_String(rb_name);
    mkr_check_text(name, "element name");
    lxb_dom_element_t *el = lxb_dom_document_create_element(
        doc, (const lxb_char_t *)RSTRING_PTR(name), (size_t)RSTRING_LEN(name), NULL);
    if (el == NULL) {
        rb_raise(mkr_eError, "failed to create element");
    }
    return mkr_wrap_node(lxb_dom_interface_node(el), self);
}

static VALUE
mkr_doc_create_text_node(VALUE self, VALUE rb_text)
{
    lxb_dom_document_t *doc = mkr_doc_unwrap(self);
    VALUE text = rb_String(rb_text);
    mkr_check_text(text, "text content");
    lxb_dom_text_t *t = lxb_dom_document_create_text_node(
        doc, (const lxb_char_t *)RSTRING_PTR(text), (size_t)RSTRING_LEN(text));
    if (t == NULL) {
        rb_raise(mkr_eError, "failed to create text node");
    }
    return mkr_wrap_node(lxb_dom_interface_node(t), self);
}

static VALUE
mkr_doc_create_comment(VALUE self, VALUE rb_text)
{
    lxb_dom_document_t *doc = mkr_doc_unwrap(self);
    VALUE text = rb_String(rb_text);
    mkr_check_text(text, "comment content");
    lxb_dom_comment_t *c = lxb_dom_document_create_comment(
        doc, (const lxb_char_t *)RSTRING_PTR(text), (size_t)RSTRING_LEN(text));
    if (c == NULL) {
        rb_raise(mkr_eError, "failed to create comment");
    }
    return mkr_wrap_node(lxb_dom_interface_node(c), self);
}

void
mkr_init_mutate(void)
{
    rb_define_method(mkr_cNode, "add_child",            mkr_node_add_child,            1);
    rb_define_method(mkr_cNode, "<<",                   mkr_node_append,               1);
    rb_define_method(mkr_cNode, "add_previous_sibling", mkr_node_add_previous_sibling, 1);
    rb_define_method(mkr_cNode, "before",               mkr_node_add_previous_sibling, 1);
    rb_define_method(mkr_cNode, "add_next_sibling",     mkr_node_add_next_sibling,     1);
    rb_define_method(mkr_cNode, "after",                mkr_node_add_next_sibling,     1);
    rb_define_method(mkr_cNode, "remove",               mkr_node_remove,               0);
    rb_define_method(mkr_cNode, "unlink",               mkr_node_remove,               0);
    rb_define_method(mkr_cNode, "replace",              mkr_node_replace,              1);

    rb_define_method(mkr_cNode, "inner_html=", mkr_node_set_inner_html, 1);
    rb_define_method(mkr_cNode, "outer_html=", mkr_node_set_outer_html, 1);

    rb_define_method(mkr_cNode, "[]=",              mkr_node_aset,   2);
    rb_define_method(mkr_cNode, "delete",           mkr_node_delete, 1);
    rb_define_method(mkr_cNode, "remove_attribute", mkr_node_delete, 1);
    rb_define_method(mkr_cNode, "content=",         mkr_node_set_content, 1);
    rb_define_method(mkr_cNode, "name=",            mkr_node_set_name,    1);

    rb_define_method(mkr_cDocument, "create_element",   mkr_doc_create_element,   1);
    rb_define_method(mkr_cDocument, "create_text_node", mkr_doc_create_text_node, 1);
    rb_define_method(mkr_cDocument, "create_comment",   mkr_doc_create_comment,   1);
}
