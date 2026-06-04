#include "glue.h"

#include <lexbor/html/parser.h>
#include <lexbor/ns/ns.h>

/* Exported by lexbor but omitted from its public headers. lxb_ns_append interns
 * a namespace URI in the document's ns table; lxb_dom_attr_set_name_ns names an
 * attribute from (namespace, qualified name), splitting prefix/local and
 * interning the namespace. */
extern const lxb_ns_data_t *
lxb_ns_append(lexbor_hash_t *hash, const lxb_char_t *link, size_t length);
extern lxb_status_t
lxb_dom_attr_set_name_ns(lxb_dom_attr_t *attr, const lxb_char_t *link,
                         size_t link_length, const lxb_char_t *name,
                         size_t name_length, bool to_lowercase);

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
 * Any structural change drops the persistent DOM index (attr->owner +
 * element-by-tag) and the text index so the next query rebuilds them.
 */

static void
mkr_invalidate_index(VALUE node)
{
    mkr_parsed_t *p = mkr_doc_parsed(mkr_node_document(node));
    mkr_parsed_dom_index_invalidate(p);
    mkr_parsed_text_index_invalidate(p);
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

/* Every tree / attribute mutation unwraps `self` through here first: a node the
 * caller has frozen (Ruby's Object#freeze) is immutable, so raise FrozenError
 * rather than silently editing it. Read accessors keep using mkr_node_unwrap. */
static lxb_dom_node_t *
mkr_node_unwrap_mutable(VALUE self)
{
    rb_check_frozen(self);
    return mkr_node_unwrap(self);
}

/* node.add_child(child) -> child. Appends child as the last child. A document
 * fragment contributes its children rather than itself. */
static VALUE
mkr_node_add_child(VALUE self, VALUE rb_child)
{
    lxb_dom_node_t *parent = mkr_node_unwrap_mutable(self);
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
    lxb_dom_node_t *ref  = mkr_node_unwrap_mutable(self);
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
    lxb_dom_node_t *ref  = mkr_node_unwrap_mutable(self);
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
    lxb_dom_node_t *node = mkr_node_unwrap_mutable(self);
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
    lxb_dom_node_t *ref   = mkr_node_unwrap_mutable(self);
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
    lxb_dom_node_t *node = mkr_node_unwrap_mutable(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        rb_raise(mkr_eError, "cannot set an attribute on a non-element node");
    }
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "attribute name");
    mkr_ruby_borrowed_text_t vv = mkr_ruby_verified_text(rb_value, "attribute value");
    lxb_dom_attr_t *attr = lxb_dom_element_set_attribute(
        lxb_dom_interface_element(node),
        (const lxb_char_t *)nv.ptr, nv.len,
        (const lxb_char_t *)vv.ptr, vv.len);
    RB_GC_GUARD(nv.value);
    RB_GC_GUARD(vv.value);
    if (attr == NULL) {
        rb_raise(mkr_eError, "failed to set attribute");
    }
    mkr_invalidate_index(self);
    return rb_value;
}

/* An attribute's OWN namespace id: the one recorded by set_attribute_ns (which
 * differs from the owner element's), else the null namespace — a normally-set or
 * parsed attribute inherits the element's ns, which for matching purposes is the
 * null namespace (an unprefixed attribute is namespaceless). */
static lxb_ns_id_t
mkr_attr_own_ns(const lxb_dom_attr_t *at)
{
    if (at->owner != NULL && at->node.ns != at->owner->node.ns) {
        return at->node.ns;
    }
    return LXB_NS__UNDEF;
}

/* Find the attribute on `el` matching (ns_id, local_name) case-sensitively — the
 * DOM keys attributes on (namespace, local name), so two with the same qualified
 * name but different namespaces coexist (unlike Lexbor's by-qualified-name,
 * case-insensitive-for-HTML lookup). */
static lxb_dom_attr_t *
mkr_attr_find_ns(lxb_dom_element_t *el, lxb_ns_id_t ns_id,
                 const lxb_char_t *local, size_t local_len)
{
    for (lxb_dom_attr_t *at = el->first_attr; at != NULL; at = at->next) {
        if (mkr_attr_own_ns(at) != ns_id) {
            continue;
        }
        /* Compare the case-preserved local name (the suffix of the qualified
         * name): Lexbor lower-cases the stored local_name even when the
         * qualified name keeps its case, but setAttributeNS is case-sensitive. */
        size_t qlen = 0, llen = 0;
        const lxb_char_t *q = lxb_dom_attr_qualified_name(at, &qlen);
        (void) lxb_dom_attr_local_name(at, &llen);
        if (q != NULL && qlen >= llen
            && llen == local_len
            && memcmp(q + (qlen - llen), local, local_len) == 0) {
            return at;
        }
    }
    return NULL;
}

