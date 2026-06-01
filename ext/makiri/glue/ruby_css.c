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

    mkr_node_set_push(c->set, node);
    c->count++;
    return c->first_only ? LXB_STATUS_STOP : LXB_STATUS_OK;
}

/* Run +selector+ against self's subtree. first_only stops after one match.
 * Raises Makiri::CSS::SyntaxError on a bad selector, Makiri::Error on
 * allocation failure or an over-large result. */
static VALUE
mkr_css_query(VALUE self, VALUE rb_selector, int first_only)
{
    lxb_dom_node_t *root = mkr_node_unwrap(self);
    VALUE document = mkr_node_document(self);

    VALUE sel = rb_String(rb_selector);
    const lxb_char_t *s = (const lxb_char_t *)RSTRING_PTR(sel);
    size_t slen = (size_t)RSTRING_LEN(sel);

    VALUE set = mkr_node_set_new(document);

    /* Lexbor CSS object graph: memory arena -> parser -> selector parser, plus
     * the traversal engine. All transient; torn down before we return. */
    lxb_css_memory_t    *mem      = lxb_css_memory_create();
    lxb_css_parser_t    *parser   = lxb_css_parser_create();
    lxb_css_selectors_t *css_sel  = lxb_css_selectors_create();
    lxb_selectors_t     *selectors = lxb_selectors_create();
    lxb_css_selector_list_t *list = NULL;

    int ok = (mem != NULL && parser != NULL && css_sel != NULL && selectors != NULL);
    if (ok) {
        ok = (lxb_css_memory_init(mem, 128) == LXB_STATUS_OK)
          && (lxb_css_parser_init(parser, NULL) == LXB_STATUS_OK)
          && (lxb_css_selectors_init(css_sel) == LXB_STATUS_OK)
          && (lxb_selectors_init(selectors) == LXB_STATUS_OK);
    }

    int syntax_error = 0;
    mkr_css_ctx_t ctx = { .set = set, .root = root, .count = 0,
                          .overflow = 0, .first_only = first_only };

    if (ok) {
        lxb_css_parser_memory_set(parser, mem);
        lxb_css_parser_selectors_set(parser, css_sel);

        list = lxb_css_selectors_parse(parser, s, slen);
        if (list == NULL || parser->status != LXB_STATUS_OK) {
            syntax_error = 1;
        } else {
            /* MATCH_FIRST collapses duplicates when a node matches several
             * selectors in a comma list. */
            lxb_selectors_opt_set(selectors, LXB_SELECTORS_OPT_MATCH_FIRST);
            (void)lxb_selectors_find(selectors, root, list,
                                     mkr_css_find_cb, &ctx);
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
        rb_raise(mkr_eCSSSyntaxError, "invalid CSS selector: %" PRIsVALUE, sel);
    }
    if (ctx.overflow) {
        rb_raise(mkr_eError, "CSS result set exceeded the node limit (%u)",
                 MKR_NODE_SET_MAX);
    }

    return set;
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
    rb_define_method(mkr_cNode, "css",    mkr_node_css,    1);
    rb_define_method(mkr_cNode, "at_css", mkr_node_at_css, 1);
}
