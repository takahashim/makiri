#include "glue.h"

#include <lexbor/css/css.h>
#include <lexbor/selectors/selectors.h>

/*
 * CSS selector queries, delegated to Lexbor's lxb_selectors engine.
 *
 *   Node#css(selector)    -> NodeSet (descendants matching, document order)
 *   Node#at_css(selector) -> first matching descendant, or nil
 *
 * Selectors are compiled per call (a per-context cache is a later concern).
 * A malformed selector raises Makiri::CSS::SyntaxError.
 */

typedef struct {
    VALUE           set;
    lxb_dom_node_t *root;       /* excluded from results: css is descendant-only */
    size_t          count;
    int             overflow;
    int             first_only;
} mkr_css_ctx_t;

static lxb_status_t
mkr_css_find_cb(lxb_dom_node_t *node, lxb_css_selector_specificity_t spec,
                void *ctx_)
{
    (void)spec;
    mkr_css_ctx_t *c = (mkr_css_ctx_t *)ctx_;

    if (node == c->root) {
        return LXB_STATUS_OK; /* descendant-only, like Nokogiri's node.css */
    }
    if (c->count >= MKR_NODE_SET_MAX) {
        c->overflow = 1;
        return LXB_STATUS_STOP; /* fail closed without raising mid-traversal */
    }

    mkr_node_set_push(c->set, (mkr_raw_node_t *)node);
    c->count++;
    return c->first_only ? LXB_STATUS_STOP : LXB_STATUS_OK;
}

/* Callback for matches?: signals that the node matched the selector. */
static lxb_status_t
mkr_css_match_cb(lxb_dom_node_t *node, lxb_css_selector_specificity_t spec,
                 void *ctx_)
{
    (void)node; (void)spec;
    *(int *)ctx_ = 1;
    return LXB_STATUS_STOP;
}

/* Compile +rb_selector+, hand the established engine + parsed list to +run+
 * (which performs the actual find / match_node against +node+), then tear the
 * Lexbor CSS object graph down in the required order and fail closed. Centralises
 * the engine lifecycle so the construction/teardown order — a delicate
 * Lexbor-specific detail — lives in exactly one place. Raises
 * Makiri::CSS::SyntaxError on a bad selector, Makiri::Error on engine init
 * failure; any result-specific limits are checked by the caller after return. */
static void
mkr_with_compiled_selector(VALUE rb_selector, lxb_dom_node_t *node,
                           lxb_status_t (*run)(lxb_selectors_t *, lxb_dom_node_t *,
                                               lxb_css_selector_list_t *, void *),
                           void *u)
{
    mkr_ruby_borrowed_text_t sv = mkr_ruby_verified_text(rb_selector, "CSS selector");
    const lxb_char_t *s = (const lxb_char_t *)sv.ptr;
    size_t slen = sv.len;

    /* Lexbor CSS object graph: memory arena -> parser -> selector parser, plus
     * the traversal engine. All transient; torn down before we return. */
    lxb_css_memory_t    *mem       = lxb_css_memory_create();
    lxb_css_parser_t    *parser    = lxb_css_parser_create();
    lxb_css_selectors_t *css_sel   = lxb_css_selectors_create();
    lxb_selectors_t     *selectors = lxb_selectors_create();
    lxb_css_selector_list_t *list  = NULL;

    int ok = (mem != NULL && parser != NULL && css_sel != NULL && selectors != NULL);
    if (ok) {
        ok = (lxb_css_memory_init(mem, 128) == LXB_STATUS_OK)
          && (lxb_css_parser_init(parser, NULL) == LXB_STATUS_OK)
          && (lxb_css_selectors_init(css_sel) == LXB_STATUS_OK)
          && (lxb_selectors_init(selectors) == LXB_STATUS_OK);
    }

    int syntax_error = 0;
    if (ok) {
        lxb_css_parser_memory_set(parser, mem);
        lxb_css_parser_selectors_set(parser, css_sel);
        list = lxb_css_selectors_parse(parser, s, slen);
        if (list == NULL || parser->status != LXB_STATUS_OK) {
            syntax_error = 1;
        } else {
            (void)run(selectors, node, list, u);
        }
    }

    /* Teardown (order mirrors Lexbor's own examples). Destroying the memory
     * arena frees the parsed selector list, so we do not free it separately. */
    if (selectors != NULL) lxb_selectors_destroy(selectors, true);
    if (parser != NULL)    lxb_css_parser_destroy(parser, true);
    if (mem != NULL)       lxb_css_memory_destroy(mem, true);
    if (css_sel != NULL)   lxb_css_selectors_destroy(css_sel, true);

    if (!ok) {
        rb_raise(mkr_eError, "failed to initialise CSS selector engine");
    }
    if (syntax_error) {
        rb_raise(mkr_eCSSSyntaxError, "invalid CSS selector: %" PRIsVALUE, sv.value);
    }
    RB_GC_GUARD(sv.value);
}