/* element.set_attribute_ns(namespace_or_nil, qualified_name, value) -> value.
 *
 * Stores the attribute under its qualified name (case-preserved — setAttributeNS
 * is case-sensitive, unlike the HTML setAttribute family) and records its OWN
 * namespace on the attr node, so namespaceURI / getAttributeNS resolve it. The
 * namespace URI is interned in the document's ns table; nil/"" stores the null
 * namespace (LXB_NS__UNDEF). */
static VALUE
mkr_node_set_attribute_ns(VALUE self, VALUE rb_ns, VALUE rb_qname, VALUE rb_value)
{
    lxb_dom_node_t *node = mkr_node_unwrap_mutable(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        rb_raise(mkr_eError, "cannot set an attribute on a non-element node");
    }
    lxb_dom_element_t *el = lxb_dom_interface_element(node);

    mkr_ruby_borrowed_text_t qv = mkr_ruby_verified_text(rb_qname, "attribute qualified name");
    mkr_ruby_borrowed_text_t vv = mkr_ruby_verified_text(rb_value, "attribute value");

    mkr_ruby_borrowed_text_t nv = {0};
    bool have_ns = false;
    if (!NIL_P(rb_ns)) {
        nv = mkr_ruby_verified_text(rb_ns, "namespace");
        have_ns = nv.len > 0;
    }

    /* Intern the wanted namespace (null/"" => LXB_NS__UNDEF) so the existing
     * attribute is matched on (namespace, local name) — the DOM key — rather than
     * the qualified name. */
    lxb_ns_id_t want_ns = LXB_NS__UNDEF;
    if (have_ns && node->owner_document != NULL && node->owner_document->ns != NULL) {
        const lxb_ns_data_t *d = lxb_ns_append(node->owner_document->ns,
                                               (const lxb_char_t *)nv.ptr, nv.len);
        if (d != NULL) {
            want_ns = d->ns_id;
        }
    }

    const lxb_char_t *qn = (const lxb_char_t *)qv.ptr;
    const lxb_char_t *colon = memchr(qn, ':', qv.len);
    const lxb_char_t *local = colon ? colon + 1 : qn;
    size_t local_len = colon ? (size_t)(qv.len - (colon - qn) - 1) : qv.len;

    /* A match keeps its qualified name (so re-setting with a different prefix
     * leaves the prefix unchanged); only the value updates. A miss appends a new
     * attribute, even when its qualified name collides with an existing one in a
     * different namespace — the namespace-aware setter splits prefix/local and
     * records the namespace; a null namespace just sets the bare name. */
    lxb_dom_attr_t *attr = mkr_attr_find_ns(el, want_ns, local, local_len);
    if (attr != NULL) {
        if (lxb_dom_attr_set_value(attr, (const lxb_char_t *)vv.ptr, vv.len) != LXB_STATUS_OK) {
            rb_raise(mkr_eError, "failed to set attribute value");
        }
    }
    else {
        attr = lxb_dom_attr_interface_create(node->owner_document);
        if (attr == NULL) {
            rb_raise(mkr_eError, "failed to create attribute");
        }
        /* A fresh attr is calloc'd, so node.ns is already LXB_NS__UNDEF for the
         * null-namespace case; only the namespaced setter changes it. */
        lxb_status_t st;
        if (have_ns) {
            st = lxb_dom_attr_set_name_ns(attr, (const lxb_char_t *)nv.ptr, nv.len,
                                          (const lxb_char_t *)qv.ptr, qv.len, false);
        }
        else {
            st = lxb_dom_attr_set_name(attr, (const lxb_char_t *)qv.ptr, qv.len, false);
        }
        if (st != LXB_STATUS_OK
            || lxb_dom_attr_set_value(attr, (const lxb_char_t *)vv.ptr, vv.len) != LXB_STATUS_OK) {
            /* Leave the un-appended attr for the document arena to free wholesale
             * (the module's "never destroy a detached node" convention). */
            rb_raise(mkr_eError, "failed to set namespaced attribute");
        }
        lxb_dom_element_attr_append(el, attr);
    }

    RB_GC_GUARD(qv.value);
    RB_GC_GUARD(vv.value);
    RB_GC_GUARD(nv.value);
    mkr_invalidate_index(self);
    return rb_value;
}

/* element.remove_attribute_ns(namespace_or_nil, local_name) -> nil. Removes the
 * attribute matching (namespace, local name) — the DOM key — so a namespaced
 * attribute is removed without disturbing a same-qualified-name one in another
 * namespace (which removal by qualified name, case-insensitive for HTML, would). */
