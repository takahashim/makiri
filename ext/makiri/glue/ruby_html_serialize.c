#include "glue.h"

#include <lexbor/html/serialize.h>

/*
 * HTML serialization, delegated to Lexbor's serializer.
 *
 *   Node#to_html / #to_s / #outer_html -> the node and its subtree (outer)
 *   Node#inner_html                    -> the node's children only (inner)
 *
 * Lexbor's serializer streams the output in many small chunks (one per tag /
 * attribute / text piece). We collect them into a single growing C buffer
 * (mkr_buf) and copy that into a Ruby String once at the end, instead of
 * rb_str_cat per chunk - the per-chunk Ruby-string growth (a capacity check +
 * coderange bookkeeping on each of thousands of appends) was the dominant cost.
 * The buffer is **pre-reserved** to roughly the output size up front, so the
 * per-chunk appends do not realloc on every geometric step - growing in cheap,
 * GC-untracked C memory and paying one final copy (vs serializing straight into
 * a growing Ruby String, where the intermediate growth pressures the GC and
 * measured slower). Lexbor emits UTF-8, which is the string's encoding.
 *
 * Mutating setters (inner_html=, outer_html=) arrive with the v0.2 mutation
 * API and are not defined here.
 */

static lxb_status_t
mkr_serialize_cb(const lxb_char_t *data, size_t len, void *ctx)
{
    return mkr_buf_append((mkr_buf_t *)ctx, data, len) == MKR_OK
               ? LXB_STATUS_OK
               : LXB_STATUS_ERROR_MEMORY_ALLOCATION;
}

/* Copy the collected bytes into one UTF-8 Ruby String, always freeing the
 * buffer; raises if the serializer (or an append) failed. */
static VALUE
mkr_serialized_str(mkr_buf_t *buf, lxb_status_t st)
{
    if (st != LXB_STATUS_OK) {
        mkr_buf_free(buf);
        rb_raise(mkr_eError, "HTML serialization failed");
    }
    VALUE str = rb_utf8_str_new(buf->len ? buf->data : "", (long)buf->len);
    mkr_buf_free(buf);
    return str;
}

/* The serializer's buffer cap + initial reservation, both derived from the
 * document's live bytes in one walk. The cap is the Lexbor analogue of the XML
 * serializer's arena_bytes cap (mkr_xml_serialize_cap): 32x the live bytes
 * (covering escaping + maximal pretty indentation) plus a 64 KiB floor, clamped
 * to MKR_BUF_HARD_MAX - tight for a small document yet scaling with a large one
 * so a legitimate parse round-trips through to_html (HTML parsing is itself
 * byte-uncapped); a pathologically deep pretty-print exceeds it and fails closed
 * (MKR_ERR_LIMIT) rather than growing without bound. The HTML tree cannot cycle
 * (mutation guards + Lexbor's insert checks), so the cap is never reached in
 * normal operation. The reservation is ~live/4: the serialized output is a
 * fraction of the arena (96-byte node structs dwarf their markup), so this
 * pre-sizes close to the real output without a wasteful over-allocation, leaving
 * the geometric growth to cover any underestimate. */
static void
mkr_html_serialize_sizes(lxb_dom_node_t *node, size_t *cap, size_t *reserve)
{
    size_t live = mkr_lxb_document_bytes(node);

    size_t c = 65536; /* floor for a small subtree */
    if (live > 0) {
        c = (live <= (SIZE_MAX - c) / 32) ? c + live * 32 : SIZE_MAX;
    }
    if (c > MKR_BUF_HARD_MAX) {
        c = MKR_BUF_HARD_MAX;
    }
    *cap = c;

    size_t r = live / 4;
    if (r < 4096) {
        r = 4096;
    }
    *reserve = r;
}

/* Serialize +node+ into a fresh UTF-8 String. +deep+ selects the children-only
 * (inner) serializer over the tree (outer) one; +pretty+ selects indented
 * output. Raises Makiri::Error on serializer failure or if the output exceeds
 * the content-scaled cap. */
static VALUE
mkr_html_serialize(lxb_dom_node_t *node, int deep, int pretty)
{
    size_t cap, reserve;
    mkr_html_serialize_sizes(node, &cap, &reserve);

    mkr_buf_t buf;
    mkr_buf_init(&buf, cap);
    (void)mkr_buf_reserve(&buf, reserve); /* best-effort pre-size */

    lxb_status_t st;
    if (deep) {
        st = pretty
            ? lxb_html_serialize_pretty_deep_cb(node, LXB_HTML_SERIALIZE_OPT_UNDEF,
                                                0, mkr_serialize_cb, &buf)
            : lxb_html_serialize_deep_cb(node, mkr_serialize_cb, &buf);
    } else {
        st = pretty
            ? lxb_html_serialize_pretty_tree_cb(node, LXB_HTML_SERIALIZE_OPT_UNDEF,
                                                0, mkr_serialize_cb, &buf)
            : lxb_html_serialize_tree_cb(node, mkr_serialize_cb, &buf);
    }
    return mkr_serialized_str(&buf, st);
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
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);

    /* A document fragment has no tag of its own; "outer" == its children, so the
     * deep (children) serializer is the right one (the tree serializer rejects a
     * fragment node). */
    int deep = (node->type == LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT);
    return mkr_html_serialize(node, deep, pretty);
}

/* Inner HTML: the node's children, without the node's own tag. */
static VALUE
mkr_node_inner_html(int argc, VALUE *argv, VALUE self)
{
    int pretty = mkr_serialize_pretty_opt(argc, argv);
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    return mkr_html_serialize(node, 1 /* deep */, pretty);
}

void
mkr_init_serialize(void)
{
    rb_define_method(mkr_mHtmlNodeMethods, "to_html",    mkr_node_to_html,    -1);
    rb_define_method(mkr_mHtmlNodeMethods, "to_s",       mkr_node_to_html,    -1);
    rb_define_method(mkr_mHtmlNodeMethods, "outer_html", mkr_node_to_html,    -1);
    rb_define_method(mkr_mHtmlNodeMethods, "inner_html", mkr_node_inner_html, -1);
}