/* find: collect descendants matching the selector (MATCH_FIRST dedups a node
 * that matches several selectors in a comma list). */
static lxb_status_t
mkr_run_find(lxb_selectors_t *selectors, lxb_dom_node_t *root,
             lxb_css_selector_list_t *list, void *u)
{
    lxb_selectors_opt_set(selectors, LXB_SELECTORS_OPT_MATCH_FIRST);
    return lxb_selectors_find(selectors, root, list, mkr_css_find_cb, u);
}

/* match_node: does THIS node match? */
static lxb_status_t
mkr_run_match(lxb_selectors_t *selectors, lxb_dom_node_t *node,
              lxb_css_selector_list_t *list, void *u)
{
    return lxb_selectors_match_node(selectors, node, list, mkr_css_match_cb, u);
}

/* Run +selector+ against self's subtree. first_only stops after one match.
 * Raises Makiri::CSS::SyntaxError on a bad selector, Makiri::Error on
 * allocation failure or an over-large result. */
static VALUE
mkr_css_query(VALUE self, VALUE rb_selector, int first_only)
{
    lxb_dom_node_t *root = mkr_html_node_unwrap(self);
    VALUE document = mkr_node_document(self);
    VALUE set = mkr_node_set_new(document);

    mkr_css_ctx_t ctx = { .set = set, .root = root, .count = 0,
                          .overflow = 0, .first_only = first_only };
    mkr_with_compiled_selector(rb_selector, root, mkr_run_find, &ctx);

    if (ctx.overflow) {
        rb_raise(mkr_eError, "CSS result set exceeded the node limit (%u)",
                 MKR_NODE_SET_MAX);
    }
    return set;
}

/* Node#matches?(selector): does THIS node match the CSS selector? (Like
 * Nokogiri — tested against the node itself, not its descendants.) A malformed
 * selector raises Makiri::CSS::SyntaxError. */
static VALUE
mkr_node_matches(VALUE self, VALUE rb_selector)
{
    lxb_dom_node_t *node = mkr_html_node_unwrap(self);
    int matched = 0;
    mkr_with_compiled_selector(rb_selector, node, mkr_run_match, &matched);
    return matched ? Qtrue : Qfalse;
}

static VALUE
mkr_node_css(VALUE self, VALUE rb_selector)
{
    return mkr_css_query(self, rb_selector, 0);
}

static VALUE
mkr_node_at_css(VALUE self, VALUE rb_selector)
{
    VALUE set = mkr_css_query(self, rb_selector, 1);
    return rb_funcall(set, rb_intern("first"), 0);
}

void
mkr_init_css(void)
{
    rb_define_method(mkr_mHtmlNodeMethods, "css",     mkr_node_css,     1);
    rb_define_method(mkr_mHtmlNodeMethods, "at_css",  mkr_node_at_css,  1);
    rb_define_method(mkr_mHtmlNodeMethods, "matches?", mkr_node_matches, 1);
}