static VALUE
mkr_node_remove_attribute_ns(VALUE self, VALUE rb_ns, VALUE rb_local)
{
    lxb_dom_node_t *node = mkr_node_unwrap_mutable(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return Qnil;
    }
    lxb_dom_element_t *el = lxb_dom_interface_element(node);

    mkr_ruby_borrowed_text_t lv = mkr_ruby_verified_text(rb_local, "attribute local name");

    lxb_ns_id_t want_ns = LXB_NS__UNDEF;
    VALUE ns_guard = Qnil;
    if (!NIL_P(rb_ns)) {
        mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_ns, "namespace");
        ns_guard = nv.value;
        if (nv.len > 0 && node->owner_document != NULL && node->owner_document->ns != NULL) {
            const lxb_ns_data_t *d = lxb_ns_append(node->owner_document->ns,
                                                   (const lxb_char_t *)nv.ptr, nv.len);
            if (d != NULL) {
                want_ns = d->ns_id;
            }
        }
    }

    lxb_dom_attr_t *attr = mkr_attr_find_ns(el, want_ns,
                               (const lxb_char_t *)lv.ptr, lv.len);
    if (attr != NULL) {
        lxb_dom_element_attr_remove(el, attr);
        mkr_invalidate_index(self);
    }

    RB_GC_GUARD(lv.value);
    RB_GC_GUARD(ns_guard);
    return Qnil;
}

/* element.name = new_name -> new_name. Renames the element in place (identity
 * preserved): create a throwaway element with the new name so the document
 * interns it, copy its name fields onto this node, then discard it. */
static VALUE
mkr_node_set_name(VALUE self, VALUE rb_name)
{
    lxb_dom_node_t *node = mkr_node_unwrap_mutable(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        rb_raise(mkr_eError, "name= is only supported on elements");
    }
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "element name");
    lxb_dom_element_t *fresh = lxb_dom_document_create_element(
        node->owner_document, (const lxb_char_t *)nv.ptr, nv.len, NULL);
    RB_GC_GUARD(nv.value);
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
    lxb_dom_node_t *node = mkr_node_unwrap_mutable(self);
    mkr_ruby_borrowed_text_t tv = mkr_ruby_verified_text(rb_text, "node content");
    lxb_status_t st = lxb_dom_node_text_content_set(
        node, (const lxb_char_t *)tv.ptr, tv.len);
    RB_GC_GUARD(tv.value);
    if (st != LXB_STATUS_OK) {
        rb_raise(mkr_eError, "failed to set node content");
    }
    mkr_invalidate_index(self);
    return rb_text;
}

/* element.delete(name) -> self. Removes the attribute if present. */
static VALUE
mkr_node_delete(VALUE self, VALUE rb_name)
{
    lxb_dom_node_t *node = mkr_node_unwrap_mutable(self);
    if (node->type != LXB_DOM_NODE_TYPE_ELEMENT) {
        return self;
    }
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "attribute name");
    lxb_dom_element_remove_attribute(
        lxb_dom_interface_element(node), (const lxb_char_t *)nv.ptr, nv.len);
    RB_GC_GUARD(nv.value);
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
    lxb_dom_node_t *node = mkr_node_unwrap_mutable(self);
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
    lxb_dom_node_t *node   = mkr_node_unwrap_mutable(self);
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
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "element name");
    lxb_dom_element_t *el = lxb_dom_document_create_element(
        doc, (const lxb_char_t *)nv.ptr, nv.len, NULL);
    RB_GC_GUARD(nv.value);
    if (el == NULL) {
        rb_raise(mkr_eError, "failed to create element");
    }
    return mkr_wrap_node(lxb_dom_interface_node(el), self);
}

static VALUE
mkr_doc_create_text_node(VALUE self, VALUE rb_text)
{
    lxb_dom_document_t *doc = mkr_doc_unwrap(self);
    mkr_ruby_borrowed_text_t tv = mkr_ruby_verified_text(rb_text, "text content");
    lxb_dom_text_t *t = lxb_dom_document_create_text_node(
        doc, (const lxb_char_t *)tv.ptr, tv.len);
    RB_GC_GUARD(tv.value);
    if (t == NULL) {
        rb_raise(mkr_eError, "failed to create text node");
    }
    return mkr_wrap_node(lxb_dom_interface_node(t), self);
}

static VALUE
mkr_doc_create_comment(VALUE self, VALUE rb_text)
{
    lxb_dom_document_t *doc = mkr_doc_unwrap(self);
    mkr_ruby_borrowed_text_t tv = mkr_ruby_verified_text(rb_text, "comment content");
    lxb_dom_comment_t *c = lxb_dom_document_create_comment(
        doc, (const lxb_char_t *)tv.ptr, tv.len);
    RB_GC_GUARD(tv.value);
    if (c == NULL) {
        rb_raise(mkr_eError, "failed to create comment");
    }
    return mkr_wrap_node(lxb_dom_interface_node(c), self);
}

