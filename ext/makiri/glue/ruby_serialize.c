#include "glue.h"

#include <lexbor/html/serialize.h>

/*
 * HTML serialization, delegated to Lexbor's serializer.
 *
 *   Node#to_html / #to_s / #outer_html -> the node and its subtree (outer)
 *   Node#inner_html                    -> the node's children only (inner)
 *
 * We use the callback variants so output streams straight into a Ruby String;
 * Lexbor emits UTF-8, which is the string's encoding.
 *
 * Mutating setters (inner_html=, outer_html=) arrive with the v0.2 mutation
 * API and are not defined here.
 */

static lxb_status_t
mkr_serialize_cb(const lxb_char_t *data, size_t len, void *ctx)
{
    rb_str_cat((VALUE)ctx, (const char *)data, (long)len);
    return LXB_STATUS_OK;
}

/* Read the optional `pretty:` keyword. */
static int
mkr_serialize_pretty_opt(int argc, VALUE *argv)
{
    VALUE opts = Qnil;
    rb_scan_args(argc, argv, "0:", &opts);
    if (NIL_P(opts)) {
        return 0;
    }
    return RTEST(rb_hash_aref(opts, ID2SYM(rb_intern("pretty"))));
}

/* Outer HTML: the node itself plus its descendants.
 * Pass `pretty: true` for indented output. */
static VALUE
mkr_node_to_html(int argc, VALUE *argv, VALUE self)
{
    int pretty = mkr_serialize_pretty_opt(argc, argv);
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    VALUE str = rb_utf8_str_new(NULL, 0);

    /* A document fragment has no tag of its own; "outer" == its children, so
     * the deep (children) serializer is the right one (the tree serializer
     * rejects a fragment node). */
    int deep = (node->type == LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT);
    lxb_status_t st;
    if (deep) {
        st = pretty
            ? lxb_html_serialize_pretty_deep_cb(node, LXB_HTML_SERIALIZE_OPT_UNDEF,
                                                0, mkr_serialize_cb, (void *)str)
            : lxb_html_serialize_deep_cb(node, mkr_serialize_cb, (void *)str);
    } else {
        st = pretty
            ? lxb_html_serialize_pretty_tree_cb(node, LXB_HTML_SERIALIZE_OPT_UNDEF,
                                                0, mkr_serialize_cb, (void *)str)
            : lxb_html_serialize_tree_cb(node, mkr_serialize_cb, (void *)str);
    }
    if (st != LXB_STATUS_OK) {
        rb_raise(mkr_eError, "HTML serialization failed");
    }
    return str;
}

/* Inner HTML: the node's children, without the node's own tag. */
static VALUE
mkr_node_inner_html(int argc, VALUE *argv, VALUE self)
{
    int pretty = mkr_serialize_pretty_opt(argc, argv);
    lxb_dom_node_t *node = mkr_node_unwrap(self);
    VALUE str = rb_utf8_str_new(NULL, 0);
    lxb_status_t st = pretty
        ? lxb_html_serialize_pretty_deep_cb(node, LXB_HTML_SERIALIZE_OPT_UNDEF,
                                            0, mkr_serialize_cb, (void *)str)
        : lxb_html_serialize_deep_cb(node, mkr_serialize_cb, (void *)str);
    if (st != LXB_STATUS_OK) {
        rb_raise(mkr_eError, "HTML serialization failed");
    }
    return str;
}

void
mkr_init_serialize(void)
{
    rb_define_method(mkr_cNode, "to_html",    mkr_node_to_html,    -1);
    rb_define_method(mkr_cNode, "to_s",       mkr_node_to_html,    -1);
    rb_define_method(mkr_cNode, "outer_html", mkr_node_to_html,    -1);
    rb_define_method(mkr_cNode, "inner_html", mkr_node_inner_html, -1);
}