/* Document#create_processing_instruction(target, data) — DOM
 * createProcessingInstruction: a detached ProcessingInstruction owned by this
 * document. Lexbor validates the target, so an invalid one fails closed. */
static VALUE
mkr_doc_create_processing_instruction(VALUE self, VALUE rb_target, VALUE rb_data)
{
    lxb_dom_document_t *doc = mkr_doc_unwrap(self);
    mkr_ruby_borrowed_text_t tv = mkr_ruby_verified_text(rb_target, "processing instruction target");
    mkr_ruby_borrowed_text_t dv = mkr_ruby_verified_text(rb_data, "processing instruction data");
    lxb_dom_processing_instruction_t *pi = lxb_dom_document_create_processing_instruction(
        doc, (const lxb_char_t *)tv.ptr, tv.len, (const lxb_char_t *)dv.ptr, dv.len);
    RB_GC_GUARD(tv.value);
    RB_GC_GUARD(dv.value);
    if (pi == NULL) {
        rb_raise(mkr_eError, "failed to create processing instruction");
    }
    return mkr_wrap_node(lxb_dom_interface_node(pi), self);
}

/* Document#create_document_fragment — DOM createDocumentFragment: an empty
 * DocumentFragment owned by this document (unlike #fragment / DocumentFragment.parse,
 * which parse HTML; this makes an empty one to build up programmatically). */
static VALUE
mkr_doc_create_document_fragment(VALUE self)
{
    lxb_dom_document_t *doc = mkr_doc_unwrap(self);
    lxb_dom_document_fragment_t *f = lxb_dom_document_create_document_fragment(doc);
    if (f == NULL) {
        rb_raise(mkr_eError, "failed to create document fragment");
    }
    return mkr_wrap_node(lxb_dom_interface_node(f), self);
}

void
mkr_init_mutate(void)
{
    rb_define_method(mkr_mHtmlNodeMethods, "add_child",            mkr_node_add_child,            1);
    rb_define_method(mkr_mHtmlNodeMethods, "<<",                   mkr_node_append,               1);
    rb_define_method(mkr_mHtmlNodeMethods, "add_previous_sibling", mkr_node_add_previous_sibling, 1);
    rb_define_method(mkr_mHtmlNodeMethods, "before",               mkr_node_add_previous_sibling, 1);
    rb_define_method(mkr_mHtmlNodeMethods, "add_next_sibling",     mkr_node_add_next_sibling,     1);
    rb_define_method(mkr_mHtmlNodeMethods, "after",                mkr_node_add_next_sibling,     1);
    rb_define_method(mkr_mHtmlNodeMethods, "remove",               mkr_node_remove,               0);
    rb_define_method(mkr_mHtmlNodeMethods, "unlink",               mkr_node_remove,               0);
    rb_define_method(mkr_mHtmlNodeMethods, "replace",              mkr_node_replace,              1);

    rb_define_method(mkr_mHtmlNodeMethods, "inner_html=", mkr_node_set_inner_html, 1);
    rb_define_method(mkr_mHtmlNodeMethods, "outer_html=", mkr_node_set_outer_html, 1);

    rb_define_method(mkr_mHtmlNodeMethods, "[]=",              mkr_node_aset,             2);
    rb_define_method(mkr_mHtmlNodeMethods, "set_attribute_ns", mkr_node_set_attribute_ns, 3);
    rb_define_method(mkr_mHtmlNodeMethods, "remove_attribute_ns", mkr_node_remove_attribute_ns, 2);
    rb_define_method(mkr_mHtmlNodeMethods, "delete",           mkr_node_delete, 1);
    rb_define_method(mkr_mHtmlNodeMethods, "remove_attribute", mkr_node_delete, 1);
    rb_define_method(mkr_mHtmlNodeMethods, "content=",         mkr_node_set_content, 1);
    rb_define_method(mkr_mHtmlNodeMethods, "name=",            mkr_node_set_name,    1);

    rb_define_method(mkr_cHtmlDocument, "create_element",   mkr_doc_create_element,   1);
    rb_define_method(mkr_cHtmlDocument, "create_text_node", mkr_doc_create_text_node, 1);
    rb_define_method(mkr_cHtmlDocument, "create_comment",   mkr_doc_create_comment,   1);
    rb_define_method(mkr_cHtmlDocument, "create_processing_instruction",
                     mkr_doc_create_processing_instruction, 2);
    rb_define_method(mkr_cHtmlDocument, "create_document_fragment",
                     mkr_doc_create_document_fragment, 0);
}
